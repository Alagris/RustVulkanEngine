use crate::ecs::collider::{Capsule, ColliderSettings, PhysicsCollider, world_pose};
use crate::ecs::component::ComponentInstance;
use crate::ecs::{Collider, Component, GameObject, Scene};
use anyhow::Result;
use cgmath::Vector3;
use glam::Vec3;
use physx_safe::ControllerMoveResult;
use serde::{Deserialize, Serialize};
use std::any::Any;

#[crate::ecs::component]
#[derive(crate::ecs::Component)]
#[unique]
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterController {
    pub radius: f32,
    /// Full cylindrical height, excluding the hemispherical ends.
    pub height: f32,
    pub step_offset: f32,
    #[serde(flatten)]
    pub settings: ColliderSettings,
    #[serde(skip)]
    collider: PhysicsCollider<Capsule>,
}

impl Default for CharacterController {
    fn default() -> Self {
        Self {
            instance: ComponentInstance::default(),
            radius: 0.5,
            height: 1.0,
            step_offset: 0.2,
            settings: ColliderSettings::default(),
            collider: PhysicsCollider::default(),
        }
    }
}

impl CharacterController {
    pub fn move_by(
        &mut self,
        scene: &Scene,
        object: &GameObject,
        displacement: Vec3,
        elapsed_time: f32,
    ) -> Result<ControllerMoveResult> {
        self.sync_physics(scene, object)?;
        let controller = self.collider.controller()?;
        let (pose, scale) = world_pose(object)?;
        let offset = pose.rotation() * (self.settings.center * scale);
        let (result, position) = scene.with_physics(|world| {
            let result =
                world.move_controller(controller.handle(), displacement, 0.0, elapsed_time)?;
            Ok((result, world.controller_position(controller.handle())?))
        })?;
        let position = position - offset;
        object.set_world_translation(Vector3::new(position.x, position.y, position.z))?;
        Ok(result)
    }
}

impl Collider for CharacterController {
    type Primitive = Capsule;
    fn collider(&self) -> &PhysicsCollider<Capsule> {
        &self.collider
    }
}

impl Component for CharacterController {
    fn try_start(&mut self, _: &Scene, _: &GameObject) -> Result<crate::ecs::Features> {
        Ok(crate::ecs::Features {
            has_physics: true,
            ..Default::default()
        })
    }

    fn sync_physics(&mut self, scene: &Scene, object: &GameObject) -> Result<()> {
        self.collider.sync_controller(
            scene,
            object,
            self.radius,
            self.height,
            self.step_offset,
            self.settings,
        )
    }

