use crate::ecs::{Component, Scene};
use crate::graphics::Transform;
use anyhow::{Result, bail};
use cgmath::{InnerSpace, Matrix4, One, Quaternion, SquareMatrix, Vector3};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) struct ComponentInstance {
    pub(crate) idx: usize,
    pub(crate) owner_id: u64,
    pub(crate) component: Box<dyn Component>,
    pub(crate) awoken: bool,
    pub(crate) started: bool,
    pub(crate) updates: bool,
}

static NEXT_GAME_OBJECT_ID: AtomicU64 = AtomicU64::new(1);

pub struct GameObject {
    id: u64,
    pub name: String,
    transform: Cell<Transform>,
    world_transform: Cell<Transform>,
    children: Vec<GameObject>,
    pub(crate) component_indices: RefCell<Vec<usize>>,
}

impl GameObject {
    pub(crate) fn new(name: String, transform: Transform, children: Vec<GameObject>) -> Self {
        assert!(
            !name.trim().is_empty(),
            "game object names must not be empty"
        );
        Self {
            id: NEXT_GAME_OBJECT_ID.fetch_add(1, Ordering::Relaxed),
            name,
            transform: Cell::new(transform),
            world_transform: Cell::new(transform),
            children,
            component_indices: RefCell::new(Vec::new()),
        }
    }

    pub fn empty(name: impl Into<String>) -> Self {
        Self::new(name.into(), Matrix4::identity(), Vec::new())
    }

    pub fn id(&self) -> u64 {
        self.id
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

    pub fn remove_component(&self, scene: &Scene, component_index: usize) -> Result<()> {
        scene.remove_component(self, component_index)
    }

    pub fn add_child(&mut self, child: GameObject) {
        self.children.push(child);
    }

    pub fn children(&self) -> &[GameObject] {
        &self.children
    }

    pub fn get_component<'a, T: Component + 'static>(
        &self,
        scene: &'a Scene,
    ) -> Option<Ref<'a, T>> {
        self.component_indices.borrow().iter().find_map(|&index| {
            let instance = scene.components.get(index)?;
            let borrowed = instance.borrow();
            Ref::filter_map(borrowed, |instance| {
                instance.component.as_any().downcast_ref::<T>()
            })
            .ok()
        })
    }

