use crate::ecs::Scene;
use crate::graphics::{Graphics, Transform};
use anyhow::{Result, anyhow};
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

pub trait Component: Any {
    fn awake(&mut self, _scene: &Scene) {}
    fn start(&mut self, _scene: &Scene) -> bool {
        false
    }
    fn update(&mut self, _scene: &Scene) {}
    fn render(
        &mut self,
        _scene: &Scene,
        _world_transform: Transform,
        _graphics: &mut Graphics,
    ) -> Result<()> {
        Ok(())
    }
    fn as_any(&self) -> &dyn Any;
}

type ComponentFactory = Arc<dyn Fn(&toml::Value) -> Result<Box<dyn Component>>>;

pub struct ComponentRegistry {
    factories: HashMap<String, ComponentFactory>,
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        let mut registry = Self {
            factories: HashMap::new(),
        };
        registry
            .register("Camera", |data| {
                let camera: crate::ecs::Camera = data
                    .clone()
                    .try_into()
                    .map_err(|error| anyhow!("invalid Camera component data: {error}"))?;
                Ok(Box::new(camera))
            })
            .expect("built-in Camera component type is unique");
        registry
    }
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        type_name: impl Into<String>,
        factory: impl Fn(&toml::Value) -> Result<Box<dyn Component>> + 'static,
    ) -> Result<()> {
        let type_name = type_name.into();
        if type_name.trim().is_empty() {
            return Err(anyhow!("component type names must not be empty"));
        }
        if self.factories.contains_key(&type_name) {
            return Err(anyhow!(
                "component factory for '{}' was registered more than once",
                type_name
            ));
        }
        self.factories.insert(type_name, Arc::new(factory));
        Ok(())
    }

    pub(crate) fn create(&self, type_name: &str, data: &toml::Value) -> Result<Box<dyn Component>> {
        let factory = self
            .factories
            .get(type_name)
            .ok_or_else(|| anyhow!("unregistered component type '{}'", type_name))?;
        factory(data)
    }
}
