use crate::ecs::component::{ComponentWeak, Unique, UniqueRc};
use crate::ecs::game_object_serde::is_finite_transform;
use crate::ecs::{Component, Scene};
use crate::graphics::Transform;
use anyhow::{Result, bail};
use cgmath::{Matrix4, SquareMatrix};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone)]
pub struct GameObjectWeak {
    id: u64,
    object: Weak<RefCell<GameObject>>,
}

impl GameObjectWeak {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn is_alive(&self) -> bool {
        self.object.upgrade().is_some()
    }

    pub fn upgrade(&self) -> Option<Unique<GameObject>> {
        self.object.upgrade()
    }
}

static NEXT_GAME_OBJECT_ID: AtomicU64 = AtomicU64::new(1);

pub struct GameObject {
    id: u64,
    pub name: String,
    transform: Cell<Transform>,
    world_transform: Cell<Transform>,
    children: Vec<Unique<GameObject>>,
    pub(crate) components: Vec<UniqueRc<dyn Component>>,
}

impl GameObject {
    pub(crate) fn new(
        name: String,
        transform: Transform,
        children: Vec<Unique<GameObject>>,
    ) -> Unique<GameObject> {
        assert!(
            !name.trim().is_empty(),
            "game object names must not be empty"
        );
        Rc::new(RefCell::new(Self {
            id: NEXT_GAME_OBJECT_ID.fetch_add(1, Ordering::Relaxed),
            name,
            transform: Cell::new(transform),
            world_transform: Cell::new(transform),
            children,
            components: Vec::new(),
        }))
    }

    pub fn empty(name: impl Into<String>) -> Unique<GameObject> {
        Self::new(name.into(), Matrix4::identity(), Vec::new())
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn weak_reference(object: &Unique<GameObject>) -> GameObjectWeak {
        GameObjectWeak {
            id: object.borrow().id,
            object: Rc::downgrade(object),
        }
    }

    pub fn set_transform(&self, transform: Transform) -> Result<()> {
        if !is_finite_transform(transform) || transform.determinant().abs() <= f32::EPSILON {
            bail!("game object transforms must be finite and invertible");
        }
        self.transform.set(transform);
        Ok(())
    }

    pub fn world_transform(&self) -> Transform {
        self.world_transform.get()
    }

    pub fn transform(&self) -> Transform {
        self.transform.get()
    }

    pub fn add_component(&self, scene: &Scene, component: Box<dyn Component>) -> Result<()> {
        scene.add_component(self, component)
    }

    pub fn remove_component<T: Component + 'static>(&self, scene: &Scene) -> Result<()> {
        scene.remove_component::<T>(self)
    }

    pub fn add_child(&mut self, child: Unique<GameObject>) {
        self.children.push(child);
    }

    pub fn remove_child(&mut self, child_id: u64) -> bool {
        let Some(index) = self
            .children
            .iter()
            .position(|child| child.borrow().id() == child_id)
        else {
            return false;
        };
        self.children.remove(index);
        true
    }

    pub fn children(&self) -> &[Unique<GameObject>] {
        &self.children
    }

    pub fn get_component<'a, T: Component + 'static>(
        &'a self,
        _scene: &'a Scene,
    ) -> Option<Ref<'a, T>> {
        self.components.iter().find_map(|component| {
            let borrowed = component.borrow();
            Ref::filter_map(borrowed, |instance| instance.as_any().downcast_ref::<T>()).ok()
        })
    }

    pub fn get_component_mut<'a, T: Component + 'static>(
        &'a self,
        _scene: &'a Scene,
    ) -> Option<RefMut<'a, T>> {
        self.components.iter().find_map(|component| {
            let borrowed = component.borrow_mut();
            RefMut::filter_map(borrowed, |instance| {
                instance.as_any_mut().downcast_mut::<T>()
            })
            .ok()
        })
    }

    pub fn get_component_weak<T: Component + 'static>(&self) -> Option<ComponentWeak<T>> {
        self.components.iter().find_map(|component| {
            let borrowed = component.try_borrow().ok()?;
            borrowed
                .as_any()
                .is::<T>()
                .then(|| ComponentWeak::new(std::rc::Rc::downgrade(component)))
        })
    }

    pub fn get_components<'a, T: Component + 'static>(
        &'a self,
        _scene: &'a Scene,
    ) -> Vec<Ref<'a, T>> {
        self.components
            .iter()
            .filter_map(|component| {
                let borrowed = component.borrow();
                Ref::filter_map(borrowed, |instance| instance.as_any().downcast_ref::<T>()).ok()
            })
            .collect()
    }

    pub fn components(&self) -> usize {
        self.components.len()
    }

    pub(crate) fn update_world_transforms(&self, parent_transform: Transform) {
        let world_transform = parent_transform * self.transform.get();
        self.world_transform.set(world_transform);
        for child in &self.children {
            child.borrow().update_world_transforms(world_transform);
        }
    }

    pub(crate) fn destroy_components(&self, scene: &Scene) -> Result<()> {
        let mut first_error = None;
        for child in self.children.iter().rev() {
            if let Err(error) = child.borrow().destroy_components(scene) {
                first_error.get_or_insert(error);
            }
        }
        for component in self.components.iter().rev() {
            let mut component = component.borrow_mut();
            if component.awoken {
                if let Err(error) = component.destroy(scene, self) {
                    first_error.get_or_insert(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub(crate) fn find_by_id(root: &Unique<GameObject>, id: u64) -> Option<Unique<GameObject>> {
        let object = root.borrow();
        if object.id == id {
            return Some(Rc::clone(root));
        }
        object
            .children
            .iter()
            .find_map(|child| Self::find_by_id(child, id))
    }

    pub(crate) fn contains(root: &Unique<GameObject>, target: &Unique<GameObject>) -> bool {
        if Rc::ptr_eq(root, target) {
            return true;
        }
        root.borrow()
            .children
            .iter()
            .any(|child| Self::contains(child, target))
    }
}