    pub fn get_component_mut<'a, T: Component + 'static>(
        &self,
        scene: &'a Scene,
    ) -> Option<RefMut<'a, T>> {
        self.component_indices.borrow().iter().find_map(|&index| {
            let instance = scene.components.get(index)?;
            let borrowed = instance.borrow_mut();
            RefMut::filter_map(borrowed, |instance| {
                instance.component.as_any_mut().downcast_mut::<T>()
            })
            .ok()
        })
    }

    pub fn get_components<'a, T: Component + 'static>(&self, scene: &'a Scene) -> Vec<Ref<'a, T>> {
        self.component_indices
            .borrow()
            .iter()
            .filter_map(|&index| {
                let instance = scene.components.get(index)?;
                let borrowed = instance.borrow();
                Ref::filter_map(borrowed, |instance| {
                    instance.component.as_any().downcast_ref::<T>()
                })
                .ok()
            })
            .collect()
    }

    pub fn components(&self) -> usize {
        self.component_indices.borrow().len()
    }

    pub(crate) fn update_world_transforms(&self, parent_transform: Transform) {
        let world_transform = parent_transform * self.transform.get();
        self.world_transform.set(world_transform);
        for child in &self.children {
            child.update_world_transforms(world_transform);
        }
    }

    pub(crate) fn destroy_components(&self, scene: &Scene) -> Result<()> {
        let mut first_error = None;
        for child in self.children.iter().rev() {
            if let Err(error) = child.destroy_components(scene) {
                first_error.get_or_insert(error);
            }
        }
        for &index in self.component_indices.borrow().iter().rev() {
            if let Some(instance) = scene.components.get(index) {
                let mut instance = instance.borrow_mut();
                if instance.awoken {
                    if let Err(error) = instance.component.destroy(scene, self) {
                        first_error.get_or_insert(error);
                    }
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub(crate) fn find_by_id(&self, id: u64) -> Option<&Self> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find_by_id(id))
    }

    pub(crate) fn find_by_id_mut(&mut self, id: u64) -> Option<&mut Self> {
        if self.id == id {
            return Some(self);
        }
        self.children
            .iter_mut()
            .find_map(|child| child.find_by_id_mut(id))
    }
}

fn is_finite_transform(transform: Transform) -> bool {
    [
        transform.x.x,
        transform.x.y,
        transform.x.z,
        transform.x.w,
        transform.y.x,
        transform.y.y,
        transform.y.z,
        transform.y.w,
        transform.z.x,
        transform.z.y,
        transform.z.z,
        transform.z.w,
        transform.w.x,
        transform.w.y,
        transform.w.z,
        transform.w.w,
    ]
    .iter()
    .all(|value| value.is_finite())
}

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(default)]
pub(crate) struct TransformDefinition {
    #[serde(with = "vector3_serde")]
    pub translation: Vector3<f32>,
    #[serde(with = "quaternion_serde")]
    pub rotation: Quaternion<f32>,
    #[serde(with = "vector3_serde")]
    pub scale: Vector3<f32>,
}

impl TransformDefinition {
    pub(crate) fn to_transform(self) -> Result<Transform> {
        if !self.translation.x.is_finite()
            || !self.translation.y.is_finite()
            || !self.translation.z.is_finite()
            || !self.scale.x.is_finite()
            || !self.scale.y.is_finite()
            || !self.scale.z.is_finite()
            || !self.rotation.s.is_finite()
            || !self.rotation.v.x.is_finite()
            || !self.rotation.v.y.is_finite()
            || !self.rotation.v.z.is_finite()
        {
            bail!("game object transform values must be finite");
        }
        if self.scale.x.abs() <= f32::EPSILON
            || self.scale.y.abs() <= f32::EPSILON
            || self.scale.z.abs() <= f32::EPSILON
            || self.rotation.magnitude2() <= f32::EPSILON
        {
            bail!("game object scale and rotation must be non-zero");
        }
        let transform = Matrix4::from_translation(self.translation)
            * Matrix4::from(self.rotation.normalize())
            * Matrix4::from_nonuniform_scale(self.scale.x, self.scale.y, self.scale.z);
        if !is_finite_transform(transform) {
            bail!("game object transform must be finite");
        }
        Ok(transform)
    }
}

impl Default for TransformDefinition {
    fn default() -> Self {
        Self {
            translation: Vector3::new(0.0, 0.0, 0.0),
            rotation: Quaternion::one(),
            scale: Vector3::new(1.0, 1.0, 1.0),
        }
    }
}

mod vector3_serde {
    use cgmath::Vector3;
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vector3<f32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let [x, y, z] = <[f32; 3]>::deserialize(deserializer)?;
        Ok(Vector3::new(x, y, z))
    }
}

mod quaternion_serde {
    use cgmath::Quaternion;
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Quaternion<f32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let [x, y, z, w] = <[f32; 4]>::deserialize(deserializer)?;
        Ok(Quaternion::new(w, x, y, z))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_definition_deserializes_quaternion_and_builds_cgmath_matrix() {
        let definition: TransformDefinition = serde_yaml::from_str(
            "translation: [2.0, 0.0, 0.0]\nrotation: [0.0, 0.0, 0.70710677, 0.70710677]\nscale: [1.0, 2.0, 1.0]\n",
        )
        .unwrap();
        let transform = definition.to_transform().unwrap();
        assert!((transform.w.x - 2.0).abs() < 0.0001);
        assert!((transform.determinant().abs() - 2.0).abs() < 0.0001);
        assert!(
            serde_yaml::from_str::<TransformDefinition>("rotation: [0.0, 0.0, 0.0, 0.0]\n")
                .unwrap()
                .to_transform()
                .is_err()
        );
    }
}
