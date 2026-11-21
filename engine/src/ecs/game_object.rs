use crate::ecs::{Component, Scene};
use crate::graphics::{Graphics, Transform};
use anyhow::{Result, bail};
use glam::{Mat4, Quat, Vec3};
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;

pub(crate) type ComponentHandle = Rc<ComponentInstance>;

pub(crate) struct ComponentInstance {
    pub(crate) component: RefCell<Box<dyn Component>>,
    pub(crate) updates: Cell<bool>,
}

pub struct GameObject {
    pub name: String,
    pub transform: Cell<Transform>,
    pub children: Vec<GameObject>,
    components: Vec<ComponentHandle>,
    update_components: RefCell<Vec<usize>>,
}

impl GameObject {
    pub(crate) fn new(
        name: String,
        transform: Transform,
        components: Vec<Box<dyn Component>>,
        children: Vec<GameObject>,
    ) -> Self {
        assert!(
            !name.trim().is_empty(),
            "game object names must not be empty"
        );
        Self {
            name,
            transform: Cell::new(transform),
            children,
            components: components
                .into_iter()
                .map(|component| {
                    Rc::new(ComponentInstance {
                        component: RefCell::new(component),
                        updates: Cell::new(false),
                    })
                })
                .collect(),
            update_components: RefCell::new(Vec::new()),
        }
    }

    pub fn get_component<T: Component + 'static>(&self) -> Option<Ref<'_, T>> {
        self.components.iter().find_map(|component| {
            let borrowed = component.component.borrow();
            Ref::filter_map(borrowed, |value| value.as_any().downcast_ref::<T>()).ok()
        })
    }

    pub fn get_components<T: Component + 'static>(&self) -> Vec<Ref<'_, T>> {
        self.components
            .iter()
            .filter_map(|component| {
                let borrowed = component.component.borrow();
                Ref::filter_map(borrowed, |value| value.as_any().downcast_ref::<T>()).ok()
            })
            .collect()
    }

    pub fn components(&self) -> usize {
        self.components.len()
    }

    pub(crate) fn awake(&self, scene: &Scene) {
        for component in &self.components {
            component.component.borrow_mut().awake(scene);
        }
        for child in &self.children {
            child.awake(scene);
        }
    }

    pub(crate) fn start(
        &self,
        scene: &Scene,
        path: &mut Vec<usize>,
        update_objects: &mut Vec<Vec<usize>>,
    ) {
        let mut active_components = Vec::new();
        for (index, component) in self.components.iter().enumerate() {
            let updates = component.component.borrow_mut().start(scene);
            component.updates.set(updates);
            if updates {
                active_components.push(index);
            }
        }
        *self.update_components.borrow_mut() = active_components;
        if !self.update_components.borrow().is_empty() {
            update_objects.push(path.clone());
        }
        for (index, child) in self.children.iter().enumerate() {
            path.push(index);
            child.start(scene, path, update_objects);
            path.pop();
        }
    }

    pub(crate) fn update(&self, scene: &Scene) {
        let indices = self.update_components.borrow().clone();
        for index in indices {
            let Some(component) = self.components.get(index) else {
                unreachable!("started component index must refer to a component");
            };
            assert!(
                component.updates.get(),
                "only started components can update"
            );
            component.component.borrow_mut().update(scene);
        }
    }

    pub(crate) fn render(
        &self,
        scene: &Scene,
        parent_transform: Transform,
        graphics: &mut Graphics,
    ) -> Result<()> {
        let world_transform = parent_transform * self.transform.get();
        for component in &self.components {
            component
                .component
                .borrow_mut()
                .render(scene, world_transform, graphics)?;
        }
        for child in &self.children {
            child.render(scene, world_transform, graphics)?;
        }
        Ok(())
    }

    pub(crate) fn at_path<'a>(&'a self, path: &[usize]) -> &'a Self {
        let Some((&child_index, remainder)) = path.split_first() else {
            return self;
        };
        let child = self
            .children
            .get(child_index)
            .expect("scene update path must remain valid");
        child.at_path(remainder)
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(default)]
pub(crate) struct TransformDefinition {
    pub translation: [f32; 3],
    pub rotation_degrees: [f32; 3],
    pub scale: [f32; 3],
}

impl TransformDefinition {
    pub(crate) fn to_transform(self) -> Result<Transform> {
        if self
            .translation
            .iter()
            .chain(self.rotation_degrees.iter())
            .chain(self.scale.iter())
            .any(|value| !value.is_finite())
        {
            bail!("game object transform values must be finite");
        }
        let scale = Vec3::from_array(self.scale);
        if scale.abs().min_element() <= f32::EPSILON {
            bail!("game object scale must not contain zero");
        }
        let transform = Mat4::from_scale_rotation_translation(
            scale,
            Quat::from_euler(
                glam::EulerRot::XYZ,
                self.rotation_degrees[0].to_radians(),
                self.rotation_degrees[1].to_radians(),
                self.rotation_degrees[2].to_radians(),
            ),
            Vec3::from_array(self.translation),
        );
        if !transform.is_finite() {
            bail!("game object transform must be finite");
        }
        Ok(transform)
    }
}

impl Default for TransformDefinition {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation_degrees: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}
