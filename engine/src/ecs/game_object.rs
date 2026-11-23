use crate::ecs::component::{ComponentWeak, Unique, UniqueRc};
use crate::ecs::{Component, Scene, TransformDefinition};
use crate::graphics::Transform;
use anyhow::Result;
use cgmath::{Matrix4, SquareMatrix, Vector3};
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
    pub references: crate::assets::AssetReferences,
    transform: Cell<TransformDefinition>,
    parent_transform: Cell<Transform>,
    world_transform: Cell<Transform>,
    children: Vec<Unique<GameObject>>,
    pub(crate) components: Vec<UniqueRc<dyn Component>>,
}

impl GameObject {
    pub(crate) fn new(
        name: String,
        transform: TransformDefinition,
        children: Vec<Unique<GameObject>>,
    ) -> Unique<GameObject> {
        assert!(
            !name.trim().is_empty(),
            "game object names must not be empty"
        );
        Rc::new(RefCell::new(Self {
            id: NEXT_GAME_OBJECT_ID.fetch_add(1, Ordering::Relaxed),
            name,
            references: Default::default(),
            transform: Cell::new(transform),
            parent_transform: Cell::new(Matrix4::identity()),
            world_transform: Cell::new(
                transform.to_transform().expect("validated local transform"),
            ),
            children,
            components: Vec::new(),
        }))
    }

    pub fn empty(name: impl Into<String>) -> Unique<GameObject> {
        Self::new(name.into(), TransformDefinition::default(), Vec::new())
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

    pub fn set_transform(&self, transform: TransformDefinition) -> Result<()> {
        transform.to_transform()?;
        self.transform.set(transform);
        self.update_world_transforms(self.parent_transform.get());
        Ok(())
    }

    pub fn set_world_translation(&self, translation: Vector3<f32>) -> Result<()> {
        let parent_inverse = self
            .parent_transform
            .get()
            .invert()
            .ok_or_else(|| anyhow::anyhow!("parent transform is not invertible"))?;
        let local = parent_inverse * translation.extend(1.0);
        let mut transform = self.transform();
        transform.translation = local.truncate();
        self.set_transform(transform)
    }

    pub fn world_transform(&self) -> Transform {
        self.world_transform.get()
    }

    pub fn transform(&self) -> TransformDefinition {
        self.transform.get()
    }

    pub fn add_component(&self, scene: &Scene, component: Box<dyn Component>) -> Result<()> {
        scene.add_component(self, component)
    }

    pub fn remove_component<T: Component + 'static>(&self, scene: &Scene) -> Result<()> {
        scene.remove_component::<T>(self)
    }

    pub fn add_child(&mut self, child: Unique<GameObject>) {
        child
            .borrow()
            .update_world_transforms(self.world_transform.get());
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
        self.parent_transform.set(parent_transform);
        let world_transform = parent_transform
            * self
                .transform
                .get()
                .to_transform()
                .expect("validated local transform");
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

#[cfg(test)]
mod tests {
    use super::*;
    use cgmath::{Quaternion, Rad, Rotation3};

    #[test]
    fn local_trs_stays_editable_and_updates_hierarchy() {
        let parent = GameObject::empty("Parent");
        let child = GameObject::empty("Child");
        parent.borrow_mut().add_child(Rc::clone(&child));
        parent
            .borrow()
            .set_transform(TransformDefinition {
                translation: Vector3::new(4.0, 0.0, 0.0),
                rotation: Quaternion::from_angle_y(Rad(std::f32::consts::FRAC_PI_2)),
                ..Default::default()
            })
            .unwrap();
        let mut local = child.borrow().transform();
        local.translation = Vector3::new(0.0, 0.0, 2.0);
        child.borrow().set_transform(local).unwrap();
        assert!((child.borrow().world_transform().w.x - 6.0).abs() < 1.0e-5);
        child
            .borrow()
            .set_world_translation(Vector3::new(4.0, 0.0, -3.0))
            .unwrap();
        assert!((child.borrow().transform().translation.x - 3.0).abs() < 1.0e-5);
        let previous = child.borrow().transform();
        let mut invalid = previous;
        invalid.scale.x = 0.0;
        assert!(child.borrow().set_transform(invalid).is_err());
        assert_eq!(child.borrow().transform(), previous);
    }
}
