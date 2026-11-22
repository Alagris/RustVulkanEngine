use crate::ecs::{GameObject, Scene};
use crate::graphics::Graphics;
use anyhow::{Result, anyhow};
use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::rc::{Rc, Weak};
use std::sync::Arc;

#[derive(Clone)]
pub struct ComponentInstance {
    pub(crate) owner_id: u64,
    pub(crate) awoken: bool,
    pub(crate) started: bool,
    pub(crate) updates: bool,
}

pub trait ComponentMetadata {
    const TYPE_NAME: &'static str;
    const REQUIRED_COMPONENTS: &'static [&'static str];
    const UNIQUE: bool;
}

pub struct ComponentRuleRegistration {
    pub type_name: &'static str,
    pub required_components: &'static [&'static str],
    pub unique: bool,
}

inventory::collect!(ComponentRuleRegistration);
pub use inventory;

impl Default for ComponentInstance {
    fn default() -> Self {
        Self {
            owner_id: 0,
            awoken: false,
            started: false,
            updates: false,
        }
    }
}

impl fmt::Debug for ComponentInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComponentInstance")
            .field("owner_id", &self.owner_id)
            .field("awoken", &self.awoken)
            .field("started", &self.started)
            .field("updates", &self.updates)
            .finish()
    }
}

pub(crate) type UniqueRc<T> = Rc<RefCell<T>>;

pub struct ComponentWeak<T: Component + 'static> {
    component: Weak<RefCell<dyn Component>>,
    marker: PhantomData<fn() -> T>,
}

impl<T: Component + 'static> ComponentWeak<T> {
    pub(crate) fn new(component: Weak<RefCell<dyn Component>>) -> Self {
        Self {
            component,
            marker: PhantomData,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.component.upgrade().is_some()
    }

    pub fn with<R>(&self, action: impl FnOnce(&T) -> R) -> Option<R> {
        let component = self.component.upgrade()?;
        let borrowed = component.borrow();
        let borrowed =
            Ref::filter_map(borrowed, |component| component.as_any().downcast_ref::<T>()).ok()?;
        Some(action(&borrowed))
    }

    pub fn with_mut<R>(&self, action: impl FnOnce(&mut T) -> R) -> Option<R> {
        let component = self.component.upgrade()?;
        let borrowed = component.borrow_mut();
        let mut borrowed = RefMut::filter_map(borrowed, |component| {
            component.as_any_mut().downcast_mut::<T>()
        })
        .ok()?;
        Some(action(&mut borrowed))
    }
}

impl<T: Component + 'static> Clone for ComponentWeak<T> {
    fn clone(&self) -> Self {
        Self {
            component: self.component.clone(),
            marker: PhantomData,
        }
    }
}

pub(crate) fn component_ref(component: Box<dyn Component>) -> UniqueRc<dyn Component> {
    let component: UniqueRc<dyn Component> = Rc::new(RefCell::new(ComponentSlot(component)));
    component
}

struct ComponentSlot(Box<dyn Component>);

impl Deref for ComponentSlot {
    type Target = ComponentInstance;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl DerefMut for ComponentSlot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.deref_mut()
    }
}

impl Component for ComponentSlot {
    fn awake(&mut self, scene: &Scene, game_object: &GameObject) {
        self.0.awake(scene, game_object);
    }

    fn start(&mut self, scene: &Scene, game_object: &GameObject) -> bool {
        self.0.start(scene, game_object)
    }

    fn update(
        &mut self,
        scene: &Scene,
        game_object: &GameObject,
        graphics: &mut Graphics,
    ) -> Result<()> {
        self.0.update(scene, game_object, graphics)
    }

    fn destroy(&mut self, scene: &Scene, game_object: &GameObject) -> Result<()> {
        self.0.destroy(scene, game_object)
    }

    fn as_any(&self) -> &dyn Any {
        self.0.as_any()
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self.0.as_any_mut()
    }
}

pub trait Component:
    Any + Deref<Target = ComponentInstance> + DerefMut<Target = ComponentInstance>
{
    fn awake(&mut self, _scene: &Scene, _game_object: &GameObject) {}
    fn start(&mut self, _scene: &Scene, _game_object: &GameObject) -> bool {
        false
    }
    fn update(
        &mut self,
        _scene: &Scene,
        _game_object: &GameObject,
        _graphics: &mut Graphics,
    ) -> Result<()> {
        Ok(())
    }
    fn destroy(&mut self, _scene: &Scene, _game_object: &GameObject) -> Result<()> {
        Ok(())
    }
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

type ComponentFactory = Arc<dyn Fn(&serde_yaml::Value) -> Result<Box<dyn Component>>>;

pub struct ComponentRegistry {
    factories: HashMap<String, ComponentFactory>,
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        let mut registry = Self {
            factories: HashMap::new(),
        };
        registry
            .register_typed::<crate::ecs::Camera>("Camera", |data| {
                serde_yaml::from_value(data.clone())
                    .map_err(|error| anyhow!("invalid Camera component data: {error}"))
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
        factory: impl Fn(&serde_yaml::Value) -> Result<Box<dyn Component>> + 'static,
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

    pub fn register_typed<T: Component + 'static>(
        &mut self,
        type_name: impl Into<String>,
        factory: impl Fn(&serde_yaml::Value) -> Result<T> + 'static,
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
        self.factories.insert(
            type_name,
            Arc::new(move |data| {
                factory(data).map(|component| Box::new(component) as Box<dyn Component>)
            }),
        );
        Ok(())
    }

    pub(crate) fn create(
        &self,
        type_name: &str,
        data: &serde_yaml::Value,
    ) -> Result<Box<dyn Component>> {
        let factory = self
            .factories
            .get(type_name)
            .ok_or_else(|| anyhow!("unregistered component type '{}'", type_name))?;
        factory(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[crate::ecs::component]
    #[derive(crate::ecs::Component)]
    #[require(Camera, crate::ecs::Mesh)]
    struct NeedsCameraAndMesh {}

    #[test]
    fn component_rules_are_compile_time_metadata() {
        let _component = NeedsCameraAndMesh {
            instance: ComponentInstance::default(),
        };
        assert_eq!(NeedsCameraAndMesh::TYPE_NAME, "NeedsCameraAndMesh");
        assert_eq!(
            NeedsCameraAndMesh::REQUIRED_COMPONENTS,
            &["Camera", "crate :: ecs :: Mesh"]
        );
        assert!(!NeedsCameraAndMesh::UNIQUE);
    }
}
