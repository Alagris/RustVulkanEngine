use crate::ecs::Component;
use crate::ecs::component::{ComponentRegistry, Unique, component_ref};
use crate::ecs::game_object::{GameObject, GameObjectWeak};
use crate::ecs::game_object_serde::GameObjectDefinition;
use crate::ecs::scene_serde::SceneDefinition;
use crate::graphics::Graphics;
use crate::input::{InputCallback, InputSystem, RawInput};
use anyhow::{Context, Result, anyhow, bail};
use cgmath::{Matrix4, SquareMatrix};
use std::any::TypeId;
use std::cell::{Cell, RefCell};
use std::fs;
use std::path::Path;
use std::rc::{Rc, Weak};

enum PendingMutation {
    Add {
        owner_id: u64,
        component: Box<dyn crate::ecs::Component>,
    },
    Remove {
        owner_id: u64,
        type_id: TypeId,
    },
    Mutate {
        target: GameObjectWeak,
        mutation: Box<dyn FnOnce(&mut GameObject)>,
    },
}

pub struct Scene {
    pub root: Unique<GameObject>,
    pub(crate) components: Vec<Weak<RefCell<dyn Component>>>,
    delta_time: Cell<f32>,
    time_since_start: Cell<f32>,
    initialized: Cell<bool>,
    destroyed: Cell<bool>,
    pending_mutations: RefCell<Vec<PendingMutation>>,
    input_system: RefCell<InputSystem>,
}

impl Scene {
    pub fn validate_yaml(source: &str) -> Result<()> {
        let definition: SceneDefinition =
            serde_yaml::from_str(source).context("Failed to parse scene YAML")?;
        validate_timing(definition.delta_time, definition.time_since_start)?;
        validate_game_object_definition(definition.root)
    }

