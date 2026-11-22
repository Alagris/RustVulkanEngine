use crate::ecs::component::ComponentInstance;
use crate::ecs::{Component, GameObject, Scene};
use crate::graphics::Graphics;
use anyhow::{Result, bail};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::any::Any;

#[crate::ecs::component]
#[derive(crate::ecs::Component)]
#[unique]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DirectionalLight {
    pub direction: Vec3,
    pub color: Vec3,
    pub intensity: f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            instance: ComponentInstance::default(),
            direction: Vec3::new(-0.4, -1.0, -0.5).normalize(),
            color: Vec3::ONE,
            intensity: 1.0,
        }
    }
}

impl DirectionalLight {
    pub fn uniform_values(&self) -> Result<([f32; 4], [f32; 4])> {
        if !self.direction.is_finite()
            || self.direction.length_squared() <= f32::EPSILON
            || !self.color.is_finite()
            || self.color.min_element() < 0.0
            || !self.intensity.is_finite()
            || self.intensity < 0.0
        {
            bail!("directional light settings must be finite and non-negative");
        }
        let direction = self.direction.normalize().extend(0.0).to_array();
        let color_intensity = self.color.extend(self.intensity).to_array();
        Ok((direction, color_intensity))
    }
}

impl Component for DirectionalLight {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn update(
        &mut self,
        _scene: &Scene,
        _game_object: &GameObject,
        _graphics: &mut Graphics,
    ) -> Result<()> {
        Ok(())
    }
}
