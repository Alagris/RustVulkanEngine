use crate::ecs::component::ComponentRegistry;
use crate::ecs::game_object::{ComponentInstance, GameObject, TransformDefinition};
use crate::graphics::Graphics;
use anyhow::{Context, Result, anyhow, bail};
use cgmath::{Matrix4, SquareMatrix};
use serde::Deserialize;
use std::cell::{Cell, Ref, RefCell};
use std::fs;
use std::path::Path;

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
        index: usize,
    },
    Mutate {
        owner_id: u64,
        mutation: Box<dyn FnOnce(&mut GameObject)>,
    },
}

pub struct Scene {
    pub root: GameObject,
    pub(crate) components: Vec<RefCell<ComponentInstance>>,
    delta_time: Cell<f32>,
    time_since_start: Cell<f32>,
    update_components: Vec<usize>,
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
            update_components: Vec::new(),
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
        self.initialize_components(0)?;
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
        self.root.update_world_transforms(Matrix4::identity());
        let update_indices = self.update_components.clone();
        for index in update_indices {
            let instance = self
                .components
                .get(index)
                .ok_or_else(|| anyhow!("scene update component index {index} is invalid"))?;
            let mut instance = instance.borrow_mut();
            assert_eq!(
                instance.idx, index,
                "component index must match its registry slot"
            );
            assert!(
                instance.started && instance.updates,
                "only started components that requested updates may update"
            );
            let owner = self
                .root
                .find_by_id(instance.owner_id)
                .ok_or_else(|| anyhow!("component owner no longer exists"))?;
            instance.component.update(self, owner, graphics)?;
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

    pub fn remove_component(&self, owner: &GameObject, index: usize) -> Result<()> {
        if self.destroyed.get() {
            bail!("cannot remove a component from a destroyed scene");
        }
        if self.root.find_by_id(owner.id()).is_none() {
            bail!("game object {} does not belong to this scene", owner.id());
        }
        let instance = self
            .components
            .get(index)
            .ok_or_else(|| anyhow!("component index {index} is outside the scene registry"))?
            .borrow();
        if instance.owner_id != owner.id() {
            bail!(
                "component index {index} is not owned by game object {}",
                owner.id()
            );
        }
        drop(instance);
        self.pending_mutations
            .borrow_mut()
            .push(PendingMutation::Remove {
                owner_id: owner.id(),
                index,
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
                    PendingMutation::Remove { owner_id, index } => {
                        self.remove_component_now(owner_id, index)?;
                    }
                    PendingMutation::Mutate { owner_id, mutation } => {
                        let object = self.root.find_by_id_mut(owner_id).ok_or_else(|| {
                            anyhow!("deferred mutation target {owner_id} no longer exists")
                        })?;
                        mutation(object);
                    }
                }
            }
            self.initialize_components(0)?;
        }
    }

    fn append_component(
        &mut self,
        owner_id: u64,
        component: Box<dyn crate::ecs::Component>,
    ) -> Result<()> {
        let owner = self
            .root
            .find_by_id(owner_id)
            .ok_or_else(|| anyhow!("component owner {owner_id} no longer exists"))?;
        let index = self.components.len();
        owner.component_indices.borrow_mut().push(index);
        self.components.push(RefCell::new(ComponentInstance {
            idx: index,
            owner_id,
            component,
            awoken: false,
            started: false,
            updates: false,
        }));
        Ok(())
    }

    fn remove_component_now(&mut self, owner_id: u64, index: usize) -> Result<()> {
        let instance = self
            .components
            .get(index)
            .ok_or_else(|| anyhow!("component index {index} no longer exists"))?;
        {
            let mut instance = instance.borrow_mut();
            if instance.owner_id != owner_id {
                bail!("component index {index} is no longer owned by game object {owner_id}");
            }
            if instance.awoken {
                let owner = self
                    .root
                    .find_by_id(owner_id)
                    .ok_or_else(|| anyhow!("component owner {owner_id} no longer exists"))?;
                instance.component.destroy(self, owner)?;
            }
        }

        let owner = self
            .root
            .find_by_id(owner_id)
            .ok_or_else(|| anyhow!("component owner {owner_id} no longer exists"))?;
        owner
            .component_indices
            .borrow_mut()
            .retain(|&value| value != index);
        self.update_components.retain(|&value| value != index);

        let last_index = self.components.len() - 1;
        self.components.swap_remove(index);
        if index != last_index {
            let moved = self
                .components
                .get(index)
                .expect("swap_remove must move the last component into the removed slot");
            let mut moved = moved.borrow_mut();
            let old_index = moved.idx;
            assert_eq!(
                old_index, last_index,
                "moved component must come from the last slot"
            );
            moved.idx = index;
            let moved_owner = self
                .root
                .find_by_id(moved.owner_id)
                .ok_or_else(|| anyhow!("moved component owner no longer exists"))?;
            let mut owner_indices = moved_owner.component_indices.borrow_mut();
            let owner_index = owner_indices
                .iter_mut()
                .find(|value| **value == old_index)
                .ok_or_else(|| anyhow!("moved component is missing from its owner's index list"))?;
            *owner_index = index;
            for update_index in &mut self.update_components {
                if *update_index == old_index {
                    *update_index = index;
                }
            }
        }
        Ok(())
    }