    pub fn load(path: &Path, registry: &ComponentRegistry) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("Failed to read scene {}", path.display()))?;
        Self::from_yaml(&source, registry)
            .with_context(|| format!("Invalid scene {}", path.display()))
    }

    pub fn from_yaml(source: &str, registry: &ComponentRegistry) -> Result<Self> {
        let definition: SceneDefinition =
            serde_yaml::from_str(source).context("Failed to parse scene YAML")?;
        validate_timing(definition.delta_time, definition.time_since_start)?;
        let mut components = Vec::new();
        let root = build_game_object(definition.root, registry, &mut components)?;
        Ok(Self {
            root,
            components,
            delta_time: Cell::new(definition.delta_time),
            time_since_start: Cell::new(definition.time_since_start),
            initialized: Cell::new(false),
            destroyed: Cell::new(false),
            pending_mutations: RefCell::new(Vec::new()),
            input_system: RefCell::new(InputSystem::default()),
        })
    }

    pub fn initialize(&mut self) -> Result<()> {
        assert!(
            !self.initialized.get(),
            "scene lifecycle must be initialized once"
        );
        self.initialize_components()?;
        self.initialized.set(true);
        self.apply_pending_mutations()
    }

    pub fn instantiate_game_object(
        &mut self,
        source: &str,
        registry: &ComponentRegistry,
    ) -> Result<Unique<GameObject>> {
        if self.destroyed.get() {
            bail!("cannot instantiate an object in a destroyed scene");
        }
        let definition: GameObjectDefinition =
            serde_yaml::from_str(source).context("Failed to parse game object YAML")?;
        let mut components = Vec::new();
        let object = build_game_object(definition, registry, &mut components)?;
        self.components.extend(components.iter().cloned());
        self.initialize_component_refs(&components)?;
        if self.initialized.get() && !self.pending_mutations.borrow().is_empty() {
            self.apply_pending_mutations()?;
        }
        Ok(object)
    }

    pub fn update(&mut self, delta_time: f32, graphics: &mut Graphics) -> Result<()> {
        assert!(
            self.initialized.get(),
            "scene must be initialized before updating"
        );
        assert!(!self.destroyed.get(), "destroyed scenes cannot be updated");
        if !delta_time.is_finite() || delta_time < 0.0 {
            return Err(anyhow!("frame delta time must be finite and non-negative"));
        }
        self.delta_time.set(delta_time);
        let time_since_start = self.time_since_start.get() + delta_time;
        if !time_since_start.is_finite() {
            return Err(anyhow!("scene time overflowed its finite range"));
        }
        self.time_since_start.set(time_since_start);
        self.root
            .borrow()
            .update_world_transforms(Matrix4::identity());
        let input_notifications = self.input_system.borrow_mut().update(delta_time)?;
        // The RefMut ends at the previous statement, so callbacks may re-enter input APIs.
        InputSystem::dispatch(self, input_notifications)?;
        let mut write_index = 0;
        let mut update_error = None;
        for read_index in 0..self.components.len() {
            let component_ref = self.components[read_index].clone();
            let Some(component) = component_ref.upgrade() else {
                continue;
            };
            if update_error.is_none() {
                let result = (|| {
                    let mut component = component.borrow_mut();
                    if component.started && component.updates {
                        let owner = component
                            .owner
                            .upgrade()
                            .ok_or_else(|| anyhow!("component owner no longer exists"))?;
                        component.update(self, &owner.borrow(), graphics)?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    update_error = Some(error);
                }
            }
            self.components[write_index] = component_ref;
            write_index += 1;
        }
        self.components.truncate(write_index);
        if let Some(error) = update_error {
            return Err(error);
        }
        if !self.pending_mutations.borrow().is_empty() {
            self.apply_pending_mutations()?;
        }
        Ok(())
    }

    pub fn defer_game_object_mutation(
        &self,
        target: &GameObjectWeak,
        mutation: impl FnOnce(&mut GameObject) + 'static,
    ) -> Result<()> {
        if self.destroyed.get() {
            bail!("cannot mutate a destroyed scene");
        }
        let object = target
            .upgrade()
            .ok_or_else(|| anyhow!("game object {} no longer exists", target.id()))?;
        if !GameObject::contains(&self.root, &object) {
            bail!("game object {} does not belong to this scene", target.id());
        }
        self.pending_mutations
            .borrow_mut()
            .push(PendingMutation::Mutate {
                target: target.clone(),
                mutation: Box::new(mutation),
            });
        Ok(())
    }

    pub fn add_component(
        &self,
        owner: &GameObject,
        component: Box<dyn crate::ecs::Component>,
    ) -> Result<()> {
        if self.destroyed.get() {
            bail!("cannot add a component to a destroyed scene");
        }
        if GameObject::find_by_id(&self.root, owner.id()).is_none() {
            bail!("game object {} does not belong to this scene", owner.id());
        }
        self.pending_mutations
            .borrow_mut()
            .push(PendingMutation::Add {
                owner_id: owner.id(),
                component,
            });
        Ok(())
    }

    pub fn remove_component<T: Component + 'static>(&self, owner: &GameObject) -> Result<()> {
        if self.destroyed.get() {
            bail!("cannot remove a component from a destroyed scene");
        }
        if GameObject::find_by_id(&self.root, owner.id()).is_none() {
            bail!("game object {} does not belong to this scene", owner.id());
        }
        if !owner
            .components
            .iter()
            .any(|component| component.borrow().as_any().is::<T>())
        {
            bail!(
                "game object {} does not have a component of the requested type",
                owner.id()
            );
        }
        self.pending_mutations
            .borrow_mut()
            .push(PendingMutation::Remove {
                owner_id: owner.id(),
                type_id: TypeId::of::<T>(),
            });
        Ok(())
    }

    fn apply_pending_mutations(&mut self) -> Result<()> {
        loop {
            let mutations = std::mem::take(&mut *self.pending_mutations.borrow_mut());
            if mutations.is_empty() {
                return Ok(());
            }
            for mutation in mutations {
                match mutation {
                    PendingMutation::Add {
                        owner_id,
                        component,
                    } => {
                        self.append_component(owner_id, component)?;
                    }
                    PendingMutation::Remove { owner_id, type_id } => {
                        self.remove_component_now(owner_id, type_id)?;
                    }
                    PendingMutation::Mutate { target, mutation } => {
                        let object = target.upgrade().ok_or_else(|| {
                            anyhow!("deferred mutation target {} no longer exists", target.id())
                        })?;
                        if !GameObject::contains(&self.root, &object) {
                            bail!(
                                "deferred mutation target {} no longer belongs to this scene",
                                target.id()
                            );
                        }
                        mutation(&mut object.borrow_mut());
                    }
                }
            }
            self.initialize_components()?;
        }
    }

    fn append_component(
        &mut self,
        owner_id: u64,
        component: Box<dyn crate::ecs::Component>,
    ) -> Result<()> {
        let owner = GameObject::find_by_id(&self.root, owner_id)
            .ok_or_else(|| anyhow!("component owner {owner_id} no longer exists"))?;
        let mut component = component;
        component.owner_id = owner_id;
        component.owner = Rc::downgrade(&owner);
        let component = component_ref(component);
        self.components.push(Rc::downgrade(&component));
        owner.borrow_mut().components.push(component);
        Ok(())
    }

    fn remove_component_now(&mut self, owner_id: u64, type_id: TypeId) -> Result<()> {
        let owner = GameObject::find_by_id(&self.root, owner_id)
            .ok_or_else(|| anyhow!("component owner {owner_id} no longer exists"))?;
        let component_index = owner
            .borrow()
            .components
            .iter()
            .position(|component| component.borrow().as_any().type_id() == type_id)
            .ok_or_else(|| anyhow!("requested component no longer exists on {owner_id}"))?;
        let component = owner.borrow().components[component_index].clone();
        {
            let mut component = component.borrow_mut();
            if component.awoken {
                component.destroy(self, &owner.borrow())?;
            }
        }
        owner.borrow_mut().components.remove(component_index);
        Ok(())
    }

    fn initialize_components(&mut self) -> Result<()> {
        let components = self.components.clone();
        self.initialize_component_refs(&components)
    }

    fn initialize_component_refs(
        &self,
        component_refs: &[Weak<RefCell<dyn Component>>],
    ) -> Result<()> {
        for component_ref in component_refs {
            let Some(component) = component_ref.upgrade() else {
                continue;
            };
            let mut component = component.borrow_mut();
            if component.awoken {
                continue;
            }
            let owner = component
                .owner
                .upgrade()
                .ok_or_else(|| anyhow!("component owner no longer exists"))?;
            component.awoken = true;
            component.awake(self, &owner.borrow());
            component.updates = component.start(self, &owner.borrow());
            component.started = true;
        }
        Ok(())
    }

    pub fn delta_time(&self) -> f32 {
        self.delta_time.get()
    }

    pub fn time_since_start(&self) -> f32 {
        self.time_since_start.get()
    }

    pub fn update_component_count(&self) -> usize {
        self.components
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|component| component.borrow().updates)
            .count()
    }

    pub fn component_count(&self) -> usize {
        self.components
            .iter()
            .filter(|component| component.upgrade().is_some())
            .count()
    }

    pub fn game_object_count(&self) -> usize {
        count_game_objects(&self.root)
    }

    pub fn destroy(&self) -> Result<()> {
        assert!(
            self.initialized.get(),
            "scene must be initialized before destruction"
        );
        if self.destroyed.replace(true) {
            return Ok(());
        }
        self.root.borrow().destroy_components(self)
    }

    pub fn find_game_object(&self, id: u64) -> Option<Unique<GameObject>> {
        GameObject::find_by_id(&self.root, id)
    }

    pub fn resolve_game_object(
        &self,
        object: &crate::ecs::GameObjectWeak,
    ) -> Option<Unique<GameObject>> {
        let object = object.upgrade()?;
        GameObject::contains(&self.root, &object).then_some(object)
    }

    pub fn find_component<T: crate::ecs::Component + 'static>(
        &self,
    ) -> Option<crate::ecs::ComponentWeak<T>> {
        self.components.iter().find_map(|component| {
            let component = component.upgrade()?;
            let borrowed = component.try_borrow().ok()?;
            borrowed
                .as_any()
                .is::<T>()
                .then(|| crate::ecs::ComponentWeak::new(Rc::downgrade(&component)))
        })
    }

    pub fn configure_input(&mut self, input_system: InputSystem) -> Result<()> {
        if self.initialized.get() {
            bail!("input must be configured before scene initialization");
        }
        *self.input_system.get_mut() = input_system;
        Ok(())
    }

    pub fn active_input_map(&self) -> Option<String> {
        self.input_system.borrow().active_map().map(str::to_owned)
    }

    pub fn set_active_input_map(&self, name: &str) -> Result<()> {
        let notifications = self.input_system.borrow_mut().set_active_map(name)?;
        // Drop the RefMut before callbacks can change the active map or subscriptions.
        InputSystem::dispatch(self, notifications)?;
        Ok(())
    }

    pub fn subscribe_input<T: Component + 'static>(
        &self,
        map: &str,
        binding: &str,
        component: &T,
        callback: InputCallback<T>,
    ) -> Result<()> {
        let component = component
            .weak_component::<T>()
            .ok_or_else(|| anyhow!("input subscription component is not scene-managed"))?;
        self.input_system
            .borrow_mut()
            .subscribe(map, binding, &component, callback)
    }

    pub fn unsubscribe_input<T: Component + 'static>(&self, component: &T) -> Result<usize> {
        let component = component
            .weak_component::<T>()
            .ok_or_else(|| anyhow!("input subscription component is not scene-managed"))?;
        Ok(self
            .input_system
            .borrow_mut()
            .unsubscribe_component(&component))
    }

    pub fn unsubscribe_input_callback<T: Component + 'static>(
        &self,
        component: &T,
        callback: InputCallback<T>,
    ) -> Result<usize> {
        let component = component
            .weak_component::<T>()
            .ok_or_else(|| anyhow!("input subscription component is not scene-managed"))?;
        Ok(self
            .input_system
            .borrow_mut()
            .unsubscribe_callback(&component, callback))
    }

    pub fn consume_input(&self, input: RawInput) -> Result<()> {
        let notifications = self.input_system.borrow_mut().consume(input)?;
        // Callbacks may re-enter Scene input APIs, so dispatch after the RefMut is dropped.
        InputSystem::dispatch(self, notifications)?;
        Ok(())
    }
}

