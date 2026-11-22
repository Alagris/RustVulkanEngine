use crate::ecs::Component;
use crate::ecs::component::{ComponentRegistry, component_ref};
use crate::ecs::game_object::{GameObject, GameObjectWeak, TransformDefinition};
use crate::graphics::Graphics;
use anyhow::{Context, Result, anyhow, bail};
use cgmath::{Matrix4, SquareMatrix};
use serde::Deserialize;
use std::any::TypeId;
use std::cell::{Cell, Ref, RefCell};
use std::fs;
use std::path::Path;
use std::rc::{Rc, Weak};

#[derive(Deserialize)]
struct SceneDefinition {
    root: GameObjectDefinition,
    #[serde(default, rename = "deltaTime")]
    delta_time: f32,
    #[serde(default, rename = "timeSinceStart")]
    time_since_start: f32,
}

#[derive(Deserialize)]
struct GameObjectDefinition {
    name: String,
    #[serde(default)]
    transform: TransformDefinition,
    #[serde(default)]
    components: Vec<ComponentDefinition>,
    #[serde(default)]
    children: Vec<GameObjectDefinition>,
}

#[derive(Deserialize)]
struct ComponentDefinition {
    #[serde(rename = "type")]
    type_name: String,
    #[serde(default = "empty_component_data")]
    data: serde_yaml::Value,
}

fn empty_component_data() -> serde_yaml::Value {
    serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
}

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
        owner_id: u64,
        mutation: Box<dyn FnOnce(&mut GameObject)>,
    },
}

pub struct Scene {
    pub root: GameObject,
    pub(crate) components: Vec<Weak<RefCell<dyn Component>>>,
    pub(crate) game_objects: Vec<GameObjectWeak>,
    delta_time: Cell<f32>,
    time_since_start: Cell<f32>,
    initialized: Cell<bool>,
    destroyed: Cell<bool>,
    pending_mutations: RefCell<Vec<PendingMutation>>,
}