    fn destroy(&mut self, scene: &Scene, _: &GameObject) -> Result<()> {
        self.collider.remove(scene)
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
    use crate::ecs::{
        BoxCollider, CapsuleCollider, ComponentRegistry, NativeCollider, SphereCollider,
    };
    use crate::graphics::Graphics;
    use cgmath::Vector3;

    #[crate::ecs::component]
    #[derive(crate::ecs::Component)]
    struct CustomCollider {
        collider: PhysicsCollider<crate::ecs::Sphere>,
    }

    impl Collider for CustomCollider {
        type Primitive = crate::ecs::Sphere;
        fn collider(&self) -> &PhysicsCollider<Self::Primitive> {
            &self.collider
        }
    }

    impl Component for CustomCollider {
        fn try_start(&mut self, _: &Scene, _: &GameObject) -> Result<crate::ecs::Features> {
            Ok(crate::ecs::Features {
                has_physics: true,
                ..Default::default()
            })
        }

        fn sync_physics(&mut self, scene: &Scene, object: &GameObject) -> Result<()> {
            self.collider.sync_static(
                scene,
                object,
                physx_safe::ColliderGeometry::sphere(0.25)?,
                physx_safe::ColliderMaterial::default(),
                Vec3::splat(100.0),
            )
        }
        fn destroy(&mut self, scene: &Scene, _: &GameObject) -> Result<()> {
            self.collider.remove(scene)
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    #[test]
    fn colliders_follow_hierarchy_and_controller_moves_and_cleans_up() {
        let mut scene = Scene::from_yaml(
            r#"
root:
  name: Root
  transform:
    translation: [5.0, 0.0, 0.0]
  components:
    - type: BoxCollider
      data:
        half_extents: [10.0, 0.1, 10.0]
    - type: SphereCollider
      data:
        center: [100.0, 100.0, 100.0]
    - type: CapsuleCollider
      data:
        center: [100.0, 100.0, 100.0]
    - type: PlaneCollider
      data:
        center: [-100.0, 0.0, 0.0]
  children:
    - name: Player
      transform:
        translation: [0.0, 1.2, 0.0]
      components:
        - type: CharacterController
"#,
            &ComponentRegistry::new(),
        )
        .unwrap();
        scene.initialize().unwrap();
        let root = scene.root.clone();
        let player = root.borrow().children()[0].clone();
        let sphere = root
            .borrow()
            .get_component_weak::<SphereCollider>()
            .unwrap();
        let capsule = root
            .borrow()
            .get_component_weak::<CapsuleCollider>()
            .unwrap();
        let floor = root.borrow().get_component_weak::<BoxCollider>().unwrap();
        let controller = player
            .borrow()
            .get_component_weak::<CharacterController>()
            .unwrap();
        let floor_handle = floor
            .with(|c| match c.collider().native().unwrap() {
                NativeCollider::Body(c) => c.body(),
                _ => panic!("expected static body"),
            })
            .unwrap();
        let sphere_handle = sphere
            .with(|c| match c.collider().native().unwrap() {
                NativeCollider::Body(c) => c.body(),
                _ => panic!("expected static body"),
            })
            .unwrap();
        assert!(capsule.with(|c| c.collider().native().is_some()).unwrap());

        let result = controller
            .with_mut(|c| c.move_by(&scene, &player.borrow(), Vec3::NEG_Y, 0.1))
            .unwrap()
            .unwrap();
        assert!(result.down);
        assert!(player.borrow().transform().translation.y > 1.0);
        let mut transform = root.borrow().transform();
        transform.translation = Vector3::new(10.0, 0.0, 0.0);
        root.borrow().set_transform(transform).unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        scene
            .with_physics(|world| {
                assert_eq!(world.pose(floor_handle).unwrap().translation().x, 10.0);
                Ok(())
            })
            .unwrap();
        let native_position = controller
            .with(|c| match c.collider().native().unwrap() {
                NativeCollider::Controller(c) => c.handle(),
                _ => panic!("expected controller"),
            })
            .unwrap();
        scene
            .with_physics(|world| {
                assert!((world.controller_position(native_position)?.x - 10.0).abs() < 1.0e-5);
                Ok(())
            })
            .unwrap();

        floor.with_mut(|floor| floor.half_extents.x = 9.0).unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        scene
            .with_physics(|world| {
                assert!(world.collider(floor_handle).is_none());
                Ok(())
            })
            .unwrap();
        root.borrow()
            .add_component(
                &scene,
                Box::new(CustomCollider {
                    instance: ComponentInstance::default(),
                    collider: PhysicsCollider::default(),
                }),
            )
            .unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        let custom = root
            .borrow()
            .get_component_weak::<CustomCollider>()
            .unwrap();
        assert!(
            custom
                .with(|component| component.collider().native().is_some())
                .unwrap()
        );

        scene
            .remove_component::<SphereCollider>(&root.borrow())
            .unwrap();
        scene.update(0.016, &mut Graphics::default()).unwrap();
        scene
            .with_physics(|world| {
                assert!(world.collider(sphere_handle).is_none());
                Ok(())
            })
            .unwrap();
        let mut temporary = PhysicsCollider::<crate::ecs::Sphere>::default();
        temporary
            .sync_static(
                &scene,
                &root.borrow(),
                physx_safe::ColliderGeometry::sphere(0.25).unwrap(),
                physx_safe::ColliderMaterial::default(),
                Vec3::splat(100.0),
            )
            .unwrap();
        let temporary_handle = match temporary.native().unwrap() {
            NativeCollider::Body(collider) => collider.body(),
            _ => panic!("expected static body"),
        };
        scene
            .with_physics(|world| {
                drop(temporary);
                assert!(world.collider(temporary_handle).is_some());
                Ok(())
            })
            .unwrap();
        scene
            .with_physics(|world| {
                assert!(world.collider(temporary_handle).is_none());
                Ok(())
            })
            .unwrap();
        scene.destroy().unwrap();

        let mut invalid = Scene::from_yaml(
            "root:\n  name: Bad\n  transform:\n    scale: [1.0, 2.0, 1.0]\n  components:\n    - type: SphereCollider\n",
            &ComponentRegistry::new(),
        ).unwrap();
        assert!(
            invalid
                .initialize()
                .unwrap_err()
                .to_string()
                .contains("nonuniform scale")
        );
    }
}