fn validate_timing(delta_time: f32, time_since_start: f32) -> Result<()> {
    if !delta_time.is_finite()
        || delta_time < 0.0
        || !time_since_start.is_finite()
        || time_since_start < 0.0
    {
        bail!("scene timing values must be finite and non-negative");
    }
    Ok(())
}

fn validate_game_object_definition(definition: GameObjectDefinition) -> Result<()> {
    if definition.name.trim().is_empty() {
        bail!("game object names must not be empty");
    }
    definition.transform.to_transform()?;
    for component in definition.components {
        if component.type_name.trim().is_empty() {
            bail!("component type names must not be empty");
        }
    }
    for child in definition.children {
        validate_game_object_definition(child)?;
    }
    Ok(())
}

fn build_game_object(
    definition: GameObjectDefinition,
    registry: &ComponentRegistry,
    components: &mut Vec<Weak<RefCell<dyn Component>>>,
) -> Result<Unique<GameObject>> {
    if definition.name.trim().is_empty() {
        bail!("game object names must not be empty");
    }
    let object = GameObject::new(
        definition.name,
        definition.transform.to_transform()?,
        Vec::new(),
    );
    for component_definition in definition.components {
        let mut component =
            registry.create(&component_definition.type_name, &component_definition.data)?;
        component.owner_id = object.borrow().id();
        component.owner = Rc::downgrade(&object);
        let component = component_ref(component);
        components.push(Rc::downgrade(&component));
        object.borrow_mut().components.push(component);
    }
    for child in definition.children {
        object
            .borrow_mut()
            .add_child(build_game_object(child, registry, components)?);
    }
    Ok(object)
}

