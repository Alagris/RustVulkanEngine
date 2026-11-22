use crate::ecs::{GameObject, Scene};
use crate::graphics::Graphics;
use anyhow::{Result, anyhow};
use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::rc::{Rc, Weak};

#[derive(Clone)]
pub struct ComponentInstance {
    pub(crate) owner_id: u64,
    pub(crate) owner: Weak<RefCell<GameObject>>,
    pub(crate) awoken: bool,
    pub(crate) started: bool,
    component: Option<Weak<RefCell<dyn Component>>>,
}

bitflags::bitflags! {
    /// Lifecycle callbacks requested by a component at startup.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Features: u8 {
        const UPDATE = 1 << 0;
        const PHYSICS = 1 << 1;
        const LATE = 1 << 2;
    }
}

pub const UPDATE: Features = Features::UPDATE;
pub const PHYSICS: Features = Features::PHYSICS;
pub const LATE: Features = Features::LATE;

pub trait ComponentMetadata {
    const TYPE_NAME: &'static str;
    const REQUIRED_COMPONENTS: &'static [&'static str];
    const UNIQUE: bool;
}

pub type ComponentFactory = fn(&serde_yaml::Value, Option<&dyn Any>) -> Result<Box<dyn Component>>;
pub type ComponentSerializer = fn(&dyn Component) -> Result<serde_yaml::Value>;
pub type Unique<T> = Rc<RefCell<T>>;

pub struct ComponentRegistration {
    pub type_name: &'static str,
    pub required_components: &'static [&'static str],
    pub unique: bool,
    pub factory: Option<ComponentFactory>,
    pub serializer: Option<ComponentSerializer>,
    pub is_type: fn(&dyn Component) -> bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentTypeInfo {
    pub type_name: &'static str,
    pub required_components: &'static [&'static str],
    pub unique: bool,
    pub can_deserialize: bool,
    pub can_serialize: bool,
}

inventory::collect!(ComponentRegistration);
pub use anyhow as anyhow_crate;
pub use inventory;
pub use serde_yaml;

impl Default for ComponentInstance {
    fn default() -> Self {
        Self {
            owner_id: 0,
            owner: Weak::new(),
            awoken: false,
            started: false,
            component: None,
        }
    }
}

impl ComponentInstance {
    pub fn weak_component<T: Component + 'static>(&self) -> Option<ComponentWeak<T>> {
        self.component
            .as_ref()
            .map(|component| ComponentWeak::new(component.clone()))
    }
}

impl fmt::Debug for ComponentInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComponentInstance")
            .field("owner_id", &self.owner_id)
            .field("awoken", &self.awoken)
            .field("started", &self.started)
            .finish()
    }
}

pub(crate) type UniqueRc<T> = Unique<T>;

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

    pub(crate) fn downgrade(&self) -> Weak<RefCell<dyn Component>> {
        self.component.clone()
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
    component.borrow_mut().component = Some(Rc::downgrade(&component));
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

    fn try_start(&mut self, scene: &Scene, game_object: &GameObject) -> Result<Features> {
        self.0.try_start(scene, game_object)
    }

    fn sync_physics(&mut self, scene: &Scene, game_object: &GameObject) -> Result<()> {
        self.0.sync_physics(scene, game_object)
    }

    fn late_update(&mut self, scene: &Scene, game_object: &GameObject) -> Result<()> {
        self.0.late_update(scene, game_object)
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
    fn try_start(&mut self, _scene: &Scene, _game_object: &GameObject) -> Result<Features> {
        Ok(Features::default())
    }
    fn sync_physics(&mut self, _scene: &Scene, _game_object: &GameObject) -> Result<()> {
        Ok(())
    }
    fn late_update(&mut self, _scene: &Scene, _game_object: &GameObject) -> Result<()> {
        Ok(())
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

pub struct ComponentRegistry {
    context: Option<Rc<dyn Any>>,
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        Self { context: None }
    }
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_context<T: Any>(&mut self, context: T) {
        self.context = Some(Rc::new(context));
    }

    pub fn component_types(&self) -> Result<Vec<ComponentTypeInfo>> {
        let mut registrations = inventory::iter::<ComponentRegistration>
            .into_iter()
            .collect::<Vec<_>>();
        registrations.sort_by_key(|registration| registration.type_name);
        for pair in registrations.windows(2) {
            if pair[0].type_name == pair[1].type_name {
                return Err(anyhow!(
                    "component type '{}' was registered more than once",
                    pair[0].type_name
                ));
            }
        }
        Ok(registrations
            .into_iter()
            .map(|registration| ComponentTypeInfo {
                type_name: registration.type_name,
                required_components: registration.required_components,
                unique: registration.unique,
                can_deserialize: registration.factory.is_some(),
                can_serialize: registration.serializer.is_some(),
            })
            .collect())
    }

    pub(crate) fn create(
        &self,
        type_name: &str,
        data: &serde_yaml::Value,
    ) -> Result<Box<dyn Component>> {
        let mut registrations = inventory::iter::<ComponentRegistration>
            .into_iter()
            .filter(|registration| registration.type_name == type_name);
        let registration = registrations
            .next()
            .ok_or_else(|| anyhow!("unregistered component type '{}'", type_name))?;
        if registrations.next().is_some() {
            return Err(anyhow!(
                "component type '{}' was registered more than once",
                type_name
            ));
        }
        let factory = registration.factory.ok_or_else(|| {
            anyhow!(
                "component type '{}' has no deserializer; add `Deserialize` or a custom component deserializer",
                type_name
            )
        })?;
        factory(data, self.context.as_deref())
    }

    pub fn serialize_component(&self, component: &dyn Component) -> Result<serde_yaml::Value> {
        let mut registrations = inventory::iter::<ComponentRegistration>
            .into_iter()
            .filter(|registration| (registration.is_type)(component));
        let registration = registrations
            .next()
            .ok_or_else(|| anyhow!("component type has no generated registration"))?;
        if registrations.next().is_some() {
            return Err(anyhow!(
                "component type '{}' was registered more than once",
                registration.type_name
            ));
        }
        let serializer = registration.serializer.ok_or_else(|| {
            anyhow!(
                "component type '{}' has no serializer; add `Serialize` or a custom component serializer",
                registration.type_name
            )
        })?;
        serializer(component)
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