impl Scene {
    pub fn load(path: &Path, registry: &ComponentRegistry) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("Failed to read scene {}", path.display()))?;
        Self::from_yaml(&source, registry)
            .with_context(|| format!("Invalid scene {}", path.display()))
    }

    pub fn from_yaml(source: &str, registry: &ComponentRegistry) -> Result<Self> {
        let value: serde_yaml::Value =
            serde_yaml::from_str(source).context("Failed to parse scene YAML")?;
        registry.validate_component_constraints(&value)?;
        let definition: SceneDefinition =
            serde_yaml::from_value(value).context("Failed to parse scene YAML")?;
        validate_timing(definition.delta_time, definition.time_since_start)?;
        let mut components = Vec::new();
        let root = build_game_object(definition.root, registry, &mut components)?;
        let mut game_objects = Vec::new();
        root.collect_weak_references(&mut game_objects);
        Ok(Self {
            root,
            components,
            game_objects,
            delta_time: Cell::new(definition.delta_time),
            time_since_start: Cell::new(definition.time_since_start),
            initialized: Cell::new(false),
            destroyed: Cell::new(false),
            pending_mutations: RefCell::new(Vec::new()),
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
        let mut live_game_objects = 0;
        for read_index in 0..self.game_objects.len() {
            if self.game_objects[read_index].is_alive() {
                if live_game_objects != read_index {
                    self.game_objects[live_game_objects] = self.game_objects[read_index].clone();
                }
                live_game_objects += 1;
            }
        }
        self.game_objects.truncate(live_game_objects);
        self.root.update_world_transforms(Matrix4::identity());
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
                        let owner = self
                            .root
                            .find_by_id(component.owner_id)
                            .ok_or_else(|| anyhow!("component owner no longer exists"))?;
                        component.update(self, owner, graphics)?;
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
        object_id: u64,
        mutation: impl FnOnce(&mut GameObject) + 'static,
    ) -> Result<()> {
        if self.destroyed.get() {
            bail!("cannot mutate a destroyed scene");
        }
        if self.root.find_by_id(object_id).is_none() {
            bail!("game object {object_id} does not belong to this scene");
        }
        self.pending_mutations
            .borrow_mut()
            .push(PendingMutation::Mutate {
                owner_id: object_id,
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
        if self.root.find_by_id(owner.id()).is_none() {
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
        if self.root.find_by_id(owner.id()).is_none() {
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
                    PendingMutation::Mutate { owner_id, mutation } => {
                        let object = self.root.find_by_id_mut(owner_id).ok_or_else(|| {
                            anyhow!("deferred mutation target {owner_id} no longer exists")
                        })?;
                        mutation(object);
                    }
                }
            }
            self.game_objects.clear();
            self.root.collect_weak_references(&mut self.game_objects);
            self.initialize_components()?;
        }
    }

    fn append_component(
        &mut self,
        owner_id: u64,
        component: Box<dyn crate::ecs::Component>,
    ) -> Result<()> {
        let owner = self
            .root
            .find_by_id_mut(owner_id)
            .ok_or_else(|| anyhow!("component owner {owner_id} no longer exists"))?;
        let mut component = component;
        component.owner_id = owner_id;
        let component = component_ref(component);
        self.components.push(Rc::downgrade(&component));
        owner.components.push(component);
        Ok(())
    }

    fn remove_component_now(&mut self, owner_id: u64, type_id: TypeId) -> Result<()> {
        let owner = self
            .root
            .find_by_id(owner_id)
            .ok_or_else(|| anyhow!("component owner {owner_id} no longer exists"))?;
        let component_index = owner
            .components
            .iter()
            .position(|component| component.borrow().as_any().type_id() == type_id)
            .ok_or_else(|| anyhow!("requested component no longer exists on {owner_id}"))?;
        let component = owner.components[component_index].clone();
        {
            let mut component = component.borrow_mut();
            if component.awoken {
                component.destroy(self, owner)?;
            }
        }
        let owner = self
            .root
            .find_by_id_mut(owner_id)
            .ok_or_else(|| anyhow!("component owner {owner_id} no longer exists"))?;
        owner.components.remove(component_index);
        Ok(())
    }

    fn initialize_components(&mut self) -> Result<()> {
        for component_ref in self.components.clone() {
            let Some(component) = component_ref.upgrade() else {
                continue;
            };
            let mut component = component.borrow_mut();
            if component.awoken {
                continue;
            }
            let owner = self
                .root
                .find_by_id(component.owner_id)
                .ok_or_else(|| anyhow!("component owner no longer exists"))?;
            component.awoken = true;
            component.awake(self, owner);
            component.updates = component.start(self, owner);
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
        self.game_objects
            .iter()
            .filter(|object| object.is_alive())
            .count()
    }

    pub fn destroy(&self) -> Result<()> {
        assert!(
            self.initialized.get(),
            "scene must be initialized before destruction"
        );
        if self.destroyed.replace(true) {
            return Ok(());
        }
        self.root.destroy_components(self)
    }

    pub fn find_game_object(&self, id: u64) -> Option<&GameObject> {
        self.root.find_by_id(id)
    }

    pub fn resolve_game_object(&self, object: &GameObjectWeak) -> Option<&GameObject> {
        if object.is_alive() {
            self.find_game_object(object.id())
        } else {
            None
        }
    }

    pub fn find_component<T: crate::ecs::Component + 'static>(&self) -> Option<Ref<'_, T>> {
        find_component(self, &self.root)
    }
}

pub struct Prefab {
    pub root: GameObject,
}

impl Prefab {
    pub fn load(path: &Path, registry: &ComponentRegistry) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("Failed to read prefab {}", path.display()))?;
        Self::from_yaml(&source, registry)
            .with_context(|| format!("Invalid prefab {}", path.display()))
    }

    pub fn from_yaml(source: &str, registry: &ComponentRegistry) -> Result<Self> {
        let value: serde_yaml::Value =
            serde_yaml::from_str(source).context("Failed to parse prefab YAML")?;
        registry.validate_component_constraints(&value)?;
        let definition: SceneDefinition =
            serde_yaml::from_value(value).context("Failed to parse prefab YAML")?;
        validate_timing(definition.delta_time, definition.time_since_start)?;
        let mut components = Vec::new();
        let root = build_game_object(definition.root, registry, &mut components)?;
        Ok(Self { root })
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

fn build_game_object(
    definition: GameObjectDefinition,
    registry: &ComponentRegistry,
    components: &mut Vec<Weak<RefCell<dyn Component>>>,
) -> Result<GameObject> {
    if definition.name.trim().is_empty() {
        bail!("game object names must not be empty");
    }
    let mut object = GameObject::new(
        definition.name,
        definition.transform.to_transform()?,
        Vec::new(),
    );
    for component_definition in definition.components {
        let mut component =
            registry.create(&component_definition.type_name, &component_definition.data)?;
        component.owner_id = object.id();
        let component = component_ref(component);
        components.push(Rc::downgrade(&component));
        object.components.push(component);
    }
    for child in definition.children {
        object.add_child(build_game_object(child, registry, components)?);
    }
    Ok(object)
}

fn find_component<'a, T: crate::ecs::Component + 'static>(
    scene: &'a Scene,
    object: &'a GameObject,
) -> Option<Ref<'a, T>> {
    object.get_component::<T>(scene).or_else(|| {
        object
            .children()
            .iter()
            .find_map(|child| find_component(scene, child))
    })
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

    #[crate::ecs::component]
    #[derive(crate::ecs::Component)]
    struct LifecycleProbe {
        awakes: Rc<Cell<usize>>,
        starts: Rc<Cell<usize>>,
        updates: Rc<Cell<usize>>,
        update_enabled: bool,
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
        registry
            .register_with_metadata::<LifecycleProbe>("Probe", {
                let awakes = Rc::clone(&awakes);
                let starts = Rc::clone(&starts);
                let updates = Rc::clone(&updates);
                move |_| {
                    Ok(LifecycleProbe {
                        instance: ComponentInstance::default(),
                        awakes: Rc::clone(&awakes),
                        starts: Rc::clone(&starts),
                        updates: Rc::clone(&updates),
                        update_enabled,
                    })
                }
            })
            .unwrap();
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
    - type: Probe
  children:
    - name: Child
      components:
        - type: Probe
"#,
            &registry,
        )
        .unwrap();
        assert_eq!(scene.root.get_components::<LifecycleProbe>(&scene).len(), 1);
        assert!(scene.root.get_component::<LifecycleProbe>(&scene).is_some());
        assert_eq!(
            scene.root.children()[0]
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
            "root:\n  name: Root\n  components:\n    - type: Probe\n",
            &registry,
        )
        .unwrap();
        scene.initialize().unwrap();
        assert_eq!(scene.update_component_count(), 0);
        scene.update(0.016, &mut Graphics::default()).unwrap();
        assert_eq!(updates.get(), 0);
    }

    #[test]
    fn typed_component_weak_handles_follow_component_lifetime() {
        let (registry, _, _, _) = registry(false);
        let mut scene = Scene::from_yaml(
            "root:\n  name: Root\n  components:\n    - type: Camera\n    - type: Probe\n",
            &registry,
        )
        .unwrap();
        scene.initialize().unwrap();
        let camera = scene
            .root
            .get_component_weak::<crate::ecs::Camera>()
            .expect("camera component exists");
        assert!(camera.is_alive());
        scene
            .remove_component::<crate::ecs::Camera>(&scene.root)
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
        let target_id = scene.root.children()[0].id();
        scene
            .defer_game_object_mutation(target_id, |target| {
                target
                    .set_transform(Matrix4::from_translation(Vector3::new(1.0, 0.0, 0.0)))
                    .unwrap();
            })
            .unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        let target = scene.find_game_object(target_id).unwrap();
        assert_eq!(target.transform().w.x, 1.0);
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
        let child = scene.root.children()[0].weak_reference();
        let child_id = child.id();
        let root_id = scene.root.id();
        assert!(scene.resolve_game_object(&child).is_some());
        assert_eq!(scene.game_object_count(), 2);

        scene
            .defer_game_object_mutation(root_id, move |root| {
                assert!(root.remove_child(child_id));
            })
            .unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();

        assert!(!child.is_alive());
        assert!(scene.resolve_game_object(&child).is_none());
        assert_eq!(scene.game_object_count(), 1);
        assert_eq!(scene.game_objects.len(), 1);
    }

    #[test]
    fn rejects_mutations_for_objects_outside_the_scene() {
        let registry = ComponentRegistry::new();
        let scene = Scene::from_yaml("root:\n  name: Root\n", &registry).unwrap();
        assert!(
            scene
                .add_component(
                    &GameObject::empty("Outsider"),
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
        let root = scene.find_game_object(scene.root.id()).unwrap();
        root.add_component(
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
            "root:\n  name: Root\n  components:\n    - type: Probe\n  children:\n    - name: Child\n      components:\n        - type: Probe\n",
            &registry,
        )
        .unwrap();
        scene.initialize().unwrap();
        let child_id = scene.root.children().first().unwrap().id();
        scene
            .remove_component::<LifecycleProbe>(&scene.root)
            .unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        let child = scene.find_game_object(child_id).unwrap();
        assert_eq!(scene.component_count(), 1);
        assert_eq!(scene.root.components(), 0);
        assert_eq!(child.components(), 1);
        assert!(scene.root.get_component::<LifecycleProbe>(&scene).is_none());
        assert!(child.get_component::<LifecycleProbe>(&scene).is_some());
        scene.update(0.016, &mut Graphics::default()).unwrap();
        assert_eq!(scene.components.len(), 1);
    }

    #[test]
    fn lifecycle_callbacks_receive_the_owner_and_destroy_runs_once() {
        #[crate::ecs::component]
        #[derive(crate::ecs::Component)]
        struct OwnerProbe {
            calls: Rc<RefCell<Vec<(&'static str, u64)>>>,
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

        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut registry = ComponentRegistry::new();
        let factory_calls = Rc::clone(&calls);
        registry
            .register_with_metadata::<OwnerProbe>("OwnerProbe", move |_| {
                Ok(OwnerProbe {
                    instance: ComponentInstance::default(),
                    calls: Rc::clone(&factory_calls),
                })
            })
            .unwrap();
        let mut scene = Scene::from_yaml(
            "root:\n  name: Root\n  components:\n    - type: OwnerProbe\n",
            &registry,
        )
        .unwrap();
        let owner_id = scene.root.id();
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
    fn prefab_uses_the_same_root_object_yaml_shape() {
        let (registry, _, _, _) = registry(false);
        let prefab = Prefab::from_yaml("root:\n  name: Prefab\n", &registry).unwrap();
        assert_eq!(prefab.root.name, "Prefab");
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
                .get_component::<crate::ecs::Camera>(&scene)
                .is_some()
        );
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