fn count_game_objects(object: &Unique<GameObject>) -> usize {
    let object = object.borrow();
    1 + object
        .children()
        .iter()
        .map(count_game_objects)
        .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::component::ComponentInstance;
    use crate::ecs::{Component, ComponentRegistry};
    use anyhow::Result;
    use cgmath::Vector3;
    use std::any::Any;
    use std::cell::Cell;
    use std::rc::Rc;

    #[crate::ecs::component(
        serialize_with = serialize_lifecycle_probe,
        deserialize_with = deserialize_lifecycle_probe
    )]
    #[derive(crate::ecs::Component)]
    struct LifecycleProbe {
        awakes: Rc<Cell<usize>>,
        starts: Rc<Cell<usize>>,
        updates: Rc<Cell<usize>>,
        update_enabled: bool,
    }

    struct LifecycleFactoryState {
        awakes: Rc<Cell<usize>>,
        starts: Rc<Cell<usize>>,
        updates: Rc<Cell<usize>>,
        update_enabled: bool,
    }

    fn deserialize_lifecycle_probe(
        _data: &serde_yaml::Value,
        context: Option<&dyn Any>,
    ) -> Result<LifecycleProbe> {
        let state = context
            .and_then(|context| context.downcast_ref::<LifecycleFactoryState>())
            .ok_or_else(|| anyhow!("LifecycleProbe requires test factory context"))?;
        Ok(LifecycleProbe {
            instance: ComponentInstance::default(),
            awakes: Rc::clone(&state.awakes),
            starts: Rc::clone(&state.starts),
            updates: Rc::clone(&state.updates),
            update_enabled: state.update_enabled,
        })
    }

    fn serialize_lifecycle_probe(component: &LifecycleProbe) -> Result<serde_yaml::Value> {
        Ok(serde_yaml::Value::Bool(component.update_enabled))
    }

    impl Component for LifecycleProbe {
        fn awake(&mut self, _scene: &Scene, _game_object: &GameObject) {
            self.awakes.set(self.awakes.get() + 1);
        }
        fn start(&mut self, _scene: &Scene, _game_object: &GameObject) -> bool {
            self.starts.set(self.starts.get() + 1);
            self.update_enabled
        }
        fn update(
            &mut self,
            _scene: &Scene,
            _game_object: &GameObject,
            _graphics: &mut Graphics,
        ) -> Result<()> {
            self.updates.set(self.updates.get() + 1);
            Ok(())
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[crate::ecs::component(deserialize_with = deserialize_owner_probe)]
    #[derive(crate::ecs::Component)]
    struct OwnerProbe {
        calls: Rc<RefCell<Vec<(&'static str, u64)>>>,
    }

    fn deserialize_owner_probe(
        _data: &serde_yaml::Value,
        context: Option<&dyn Any>,
    ) -> Result<OwnerProbe> {
        let calls = context
            .and_then(|context| context.downcast_ref::<Rc<RefCell<Vec<(&'static str, u64)>>>>())
            .ok_or_else(|| anyhow!("OwnerProbe requires test factory context"))?;
        Ok(OwnerProbe {
            instance: ComponentInstance::default(),
            calls: Rc::clone(calls),
        })
    }

    impl Component for OwnerProbe {
        fn awake(&mut self, _scene: &Scene, game_object: &GameObject) {
            self.calls.borrow_mut().push(("awake", game_object.id()));
        }
        fn start(&mut self, _scene: &Scene, game_object: &GameObject) -> bool {
            self.calls.borrow_mut().push(("start", game_object.id()));
            true
        }
        fn update(
            &mut self,
            _scene: &Scene,
            game_object: &GameObject,
            _graphics: &mut Graphics,
        ) -> Result<()> {
            self.calls.borrow_mut().push(("update", game_object.id()));
            Ok(())
        }
        fn destroy(&mut self, _scene: &Scene, game_object: &GameObject) -> Result<()> {
            self.calls.borrow_mut().push(("destroy", game_object.id()));
            Ok(())
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    fn registry(
        update_enabled: bool,
    ) -> (
        ComponentRegistry,
        Rc<Cell<usize>>,
        Rc<Cell<usize>>,
        Rc<Cell<usize>>,
    ) {
        let awakes = Rc::new(Cell::new(0));
        let starts = Rc::new(Cell::new(0));
        let updates = Rc::new(Cell::new(0));
        let mut registry = ComponentRegistry::new();
        registry.set_context(LifecycleFactoryState {
            awakes: Rc::clone(&awakes),
            starts: Rc::clone(&starts),
            updates: Rc::clone(&updates),
            update_enabled,
        });
        (registry, awakes, starts, updates)
    }

    #[test]
    fn parses_tree_and_tracks_only_components_that_request_updates() {
        let (registry, awakes, starts, updates) = registry(true);
        let mut scene = Scene::from_yaml(
            r#"
deltaTime: 0.02
timeSinceStart: 3.5
root:
  name: Root
  components:
    - type: LifecycleProbe
  children:
    - name: Child
      components:
        - type: LifecycleProbe
"#,
            &registry,
        )
        .unwrap();
        assert_eq!(
            scene
                .root
                .borrow()
                .get_components::<LifecycleProbe>(&scene)
                .len(),
            1
        );
        assert!(
            scene
                .root
                .borrow()
                .get_component::<LifecycleProbe>(&scene)
                .is_some()
        );
        assert_eq!(
            scene.root.borrow().children()[0]
                .borrow()
                .get_components::<LifecycleProbe>(&scene)
                .len(),
            1
        );
        scene.initialize().unwrap();
        assert_eq!(awakes.get(), 2);
        assert_eq!(starts.get(), 2);
        assert_eq!(scene.update_component_count(), 2);
        scene.update(0.1, &mut Graphics::default()).unwrap();
        assert_eq!(updates.get(), 2);
        assert_eq!(scene.delta_time(), 0.1);
        assert!((scene.time_since_start() - 3.6).abs() < 0.0001);
    }

    #[test]
    fn non_updating_components_are_not_added_to_the_update_list() {
        let (registry, _, _, updates) = registry(false);
        let mut scene = Scene::from_yaml(
            "root:\n  name: Root\n  components:\n    - type: LifecycleProbe\n",
            &registry,
        )
        .unwrap();
        scene.initialize().unwrap();
        assert_eq!(scene.update_component_count(), 0);
        scene.update(0.016, &mut Graphics::default()).unwrap();
        assert_eq!(updates.get(), 0);
    }

    #[test]
    fn component_registry_generates_type_listing_and_custom_serde_hooks() {
        let (registry, _, _, _) = registry(true);
        let scene = Scene::from_yaml(
            "root:\n  name: Root\n  components:\n    - type: LifecycleProbe\n",
            &registry,
        )
        .unwrap();
        let info = registry
            .component_types()
            .unwrap()
            .into_iter()
            .find(|component| component.type_name == "LifecycleProbe")
            .unwrap();
        assert!(info.can_deserialize);
        assert!(info.can_serialize);
        let root = scene.root.borrow();
        let component = root
            .get_component::<LifecycleProbe>(&scene)
            .expect("component was created by its generated factory");
        assert_eq!(
            registry.serialize_component(&*component).unwrap(),
            serde_yaml::Value::Bool(true)
        );
    }

    #[test]
    fn typed_component_weak_handles_follow_component_lifetime() {
        let (registry, _, _, _) = registry(false);
        let mut scene = Scene::from_yaml(
            "root:\n  name: Root\n  components:\n    - type: Camera\n    - type: LifecycleProbe\n",
            &registry,
        )
        .unwrap();
        scene.initialize().unwrap();
        let camera = scene
            .root
            .borrow()
            .get_component_weak::<crate::ecs::Camera>()
            .expect("camera component exists");
        assert!(camera.is_alive());
        scene
            .remove_component::<crate::ecs::Camera>(&scene.root.borrow())
            .unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        assert!(!camera.is_alive());
    }

    #[test]
    fn deferred_mutations_allow_callbacks_to_change_other_objects_safely() {
        let registry = ComponentRegistry::new();
        let mut scene = Scene::from_yaml(
            "root:\n  name: Root\n  children:\n    - name: Target\n",
            &registry,
        )
        .unwrap();
        scene.initialize().unwrap();
        let target = GameObject::weak_reference(&scene.root.borrow().children()[0]);
        let outside = GameObject::empty("Outside");
        let outside_weak = GameObject::weak_reference(&outside);
        assert!(
            scene
                .defer_game_object_mutation(&outside_weak, |_| {})
                .is_err()
        );
        scene
            .defer_game_object_mutation(&target, |target| {
                target
                    .set_transform(Matrix4::from_translation(Vector3::new(1.0, 0.0, 0.0)))
                    .unwrap();
            })
            .unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        let target = target.upgrade().unwrap();
        assert_eq!(target.borrow().transform().w.x, 1.0);
    }

    #[test]
    fn weak_game_object_handles_expire_when_parent_removes_a_child() {
        let registry = ComponentRegistry::new();
        let mut scene = Scene::from_yaml(
            "root:\n  name: Root\n  children:\n    - name: Child\n",
            &registry,
        )
        .unwrap();
        scene.initialize().unwrap();
        let child = GameObject::weak_reference(&scene.root.borrow().children()[0]);
        let child_id = child.id();
        let root = GameObject::weak_reference(&scene.root);
        assert!(scene.resolve_game_object(&child).is_some());
        assert_eq!(scene.game_object_count(), 2);

        scene
            .defer_game_object_mutation(&root, move |root| {
                assert!(root.remove_child(child_id));
            })
            .unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();

        assert!(!child.is_alive());
        assert!(scene.resolve_game_object(&child).is_none());
        assert_eq!(scene.game_object_count(), 1);
    }

    #[test]
    fn rejects_mutations_for_objects_outside_the_scene() {
        let registry = ComponentRegistry::new();
        let scene = Scene::from_yaml("root:\n  name: Root\n", &registry).unwrap();
        assert!(
            scene
                .add_component(
                    &GameObject::empty("Outsider").borrow(),
                    Box::new(Noop {
                        instance: ComponentInstance::default(),
                    }),
                )
                .is_err()
        );
    }

    #[test]
    fn game_object_component_api_adds_components_with_lifecycle() {
        let (registry, awakes, starts, updates) = registry(true);
        let mut scene = Scene::from_yaml("root:\n  name: Root\n", &registry).unwrap();
        scene.initialize().unwrap();
        let root = scene.find_game_object(scene.root.borrow().id()).unwrap();
        root.borrow()
            .add_component(
                &scene,
                Box::new(LifecycleProbe {
                    instance: ComponentInstance::default(),
                    awakes: Rc::clone(&awakes),
                    starts: Rc::clone(&starts),
                    updates: Rc::clone(&updates),
                    update_enabled: true,
                }),
            )
            .unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        assert_eq!(awakes.get(), 1);
        assert_eq!(starts.get(), 1);
        assert_eq!(updates.get(), 0);
        scene.update(0.016, &mut Graphics::default()).unwrap();
        assert_eq!(updates.get(), 1);
    }

    #[test]
    fn removing_components_releases_owner_and_prunes_dead_scene_references() {
        let (registry, _, _, _) = registry(false);
        let mut scene = Scene::from_yaml(
            "root:\n  name: Root\n  components:\n    - type: LifecycleProbe\n  children:\n    - name: Child\n      components:\n        - type: LifecycleProbe\n",
            &registry,
        )
        .unwrap();
        scene.initialize().unwrap();
        let child_id = scene
            .root
            .borrow()
            .children()
            .first()
            .unwrap()
            .borrow()
            .id();
        scene
            .remove_component::<LifecycleProbe>(&scene.root.borrow())
            .unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        let child = scene.find_game_object(child_id).unwrap();
        assert_eq!(scene.component_count(), 1);
        assert_eq!(scene.root.borrow().components(), 0);
        assert_eq!(child.borrow().components(), 1);
        assert!(
            scene
                .root
                .borrow()
                .get_component::<LifecycleProbe>(&scene)
                .is_none()
        );
        assert!(
            child
                .borrow()
                .get_component::<LifecycleProbe>(&scene)
                .is_some()
        );
        scene.update(0.016, &mut Graphics::default()).unwrap();
        assert_eq!(scene.components.len(), 1);
    }

    #[test]
    fn lifecycle_callbacks_receive_the_owner_and_destroy_runs_once() {
        let calls = Rc::new(RefCell::new(Vec::<(&'static str, u64)>::new()));
        let mut registry = ComponentRegistry::new();
        registry.set_context(Rc::clone(&calls));
        let mut scene = Scene::from_yaml(
            "root:\n  name: Root\n  components:\n    - type: OwnerProbe\n",
            &registry,
        )
        .unwrap();
        let owner_id = scene.root.borrow().id();
        scene.initialize().unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        scene.destroy().unwrap();
        scene.destroy().unwrap();
        assert_eq!(
            calls.borrow().as_slice(),
            [
                ("awake", owner_id),
                ("start", owner_id),
                ("update", owner_id),
                ("destroy", owner_id)
            ]
        );
    }

    #[test]
    fn instantiated_game_objects_are_awakened_and_join_scene_component_updates() {
        let calls = Rc::new(RefCell::new(Vec::<(&'static str, u64)>::new()));
        let mut registry = ComponentRegistry::new();
        registry.set_context(Rc::clone(&calls));
        let mut scene = Scene::from_yaml("root:\n  name: Root\n", &registry).unwrap();
        scene.initialize().unwrap();

        let object = scene
            .instantiate_game_object(
                "name: Spawned\ncomponents:\n  - type: OwnerProbe\n",
                &registry,
            )
            .unwrap();
        let object_id = object.borrow().id();
        scene.root.borrow_mut().add_child(object.clone());
        assert_eq!(
            calls.borrow().as_slice(),
            [("awake", object_id), ("start", object_id)]
        );
        scene.update(0.016, &mut Graphics::default()).unwrap();
        scene.destroy().unwrap();
        assert_eq!(
            calls.borrow().as_slice(),
            [
                ("awake", object_id),
                ("start", object_id),
                ("update", object_id),
                ("destroy", object_id)
            ]
        );
    }

    #[test]
    fn rejects_missing_or_unknown_components() {
        let (registry, _, _, _) = registry(false);
        assert!(Scene::from_yaml("", &registry).is_err());
        assert!(
            Scene::from_yaml(
                "root:\n  name: Root\n  components:\n    - type: Unknown\n",
                &registry
            )
            .is_err()
        );
    }

    #[test]
    fn camera_components_are_registered_by_default() {
        let registry = ComponentRegistry::new();
        let scene = Scene::from_yaml(
            "root:\n  name: Root\n  components:\n    - type: Camera\n",
            &registry,
        )
        .unwrap();
        assert!(
            scene
                .root
                .borrow()
                .get_component::<crate::ecs::Camera>(&scene)
                .is_some()
        );
    }

    #[test]
    fn directional_light_is_auto_registered_with_unique_metadata() {
        let registry = ComponentRegistry::new();
        let scene = Scene::from_yaml(
            "root:\n  name: Root\n  components:\n    - type: DirectionalLight\n      data:\n        direction: [0.0, -1.0, 0.0]\n        color: [0.8, 0.9, 1.0]\n        intensity: 2.0\n",
            &registry,
        )
        .unwrap();
        let root = scene.root.borrow();
        let light = root
            .get_component::<crate::ecs::DirectionalLight>(&scene)
            .expect("generated component factory creates directional lights");
        assert_eq!(light.intensity, 2.0);
        assert!(light.uniform_values().is_ok());
        let metadata = registry
            .component_types()
            .unwrap()
            .into_iter()
            .find(|component| component.type_name == "DirectionalLight")
            .unwrap();
        assert!(metadata.unique);
    }

    #[test]
    fn runtime_scene_loading_does_not_validate_editor_component_rules() {
        let registry = ComponentRegistry::new();
        let scene = Scene::from_yaml(
            "root:\n  name: Root\n  components:\n    - type: Camera\n    - type: Camera\n",
            &registry,
        )
        .unwrap();
        assert_eq!(scene.root.borrow().components.len(), 2);
    }

    #[crate::ecs::component]
    #[derive(crate::ecs::Component)]
    struct Noop {}

    impl Component for Noop {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }
}