    fn initialize_components(&mut self, from_index: usize) -> Result<()> {
        for index in from_index..self.components.len() {
            let instance = self
                .components
                .get(index)
                .expect("component initialization range must be valid");
            let mut instance = instance.borrow_mut();
            if instance.awoken {
                continue;
            }
            assert_eq!(
                instance.idx, index,
                "component index must match its registry slot"
            );
            let owner = self
                .root
                .find_by_id(instance.owner_id)
                .ok_or_else(|| anyhow!("component owner no longer exists"))?;
            instance.awoken = true;
            instance.component.awake(self, owner);
            let updates = instance.component.start(self, owner);
            instance.started = true;
            instance.updates = updates;
            if updates {
                self.update_components.push(index);
            }
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
        self.update_components.len()
    }

    pub fn component_count(&self) -> usize {
        self.components.len()
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

    pub fn find_component<T: crate::ecs::Component + 'static>(&self) -> Option<Ref<'_, T>> {
        find_component(self, &self.root)
    }
}

pub struct Prefab {
    pub root: GameObject,
    _components: Vec<RefCell<ComponentInstance>>,
}

impl Prefab {
    pub fn load(path: &Path, registry: &ComponentRegistry) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("Failed to read prefab {}", path.display()))?;
        Self::from_yaml(&source, registry)
            .with_context(|| format!("Invalid prefab {}", path.display()))
    }

    pub fn from_yaml(source: &str, registry: &ComponentRegistry) -> Result<Self> {
        let definition: SceneDefinition =
            serde_yaml::from_str(source).context("Failed to parse prefab YAML")?;
        validate_timing(definition.delta_time, definition.time_since_start)?;
        let mut components = Vec::new();
        let root = build_game_object(definition.root, registry, &mut components)?;
        Ok(Self {
            root,
            _components: components,
        })
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
    components: &mut Vec<RefCell<ComponentInstance>>,
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
        let index = components.len();
        let component =
            registry.create(&component_definition.type_name, &component_definition.data)?;
        components.push(RefCell::new(ComponentInstance {
            idx: index,
            owner_id: object.id(),
            component,
            awoken: false,
            started: false,
            updates: false,
        }));
        object.component_indices.borrow_mut().push(index);
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
    use crate::ecs::{Component, ComponentRegistry};
    use anyhow::Result;
    use cgmath::Vector3;
    use std::any::Any;
    use std::cell::Cell;
    use std::rc::Rc;

    #[derive(Clone)]
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
            .register("Probe", {
                let awakes = Rc::clone(&awakes);
                let starts = Rc::clone(&starts);
                let updates = Rc::clone(&updates);
                move |_| {
                    Ok(Box::new(LifecycleProbe {
                        awakes: Rc::clone(&awakes),
                        starts: Rc::clone(&starts),
                        updates: Rc::clone(&updates),
                        update_enabled,
                    }))
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
    fn rejects_mutations_for_objects_outside_the_scene() {
        let registry = ComponentRegistry::new();
        let scene = Scene::from_yaml("root:\n  name: Root\n", &registry).unwrap();
        assert!(
            scene
                .add_component(&GameObject::empty("Outsider"), Box::new(Noop))
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
    fn swap_remove_repairs_global_and_owner_component_indices() {
        let (registry, _, _, _) = registry(false);
        let mut scene = Scene::from_yaml(
            "root:\n  name: Root\n  components:\n    - type: Probe\n  children:\n    - name: Child\n      components:\n        - type: Probe\n",
            &registry,
        )
        .unwrap();
        scene.initialize().unwrap();
        let child_id = scene.root.children().first().unwrap().id();
        let removed_index = scene.root.component_indices.borrow()[0];
        let moved_index = scene.root.children()[0].component_indices.borrow()[0];
        scene.remove_component(&scene.root, removed_index).unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        let child = scene.find_game_object(child_id).unwrap();
        assert_eq!(scene.component_count(), 1);
        assert_eq!(child.component_indices.borrow().as_slice(), &[0]);
        assert_eq!(scene.components[0].borrow().idx, 0);
        assert_eq!(scene.components[0].borrow().owner_id, child.id());
        assert_eq!(moved_index, 1);
        assert!(scene.root.get_component::<LifecycleProbe>(&scene).is_none());
        assert!(child.get_component::<LifecycleProbe>(&scene).is_some());
    }

    #[test]
    fn lifecycle_callbacks_receive_the_owner_and_destroy_runs_once() {
        struct OwnerProbe(Rc<RefCell<Vec<(&'static str, u64)>>>);

        impl Component for OwnerProbe {
            fn awake(&mut self, _scene: &Scene, game_object: &GameObject) {
                self.0.borrow_mut().push(("awake", game_object.id()));
            }
            fn start(&mut self, _scene: &Scene, game_object: &GameObject) -> bool {
                self.0.borrow_mut().push(("start", game_object.id()));
                true
            }
            fn update(
                &mut self,
                _scene: &Scene,
                game_object: &GameObject,
                _graphics: &mut Graphics,
            ) -> Result<()> {
                self.0.borrow_mut().push(("update", game_object.id()));
                Ok(())
            }
            fn destroy(&mut self, _scene: &Scene, game_object: &GameObject) -> Result<()> {
                self.0.borrow_mut().push(("destroy", game_object.id()));
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
            .register("OwnerProbe", move |_| {
                Ok(Box::new(OwnerProbe(Rc::clone(&factory_calls))))
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

    struct Noop;

    impl Component for Noop {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }
}
