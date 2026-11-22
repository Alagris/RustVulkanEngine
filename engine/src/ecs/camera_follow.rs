use crate::ecs::component::{ComponentInstance, ComponentWeak};
use crate::ecs::{Camera, Component, GameObject, Scene};
use anyhow::{Result, anyhow, bail};
use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use std::any::Any;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CameraFollowMode {
    #[default]
    FirstPerson,
    Orbit {
        distance: f32,
        /// Exponential follow rate in inverse seconds; zero disables damping.
        damping: f32,
    },
}

#[crate::ecs::component]
#[derive(crate::ecs::Component)]
#[require(Camera)]
#[unique]
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct CameraFollow {
    pub mode: CameraFollowMode,
    pub offset: Vec3,
    pub pitch: f32,
    #[serde(skip)]
    camera: Option<ComponentWeak<Camera>>,
    #[serde(skip)]
    initialized: bool,
}

impl Default for CameraFollow {
    fn default() -> Self {
        Self {
            instance: ComponentInstance::default(),
            mode: CameraFollowMode::FirstPerson,
            offset: Vec3::new(0.0, 0.6, 0.0),
            pitch: 0.0,
            camera: None,
            initialized: false,
        }
    }
}

impl Component for CameraFollow {
    fn try_start(&mut self, _: &Scene, object: &GameObject) -> Result<crate::ecs::Features> {
        self.camera =
            Some(object.get_component_weak::<Camera>().ok_or_else(|| {
                anyhow!("CameraFollow requires a Camera on the same game object")
            })?);
        Ok(crate::ecs::LATE)
    }

