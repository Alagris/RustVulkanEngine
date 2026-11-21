use crate::ecs::component::ComponentRegistry;
use crate::ecs::game_object::{GameObject, TransformDefinition};
use crate::graphics::Graphics;
use anyhow::{Context, Result, anyhow, bail};
use glam::Mat4;
use serde::Deserialize;
use std::cell::{Cell, Ref};
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
    data: toml::Value,
}

fn empty_component_data() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

pub struct Scene {
    pub root: GameObject,
    delta_time: Cell<f32>,
    time_since_start: Cell<f32>,
    update_objects: Vec<Vec<usize>>,
    initialized: Cell<bool>,
}

impl Scene {
    pub fn load(path: &Path, registry: &ComponentRegistry) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("Failed to read scene {}", path.display()))?;
        Self::from_toml(&source, registry)
            .with_context(|| format!("Invalid scene {}", path.display()))
    }

    pub fn from_toml(source: &str, registry: &ComponentRegistry) -> Result<Self> {
        let definition: SceneDefinition =
            toml::from_str(source).context("Failed to parse scene TOML")?;
        validate_timing(definition.delta_time, definition.time_since_start)?;
        Ok(Self {
            root: build_game_object(definition.root, registry)?,
            delta_time: Cell::new(definition.delta_time),
            time_since_start: Cell::new(definition.time_since_start),
            update_objects: Vec::new(),
            initialized: Cell::new(false),
        })
    }

    pub fn initialize(&mut self) {
        assert!(
            !self.initialized.get(),
            "scene lifecycle must be initialized once"
        );
        self.root.awake(self);
        let mut update_objects = Vec::new();
        self.root.start(self, &mut Vec::new(), &mut update_objects);
        self.update_objects = update_objects;
        self.initialized.set(true);
    }

    pub fn update(&self, delta_time: f32) -> Result<()> {
        assert!(
            self.initialized.get(),
            "scene must be initialized before updating"
        );
        if !delta_time.is_finite() || delta_time < 0.0 {
            return Err(anyhow!("frame delta time must be finite and non-negative"));
        }
        self.delta_time.set(delta_time);
        let time_since_start = self.time_since_start.get() + delta_time;
        if !time_since_start.is_finite() {
            return Err(anyhow!("scene time overflowed its finite range"));
        }
        self.time_since_start.set(time_since_start);
        for path in &self.update_objects {
            self.root.at_path(path).update(self);
        }
        Ok(())
    }

    pub fn delta_time(&self) -> f32 {
        self.delta_time.get()
    }

    pub fn time_since_start(&self) -> f32 {
        self.time_since_start.get()
    }

    pub fn update_object_count(&self) -> usize {
        self.update_objects.len()
    }

    pub fn render(&self, graphics: &mut Graphics) -> Result<()> {
        assert!(
            self.initialized.get(),
            "scene must be initialized before rendering"
        );
        self.root.render(self, Mat4::IDENTITY, graphics)
    }

    pub fn find_component<T: crate::ecs::Component + 'static>(&self) -> Option<Ref<'_, T>> {
        find_component(&self.root)
    }
}

pub struct Prefab {
    pub root: GameObject,
}

