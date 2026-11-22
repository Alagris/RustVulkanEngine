use anyhow::{Context, Result, anyhow, bail};
use cgmath::{Quaternion, Rad, Rotation3};
use engine::ecs::component::{ComponentInstance, ComponentWeak};
use engine::ecs::{CameraFollow, CharacterController, Component, GameObject, Scene};
use engine::graphics::Graphics;
use engine::input::{InputState, InputType, InputValue};
use glam::{Mat4, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::any::Any;

#[engine::ecs::component]
#[derive(engine::ecs::Component)]
#[require(CharacterController, CameraFollow, Camera)]
#[unique]
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerController {
    pub input_map: String,
    pub speed: f32,
    pub mouse_sensitivity: f32,
    pub gravity: f32,
    pub jump_force: f32,
    #[serde(skip)]
    controller: Option<ComponentWeak<CharacterController>>,
    #[serde(skip)]
    camera: Option<ComponentWeak<CameraFollow>>,
    #[serde(skip)]
    buttons: [bool; 4],
    #[serde(skip)]
    mouse_delta: Vec2,
    #[serde(skip)]
    vertical_velocity: f32,
    #[serde(skip)]
    grounded: bool,
    #[serde(skip)]
    jump_requested: bool,
}

impl Default for PlayerController {
    fn default() -> Self {
        Self {
            instance: ComponentInstance::default(),
            input_map: "Gameplay".to_owned(),
            speed: 4.0,
            mouse_sensitivity: 0.002,
            gravity: 9.81,
            jump_force: 5.0,
            controller: None,
            camera: None,
            buttons: [false; 4],
            mouse_delta: Vec2::ZERO,
            vertical_velocity: 0.0,
            grounded: false,
            jump_requested: false,
        }
    }
}

impl PlayerController {
    fn button(&mut self, index: usize, state: InputState, value: InputValue) {
        self.buttons[index] = !matches!(state, InputState::Ended | InputState::Canceled)
            && matches!(value, InputValue::Bool(true));
    }
    fn forward(&mut self, _: &Scene, _: &str, _: u32, state: InputState, value: InputValue) {
        self.button(0, state, value);
    }
    fn back(&mut self, _: &Scene, _: &str, _: u32, state: InputState, value: InputValue) {
        self.button(1, state, value);
    }
    fn left(&mut self, _: &Scene, _: &str, _: u32, state: InputState, value: InputValue) {
        self.button(2, state, value);
    }
    fn right(&mut self, _: &Scene, _: &str, _: u32, state: InputState, value: InputValue) {
        self.button(3, state, value);
    }
    fn look(&mut self, _: &Scene, _: &str, _: u32, _: InputState, value: InputValue) {
        if let InputValue::Vector2(delta) = value {
            self.mouse_delta += delta;
        }
    }
    fn jump(&mut self, _: &Scene, _: &str, _: u32, state: InputState, value: InputValue) {
        if matches!(state, InputState::Started | InputState::Triggered)
            && matches!(value, InputValue::Bool(true))
        {
            self.jump_requested = true;
        }
    }
}

impl Component for PlayerController {
    fn try_start(&mut self, scene: &Scene, object: &GameObject) -> Result<engine::ecs::Features> {
        self.controller = Some(
            object
                .get_component_weak::<CharacterController>()
                .ok_or_else(|| anyhow!("PlayerController requires a CharacterController"))?,
        );
        self.camera = Some(
            object
                .get_component_weak::<CameraFollow>()
                .ok_or_else(|| anyhow!("PlayerController requires a CameraFollow"))?,
        );
        let subscriptions: [(&str, engine::input::InputCallback<Self>); 6] = [
            ("forward", Self::forward),
            ("back", Self::back),
            ("left", Self::left),
            ("right", Self::right),
            ("look", Self::look),
            ("jump", Self::jump),
        ];
        for (binding, _) in subscriptions {
            let expected = if binding == "look" {
                InputType::Vector2
            } else {
                InputType::Bool
            };
            let actual = scene.input_binding_type(&self.input_map, binding)?;
            if actual != expected {
                bail!(
                    "PlayerController input binding '{binding}' must produce {expected:?}, not {actual:?}"
                );
            }
        }
        for (binding, callback) in subscriptions {
            if let Err(error) = scene.subscribe_input(&self.input_map, binding, self, callback) {
                scene.unsubscribe_input(self)?;
                return Err(error)
                    .with_context(|| format!("PlayerController input binding '{binding}'"));
            }
        }
        Ok(engine::ecs::UPDATE)
    }

    fn update(&mut self, scene: &Scene, object: &GameObject, _: &mut Graphics) -> Result<()> {
        if !self.jump_force.is_finite() || self.jump_force <= 0.0 {
            bail!("player jump force must be finite and positive");
        }
        if !self.speed.is_finite()
            || self.speed < 0.0
            || !self.mouse_sensitivity.is_finite()
            || self.mouse_sensitivity < 0.0
            || !self.gravity.is_finite()
            || self.gravity < 0.0
            || !self.mouse_delta.is_finite()
        {
            bail!("invalid player controller settings or input");
        }
        let mouse = std::mem::take(&mut self.mouse_delta) * self.mouse_sensitivity;
        if mouse != Vec2::ZERO {
            let mut transform = object.transform();
            transform.rotation = Quaternion::from_angle_y(Rad(-mouse.x)) * transform.rotation;
            object.set_transform(transform)?;
            self.camera
                .as_ref()
                .ok_or_else(|| anyhow!("player camera is not initialized"))?
                .with_mut(|camera| {
                    camera.pitch = (camera.pitch - mouse.y).clamp(-1.55, 1.55);
                })
                .ok_or_else(|| anyhow!("player camera no longer exists"))?;
        }
        let dt = scene.delta_time();
        if dt == 0.0 {
            return Ok(());
        }
        let mut jump_requested = std::mem::take(&mut self.jump_requested);
        if jump_requested && self.grounded {
            self.vertical_velocity = self.jump_force;
            self.grounded = false;
            jump_requested = false;
        }
        self.vertical_velocity -= self.gravity * dt;
        let columns: [[f32; 4]; 4] = object.world_transform().into();
        let transform = Mat4::from_cols_array_2d(&columns);
        let mut forward = transform.transform_vector3(-Vec3::Z);
        forward.y = 0.0;
        let forward = forward.normalize_or_zero();
        let right = forward.cross(Vec3::Y);
        let forward_axis = self.buttons[0] as i32 - self.buttons[1] as i32;
        let right_axis = self.buttons[3] as i32 - self.buttons[2] as i32;
        let movement =
            (forward * forward_axis as f32 + right * right_axis as f32).normalize_or_zero();
        let displacement = (movement * self.speed + Vec3::Y * self.vertical_velocity) * dt;
        let result = self
            .controller
            .as_ref()
            .ok_or_else(|| anyhow!("player controller is not initialized"))?
            .with_mut(|controller| controller.move_by(scene, object, displacement, dt))
            .ok_or_else(|| anyhow!("player character controller no longer exists"))??;
        if result.up && self.vertical_velocity > 0.0 {
            self.vertical_velocity = 0.0;
        }
        self.grounded = result.down && self.vertical_velocity <= 0.0;
        if self.grounded {
            self.vertical_velocity = 0.0;
            if jump_requested {
                self.vertical_velocity = self.jump_force;
                self.grounded = false;
            }
        }
        Ok(())
    }

    fn destroy(&mut self, scene: &Scene, _: &GameObject) -> Result<()> {
        scene.unsubscribe_input(self)?;
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
    use engine::data::DataAssetDatabase;
    use engine::ecs::{Camera, ComponentRegistry};
    use engine::input::{Input, InputSystem, RawInput, codes};
    use std::path::Path;
    use winit::keyboard::KeyCode;

    #[test]
    fn sample_bindings_move_player_and_turn_camera() {
        assert_eq!(codes::keyboard(KeyCode::KeyW), 41);
        assert_eq!(codes::keyboard(KeyCode::KeyS), 37);
        assert_eq!(codes::keyboard(KeyCode::KeyA), 19);
        assert_eq!(codes::keyboard(KeyCode::KeyD), 22);
        let assets =
            DataAssetDatabase::load_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets"))
                .unwrap();
        let input =
            InputSystem::from_data_asset(&assets.get::<Input>("PlayerControls").unwrap()).unwrap();
        let mut scene = Scene::from_yaml(
            r#"
root:
  name: Root
  components:
    - type: BoxCollider
      data:
        half_extents: [10.0, 0.1, 10.0]
  children:
    - name: Player
      transform:
        translation: [0.0, 1.2, 0.0]
      components:
        - type: CharacterController
        - type: Camera
        - type: CameraFollow
        - type: PlayerController
    - name: Ceiling
      transform:
        translation: [0.0, 3.8, 0.0]
      components:
        - type: BoxCollider
          data:
            half_extents: [10.0, 0.1, 10.0]
"#,
            &ComponentRegistry::new(),
        )
        .unwrap();
        scene.configure_input(input).unwrap();
        scene.initialize().unwrap();
        let player = scene.root.borrow().children()[0].clone();
        let camera = player.borrow().get_component_weak::<Camera>().unwrap();
        scene
            .consume_input(RawInput::Button {
                key_code: codes::keyboard(KeyCode::KeyW),
                pressed: true,
            })
            .unwrap();
        scene.update(0.1, &mut Graphics::default()).unwrap();
        let moved = player.borrow().transform().translation;
        assert!(moved.z < -0.3);
        scene
            .consume_input(RawInput::Button {
                key_code: codes::keyboard(KeyCode::KeyW),
                pressed: false,
            })
            .unwrap();
        for (key, direction) in [
            (KeyCode::KeyA, Vec3::NEG_X),
            (KeyCode::KeyD, Vec3::X),
            (KeyCode::KeyS, Vec3::Z),
        ] {
            let before = player.borrow().transform().translation;
            scene
                .consume_input(RawInput::Button {
                    key_code: codes::keyboard(key),
                    pressed: true,
                })
                .unwrap();
            scene.update(0.1, &mut Graphics::default()).unwrap();
            scene
                .consume_input(RawInput::Button {
                    key_code: codes::keyboard(key),
                    pressed: false,
                })
                .unwrap();
            let after = player.borrow().transform().translation;
            let displacement = Vec3::new(after.x - before.x, 0.0, after.z - before.z);
            assert!(displacement.dot(direction) > 0.3);
        }
        let moved = player.borrow().transform().translation;
        scene
            .consume_input(RawInput::CursorMoved(Vec2::ZERO))
            .unwrap();
        scene
            .consume_input(RawInput::CursorMoved(Vec2::new(100.0, 50.0)))
            .unwrap();
        scene.update(0.1, &mut Graphics::default()).unwrap();
        assert!((player.borrow().transform().translation.z - moved.z).abs() < 1.0e-4);
        camera
            .with(|camera| {
                let forward = (camera.target - camera.position).normalize();
                assert!(forward.x > 0.1);
                assert!(forward.y < -0.05);
            })
            .unwrap();
        let controller = player
            .borrow()
            .get_component_weak::<PlayerController>()
            .unwrap();
        for _ in 0..30 {
            scene.update(0.05, &mut Graphics::default()).unwrap();
        }
        assert!(controller.with(|player| player.grounded).unwrap());
        controller
            .with_mut(|player| player.jump_force = 8.0)
            .unwrap();
        let ground_y = player.borrow().transform().translation.y;
        let space = codes::keyboard(KeyCode::Space);
        assert_eq!(space, 62);
        scene
            .consume_input(RawInput::Button {
                key_code: space,
                pressed: true,
            })
            .unwrap();
        scene.update(0.0, &mut Graphics::default()).unwrap();
        assert_eq!(player.borrow().transform().translation.y, ground_y);
        scene.update(0.1, &mut Graphics::default()).unwrap();
        assert!(player.borrow().transform().translation.y > ground_y + 0.6);
        let first_velocity = controller.with(|player| player.vertical_velocity).unwrap();
        scene
            .consume_input(RawInput::Button {
                key_code: space,
                pressed: false,
            })
            .unwrap();
        scene
            .consume_input(RawInput::Button {
                key_code: space,
                pressed: true,
            })
            .unwrap();
        scene.update(0.1, &mut Graphics::default()).unwrap();
        assert!(controller.with(|player| player.vertical_velocity).unwrap() < first_velocity);
        let mut hit_ceiling = false;
        for _ in 0..60 {
            scene.update(0.05, &mut Graphics::default()).unwrap();
            hit_ceiling |= controller
                .with(|player| !player.grounded && player.vertical_velocity == 0.0)
                .unwrap();
        }
        assert!(hit_ceiling);
        assert!(controller.with(|player| player.grounded).unwrap());
        scene
            .consume_input(RawInput::Button {
                key_code: space,
                pressed: false,
            })
            .unwrap();
        let landed_y = player.borrow().transform().translation.y;
        scene
            .consume_input(RawInput::Button {
                key_code: space,
                pressed: true,
            })
            .unwrap();
        scene.update(0.1, &mut Graphics::default()).unwrap();
        assert!(player.borrow().transform().translation.y > landed_y + 0.6);
        controller
            .with_mut(|player| player.jump_force = f32::NAN)
            .unwrap();
        assert!(
            scene
                .update(0.1, &mut Graphics::default())
                .unwrap_err()
                .to_string()
                .contains("jump force must be finite and positive")
        );
        scene.destroy().unwrap();
    }

    #[test]
    fn player_reports_missing_input_configuration() {
        let mut scene = Scene::from_yaml(
            "root:\n  name: Player\n  components:\n    - type: CharacterController\n    - type: Camera\n    - type: CameraFollow\n    - type: PlayerController\n",
            &ComponentRegistry::new(),
        ).unwrap();
        assert!(
            scene
                .initialize()
                .unwrap_err()
                .to_string()
                .contains("input map")
        );
    }
}