    fn late_update(&mut self, scene: &Scene, object: &GameObject) -> Result<()> {
        if !self.offset.is_finite()
            || !self.pitch.is_finite()
            || self.pitch.abs() >= std::f32::consts::FRAC_PI_2
        {
            bail!("invalid camera follow settings");
        }
        let columns: [[f32; 4]; 4] = object.world_transform().into();
        let transform = Mat4::from_cols_array_2d(&columns);
        if !transform.is_finite() || transform.determinant().abs() <= f32::EPSILON {
            bail!("camera follow target transform is invalid");
        }
        let (_, rotation, position) = transform.to_scale_rotation_translation();
        let forward = (rotation * Quat::from_rotation_x(self.pitch)) * -Vec3::Z;
        if !forward.is_finite() || forward.cross(Vec3::Y).length_squared() <= f32::EPSILON {
            bail!("camera follow direction must not be vertical");
        }
        let anchor = position + rotation * self.offset;
        let (desired_position, desired_target, alpha) = match self.mode {
            CameraFollowMode::FirstPerson => (anchor, anchor + forward, 1.0),
            CameraFollowMode::Orbit { distance, damping } => {
                if !distance.is_finite() || distance <= 0.0 || !damping.is_finite() || damping < 0.0
                {
                    bail!("invalid camera follow orbit settings");
                }
                let alpha = if !self.initialized || damping == 0.0 {
                    1.0
                } else {
                    1.0 - (-damping * scene.delta_time()).exp()
                };
                (anchor - forward * distance, anchor, alpha)
            }
        };
        let camera = self
            .camera
            .as_ref()
            .ok_or_else(|| anyhow!("CameraFollow is not initialized"))?;
        camera
            .with_mut(|camera| -> Result<()> {
                if !desired_position.is_finite() || !desired_target.is_finite() {
                    bail!("camera follow position is not finite");
                }
                if matches!(self.mode, CameraFollowMode::Orbit { .. }) && alpha < 1.0 {
                    let current = camera.position - camera.target;
                    let desired = desired_position - desired_target;
                    let current_distance = current.length();
                    let desired_distance = desired.length();
                    if !current_distance.is_finite() || current_distance <= f32::EPSILON {
                        bail!("camera follow current view is invalid");
                    }
                    let current_yaw = current.x.atan2(current.z);
                    let desired_yaw = desired.x.atan2(desired.z);
                    let yaw_delta = (desired_yaw - current_yaw + std::f32::consts::PI)
                        .rem_euclid(std::f32::consts::TAU)
                        - std::f32::consts::PI;
                    let yaw = current_yaw + yaw_delta * alpha;
                    let current_pitch = (current.y / current_distance).clamp(-1.0, 1.0).asin();
                    let desired_pitch = (desired.y / desired_distance).clamp(-1.0, 1.0).asin();
                    let pitch = current_pitch + (desired_pitch - current_pitch) * alpha;
                    let distance = current_distance + (desired_distance - current_distance) * alpha;
                    let target = camera.target.lerp(desired_target, alpha);
                    camera.target = target;
                    camera.position = target
                        + Vec3::new(
                            yaw.sin() * pitch.cos(),
                            pitch.sin(),
                            yaw.cos() * pitch.cos(),
                        ) * distance;
                } else {
                    camera.position = desired_position;
                    camera.target = desired_target;
                }
                camera.up = Vec3::Y;
                Ok(())
            })
            .ok_or_else(|| anyhow!("CameraFollow camera no longer exists"))??;
        self.initialized = true;
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::ComponentRegistry;
    use crate::graphics::Graphics;
    use cgmath::{Quaternion, Rad, Rotation3, Vector3};

    #[test]
    fn first_person_and_orbit_follow_with_damping() {
        let mut scene = Scene::from_yaml(
            "root:\n  name: Player\n  components:\n    - type: CameraFollow\n    - type: Camera\n",
            &ComponentRegistry::new(),
        )
        .unwrap();
        scene.initialize().unwrap();
        let camera = scene.root.borrow().get_component_weak::<Camera>().unwrap();
        let follow = scene
            .root
            .borrow()
            .get_component_weak::<CameraFollow>()
            .unwrap();
        camera
            .with(|camera| {
                assert_eq!(camera.position, Vec3::new(0.0, 0.6, 0.0));
                assert_eq!(camera.target, Vec3::new(0.0, 0.6, -1.0));
            })
            .unwrap();
        follow
            .with_mut(|follow| {
                follow.mode = CameraFollowMode::Orbit {
                    distance: 4.0,
                    damping: 2.0,
                };
            })
            .unwrap();
        let mut transform = scene.root.borrow().transform();
        transform.translation = Vector3::new(10.0, 0.0, 0.0);
        scene.root.borrow().set_transform(transform).unwrap();
        scene.update(0.5, &mut Graphics::default()).unwrap();
        camera
            .with(|camera| {
                assert!((camera.position.x - 10.0 * (1.0 - (-1.0_f32).exp())).abs() < 1.0e-5);
                assert!(camera.position.z > 0.0 && camera.position.z < 4.0);
            })
            .unwrap();
        transform.rotation = Quaternion::from_angle_y(Rad(std::f32::consts::PI));
        scene.root.borrow().set_transform(transform).unwrap();
        scene
            .update(std::f32::consts::LN_2 / 2.0, &mut Graphics::default())
            .unwrap();
        camera
            .with(|camera| {
                assert!(camera.position.distance(camera.target) > 0.1);
                assert!(camera.frame_uniform(1.0, 1).is_ok());
            })
            .unwrap();
        follow
            .with_mut(|follow| follow.mode = CameraFollowMode::FirstPerson)
            .unwrap();
        scene.update(0.1, &mut Graphics::default()).unwrap();
        camera
            .with(|camera| assert_eq!(camera.position, Vec3::new(10.0, 0.6, 0.0)))
            .unwrap();
        follow
            .with_mut(|follow| {
                follow.mode = CameraFollowMode::Orbit {
                    distance: 0.0,
                    damping: 0.0,
                }
            })
            .unwrap();
        assert!(
            scene
                .update(0.1, &mut Graphics::default())
                .unwrap_err()
                .to_string()
                .contains("camera follow")
        );
        scene.destroy().unwrap();
    }

    #[test]
    fn orbit_settings_are_serialized_inside_mode() {
        let follow: CameraFollow =
            serde_yaml::from_str("mode:\n  type: orbit\n  distance: 6.0\n  damping: 3.0\n")
                .unwrap();
        assert_eq!(
            follow.mode,
            CameraFollowMode::Orbit {
                distance: 6.0,
                damping: 3.0
            }
        );
        let serialized = serde_yaml::to_string(&follow).unwrap();
        let parsed: CameraFollow = serde_yaml::from_str(&serialized).unwrap();
        assert_eq!(parsed.mode, follow.mode);
    }

    #[test]
    fn missing_camera_is_an_initialization_error() {
        let mut scene = Scene::from_yaml(
            "root:\n  name: Player\n  components:\n    - type: CameraFollow\n",
            &ComponentRegistry::new(),
        )
        .unwrap();
        assert!(
            scene
                .initialize()
                .unwrap_err()
                .to_string()
                .contains("requires a Camera")
        );
        assert!(
            scene
                .initialize()
                .unwrap_err()
                .to_string()
                .contains("requires a Camera")
        );
    }
}