impl Prefab {
    pub fn load(path: &Path, registry: &ComponentRegistry) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("Failed to read prefab {}", path.display()))?;
        Self::from_toml(&source, registry)
            .with_context(|| format!("Invalid prefab {}", path.display()))
    }

    pub fn from_toml(source: &str, registry: &ComponentRegistry) -> Result<Self> {
        let definition: SceneDefinition =
            toml::from_str(source).context("Failed to parse prefab TOML")?;
        validate_timing(definition.delta_time, definition.time_since_start)?;
        Ok(Self {
            root: build_game_object(definition.root, registry)?,
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
) -> Result<GameObject> {
    if definition.name.trim().is_empty() {
        bail!("game object names must not be empty");
    }
    let components = definition
        .components
        .into_iter()
        .map(|component| registry.create(&component.type_name, &component.data))
        .collect::<Result<Vec<_>>>()?;
    let children = definition
        .children
        .into_iter()
        .map(|child| build_game_object(child, registry))
        .collect::<Result<Vec<_>>>()?;
    Ok(GameObject::new(
        definition.name,
        definition.transform.to_transform()?,
        components,
        children,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::Component;
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
        fn awake(&mut self, _scene: &Scene) {
            self.awakes.set(self.awakes.get() + 1);
        }
        fn start(&mut self, _scene: &Scene) -> bool {
            self.starts.set(self.starts.get() + 1);
            self.update_enabled
        }
        fn update(&mut self, _scene: &Scene) {
            self.updates.set(self.updates.get() + 1);
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
        let values = (awakes.clone(), starts.clone(), updates.clone());
        let mut registry = ComponentRegistry::new();
        registry
            .register("Probe", move |_| {
                Ok(Box::new(LifecycleProbe {
                    awakes: awakes.clone(),
                    starts: starts.clone(),
                    updates: updates.clone(),
                    update_enabled,
                }))
            })
            .unwrap();
        (registry, values.0, values.1, values.2)
    }

    #[test]
    fn parses_scene_tree_and_tracks_only_components_that_request_updates() {
        let (registry, awakes, starts, updates) = registry(true);
        let mut scene = Scene::from_toml(
            r#"
deltaTime = 0.02
timeSinceStart = 3.5
[root]
name = "Root"
[[root.components]]
type = "Probe"
[[root.children]]
name = "Child"
[[root.children.components]]
type = "Probe"
"#,
            &registry,
        )
        .unwrap();
        assert_eq!(scene.root.get_components::<LifecycleProbe>().len(), 1);
        assert!(scene.root.get_component::<LifecycleProbe>().is_some());
        assert_eq!(
            scene.root.children[0]
                .get_components::<LifecycleProbe>()
                .len(),
            1
        );
        scene.initialize();
        assert_eq!(awakes.get(), 2);
        assert_eq!(starts.get(), 2);
        assert_eq!(scene.update_object_count(), 2);
        scene.update(0.1).unwrap();
        assert_eq!(updates.get(), 2);
        assert_eq!(scene.delta_time(), 0.1);
        assert!((scene.time_since_start() - 3.6).abs() < 0.0001);
    }

    #[test]
    fn non_updating_components_are_not_added_to_the_update_list() {
        let (registry, _awakes, _starts, updates) = registry(false);
        let mut scene = Scene::from_toml(
            "[root]\nname = \"Root\"\n[[root.components]]\ntype = \"Probe\"\n",
            &registry,
        )
        .unwrap();
        scene.initialize();
        assert_eq!(scene.update_object_count(), 0);
        scene.update(0.016).unwrap();
        assert_eq!(updates.get(), 0);
    }

    #[test]
    fn prefab_uses_the_same_root_object_toml_shape() {
        let (registry, _, _, _) = registry(false);
        let prefab = Prefab::from_toml("[root]\nname = \"Prefab\"\n", &registry).unwrap();
        assert_eq!(prefab.root.name, "Prefab");
    }

    #[test]
    fn rejects_missing_or_unknown_components() {
        let (registry, _, _, _) = registry(false);
        assert!(Scene::from_toml("", &registry).is_err());
        assert!(
            Scene::from_toml(
                "[root]\nname = \"Root\"\n[[root.components]]\ntype = \"Unknown\"\n",
                &registry
            )
            .is_err()
        );
    }

    #[test]
    fn camera_components_are_registered_by_default() {
        let registry = ComponentRegistry::new();
        let scene = Scene::from_toml(
            "[root]\nname = \"Root\"\n[[root.components]]\ntype = \"Camera\"\n",
            &registry,
        )
        .unwrap();
        assert!(scene.root.get_component::<crate::ecs::Camera>().is_some());
    }
}

fn find_component<T: crate::ecs::Component + 'static>(object: &GameObject) -> Option<Ref<'_, T>> {
    object
        .get_component::<T>()
        .or_else(|| object.children.iter().find_map(find_component::<T>))
}
