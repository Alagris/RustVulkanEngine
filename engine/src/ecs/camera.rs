use crate::ecs::component::ComponentInstance;
use crate::ecs::{Component, GameObject, Scene};
use crate::graphics::FrameUniform;
use anyhow::{Result, bail};
use glam::{Mat4, Vec3, Vec4};
use serde::{Deserialize, Serialize};
use std::any::Any;

#[crate::ecs::component]
#[derive(crate::ecs::Component)]
#[unique]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Camera {
    pub position: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            instance: ComponentInstance::default(),
            position: Vec3::new(0.0, 3.0, 5.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: 60_f32.to_radians(),
            near: 0.1,
            far: 50.0,
        }
    }
}

impl Camera {
    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.position, self.target, self.up)
    }

    pub fn frame_uniform(&self, aspect: f32, cascade_count: u32) -> Result<FrameUniform> {
        if !aspect.is_finite()
            || aspect <= 0.0
            || !self.near.is_finite()
            || self.near <= 0.0
            || !self.far.is_finite()
            || self.far <= self.near
            || !self.fov_y.is_finite()
            || self.fov_y <= 0.0
            || self.fov_y >= std::f32::consts::PI
            || !(1..=4).contains(&cascade_count)
            || !self.position.is_finite()
            || !self.target.is_finite()
            || !self.up.is_finite()
            || self.up.length_squared() <= f32::EPSILON
            || self.position.distance_squared(self.target) <= f32::EPSILON
        {
            bail!("camera projection and view settings are invalid");
        }
        let view = self.view_matrix();
        let inverse_view = view.inverse();
        let camera_position = inverse_view.transform_point3(Vec3::ZERO);
        let camera_right = inverse_view.transform_vector3(Vec3::X).normalize();
        let camera_up = inverse_view.transform_vector3(Vec3::Y).normalize();
        let camera_forward = inverse_view.transform_vector3(-Vec3::Z).normalize();
        let tangent = (self.fov_y * 0.5).tan();
        let light_direction = Vec3::new(-2.0, -4.0, -1.0).normalize();
        let mut shadow_view_projection = [Mat4::IDENTITY.to_cols_array_2d(); 4];
        let mut cascade_splits = [self.far; 4];
        let mut previous_split = self.near;

        for cascade in 0..cascade_count as usize {
            let fraction = (cascade + 1) as f32 / cascade_count as f32;
            let logarithmic = self.near * (self.far / self.near).powf(fraction);
            let linear = self.near + (self.far - self.near) * fraction;
            let split = 0.5 * logarithmic + 0.5 * linear;
            cascade_splits[cascade] = split;

            let mut center = Vec3::ZERO;
            let mut corners = [Vec3::ZERO; 8];
            let mut corner_index = 0;
            for distance in [previous_split, split] {
                let half_height = tangent * distance;
                let half_width = half_height * aspect;
                for x_sign in [-1.0, 1.0] {
                    for y_sign in [-1.0, 1.0] {
                        let corner = camera_position
                            + camera_forward * distance
                            + camera_right * (half_width * x_sign)
                            + camera_up * (half_height * y_sign);
                        corners[corner_index] = corner;
                        center += corner;
                        corner_index += 1;
                    }
                }
            }
            assert_eq!(corner_index, corners.len());
            center /= corners.len() as f32;
            let radius = corners
                .iter()
                .map(|corner| corner.distance(center))
                .fold(0.0_f32, f32::max)
                .max(0.1);
            let light_up = if light_direction.dot(Vec3::Y).abs() > 0.98 {
                Vec3::Z
            } else {
                Vec3::Y
            };
            let light_view =
                Mat4::look_at_rh(center - light_direction * radius * 2.0, center, light_up);
            let mut light_projection =
                Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.1, radius * 4.0);
            light_projection.y_axis.y *= -1.0;
            shadow_view_projection[cascade] = (light_projection * light_view).to_cols_array_2d();
            previous_split = split;
        }
        for cascade in cascade_count as usize..4 {
            shadow_view_projection[cascade] = shadow_view_projection[cascade_count as usize - 1];
        }
        cascade_splits[cascade_count as usize - 1] = self.far;

        let mut projection = Mat4::perspective_rh(self.fov_y, aspect, self.near, self.far);
        projection.y_axis.y *= -1.0;
        let view_projection = projection * view;
        assert!(view_projection.is_finite());
        Ok(FrameUniform {
            view_projection: view_projection.to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            shadow_view_projection,
            cascade_splits: Vec4::from_array(cascade_splits).to_array(),
            cascade_index: 0,
            padding: [0; 3],
            directional_light_direction: [0.0, -1.0, 0.0, 0.0],
            directional_light_color_intensity: [1.0, 1.0, 1.0, 0.0],
            camera_position: camera_position.extend(1.0).to_array(),
        })
    }
}

impl Component for Camera {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn start(&mut self, _scene: &Scene, _game_object: &GameObject) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_is_serializable_and_builds_frame_uniforms() {
        let camera = Camera::default();
        let serialized = serde_yaml::to_string(&camera).unwrap();
        assert!(!serialized.contains("instance"));
        let parsed: Camera = serde_yaml::from_str(&serialized).unwrap();
        assert_eq!(parsed.position, camera.position);
        let frame = parsed.frame_uniform(16.0 / 9.0, 4).unwrap();
        assert_eq!(frame.cascade_index, 0);
        assert!(
            frame
                .view_projection
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
    }

    #[test]
    fn rejects_invalid_camera_projection() {
        let mut camera = Camera::default();
        camera.near = -1.0;
        assert!(camera.frame_uniform(1.0, 4).is_err());
    }
}
