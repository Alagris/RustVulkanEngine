use crate::ecs::{GameObject, Scene};
use anyhow::{Result, anyhow, bail};
use glam::{Mat4, Quat, Vec3};
use physx_safe::{
    CapsuleAxis, CapsuleController, CapsuleControllerDesc, Collider as NativeBodyCollider,
    ColliderDesc, ColliderGeometry, ColliderMaterial, GeometryKind, PhysicsWorld, Pose,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::{Rc, Weak};

pub trait Collider {
    type Primitive: ColliderPrimitive;
    fn collider(&self) -> &PhysicsCollider<Self::Primitive>;
}

pub trait ColliderPrimitive {
    fn accepts(geometry: GeometryKind) -> bool;
}

pub struct Sphere;
pub struct BoxShape;
pub struct Capsule;
pub struct Plane;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ColliderSettings {
    pub center: Vec3,
    pub static_friction: f32,
    pub dynamic_friction: f32,
    pub restitution: f32,
}

impl Default for ColliderSettings {
    fn default() -> Self {
        Self {
            center: Vec3::ZERO,
            static_friction: 0.5,
            dynamic_friction: 0.5,
            restitution: 0.0,
        }
    }
}

impl ColliderSettings {
    pub fn material(self) -> Result<ColliderMaterial> {
        Ok(ColliderMaterial::new(
            self.static_friction,
            self.dynamic_friction,
            self.restitution,
        )?)
    }
}

macro_rules! impl_static_collider {
    ($component:ident, $primitive:ty, $geometry:expr) => {
        impl crate::ecs::Collider for $component {
            type Primitive = $primitive;
            fn collider(&self) -> &crate::ecs::PhysicsCollider<Self::Primitive> {
                &self.collider
            }
        }

        impl crate::ecs::Component for $component {
            fn try_start(
                &mut self,
                _: &crate::ecs::Scene,
                _: &crate::ecs::GameObject,
            ) -> anyhow::Result<crate::ecs::Features> {
                Ok(crate::ecs::PHYSICS)
            }
            fn sync_physics(
                &mut self,
                scene: &crate::ecs::Scene,
                object: &crate::ecs::GameObject,
            ) -> anyhow::Result<()> {
                let geometry = ($geometry)(self)?;
                self.collider.sync_static(
                    scene,
                    object,
                    geometry,
                    self.settings.material()?,
                    self.settings.center,
                )
            }
            fn destroy(
                &mut self,
                scene: &crate::ecs::Scene,
                _: &crate::ecs::GameObject,
            ) -> anyhow::Result<()> {
                self.collider.remove(scene)
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }
    };
}
pub(crate) use impl_static_collider;

impl ColliderPrimitive for Sphere {
    fn accepts(geometry: GeometryKind) -> bool {
        matches!(geometry, GeometryKind::Sphere { .. })
    }
}
impl ColliderPrimitive for BoxShape {
    fn accepts(geometry: GeometryKind) -> bool {
        matches!(geometry, GeometryKind::Box { .. })
    }
}
impl ColliderPrimitive for Capsule {
    fn accepts(geometry: GeometryKind) -> bool {
        matches!(geometry, GeometryKind::Capsule { .. })
    }
}
impl ColliderPrimitive for Plane {
    fn accepts(geometry: GeometryKind) -> bool {
        matches!(geometry, GeometryKind::Plane)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum NativeCollider {
    Body(NativeBodyCollider),
    Controller(CapsuleController),
}

#[derive(Default)]
pub(crate) struct ScenePhysics {
    pub(crate) world: RefCell<Option<PhysicsWorld>>,
    pending_removals: RefCell<Vec<NativeCollider>>,
}

impl ScenePhysics {
    pub(crate) fn flush_removals(&self, world: &mut PhysicsWorld) -> Result<()> {
        while let Some(collider) = self.pending_removals.borrow_mut().pop() {
            remove_native(world, collider)?;
        }
        Ok(())
    }
}

/// Reusable collider storage for built-in and custom components.
pub struct PhysicsCollider<P: ColliderPrimitive> {
    native: Option<NativeCollider>,
    world: Weak<ScenePhysics>,
    marker: PhantomData<P>,
}

impl<P: ColliderPrimitive> Default for PhysicsCollider<P> {
    fn default() -> Self {
        Self {
            native: None,
            world: Weak::new(),
            marker: PhantomData,
        }
    }
}

impl<P: ColliderPrimitive> PhysicsCollider<P> {
    pub fn native(&self) -> Option<&NativeCollider> {
        self.native.as_ref()
    }

    fn check_world(&self, scene: &Scene) -> Result<()> {
        if let Some(world) = self.world.upgrade() {
            if !Rc::ptr_eq(&world, &scene.physics) {
                bail!("collider belongs to another scene");
            }
        } else if self.native.is_some() {
            bail!("collider's physics world no longer exists");
        }
        Ok(())
    }

    /// Create or synchronize a static collider with the object's world transform.
    pub fn sync_static(
        &mut self,
        scene: &Scene,
        object: &GameObject,
        geometry: ColliderGeometry,
        material: ColliderMaterial,
        center: Vec3,
    ) -> Result<()> {
        self.check_world(scene)?;
        if !P::accepts(geometry.kind()) {
            bail!("collider geometry does not match its primitive type");
        }
        let (pose, scale) = world_pose(object)?;
        let local_pose = Pose::new(center * scale, Quat::IDENTITY)?;
        let desc = ColliderDesc::new(geometry.scaled(scale)?, material).with_local_pose(local_pose);
        scene.with_physics(|world| {
            if let Some(NativeCollider::Body(collider)) = self.native {
                if collider.descriptor() == desc {
                    if !world.set_pose(collider.body(), pose) {
                        bail!("collider body no longer exists");
                    }
                    return Ok(());
                }
            } else if self.native.is_some() {
                bail!("a controller collider cannot be synchronized as a static collider");
            }
            let handle = world.create_static_collider(pose, desc)?;
            let collider = world
                .collider(handle)
                .ok_or_else(|| anyhow!("new collider is missing"))?;
            if let Some(NativeCollider::Body(old)) = self.native
                && let Err(error) = world.remove_body(old.body())
            {
                world.remove_body(handle)?;
                return Err(error.into());
            }
            self.native = Some(NativeCollider::Body(collider));
            self.world = Rc::downgrade(&scene.physics);
            Ok(())
        })
    }

    pub fn remove(&mut self, scene: &Scene) -> Result<()> {
        self.check_world(scene)?;
        let Some(native) = self.native else {
            return Ok(());
        };
        scene.with_physics(|world| remove_native(world, native))?;
        self.native = None;
        self.world = Weak::new();
        Ok(())
    }
}

impl PhysicsCollider<Capsule> {
    pub(crate) fn sync_controller(
        &mut self,
        scene: &Scene,
        object: &GameObject,
        radius: f32,
        height: f32,
        step_offset: f32,
        settings: ColliderSettings,
    ) -> Result<()> {
        self.check_world(scene)?;
        let (pose, scale) = world_pose(object)?;
        if (pose.rotation() * Vec3::Y - Vec3::Y).length_squared() > 1.0e-8 {
            bail!("character controllers must remain upright");
        }
        let geometry =
            ColliderGeometry::capsule(radius, height * 0.5, CapsuleAxis::Y)?.scaled(scale)?;
        let GeometryKind::Capsule {
            radius,
            half_height,
            ..
        } = geometry.kind()
        else {
            unreachable!()
        };
        let position = pose.translation() + pose.rotation() * (settings.center * scale);
        let descriptor = CapsuleControllerDesc::new(
            position,
            radius,
            half_height * 2.0,
            step_offset * scale.y,
            settings.material()?,
        )?;
        scene.with_physics(|world| {
            if let Some(NativeCollider::Controller(controller)) = self.native {
                let old = controller.descriptor();
                if old.radius == descriptor.radius
                    && old.height == descriptor.height
                    && old.step_offset == descriptor.step_offset
                    && old.material == descriptor.material
                {
                    if world.controller_position(controller.handle())? != position {
                        world.set_controller_position(controller.handle(), position)?;
                    }
                    return Ok(());
                }
            } else if self.native.is_some() {
                bail!("a static collider cannot be synchronized as a controller");
            }
            let controller = world.create_capsule_controller(descriptor)?;
            if let Some(NativeCollider::Controller(old)) = self.native
                && let Err(error) = world.remove_controller(old.handle())
            {
                world.remove_controller(controller.handle())?;
                return Err(error.into());
            }
            self.native = Some(NativeCollider::Controller(controller));
            self.world = Rc::downgrade(&scene.physics);
            Ok(())
        })
    }

    pub(crate) fn controller(&self) -> Result<CapsuleController> {
        match self.native {
            Some(NativeCollider::Controller(controller)) => Ok(controller),
            _ => Err(anyhow!("character controller has not been initialized")),
        }
    }
}

fn remove_native(world: &mut PhysicsWorld, native: NativeCollider) -> Result<()> {
    match native {
        NativeCollider::Body(collider) => world.remove_body(collider.body())?,
        NativeCollider::Controller(controller) => world.remove_controller(controller.handle())?,
    }
    Ok(())
}

impl<P: ColliderPrimitive> Drop for PhysicsCollider<P> {
    fn drop(&mut self) {
        if let (Some(native), Some(world)) = (self.native.take(), self.world.upgrade()) {
            match world.world.try_borrow_mut() {
                Ok(mut world) => {
                    if let Some(world) = world.as_mut()
                        && let Err(error) = remove_native(world, native)
                    {
                        log::error!("failed to release collider: {error:#}");
                    }
                }
                Err(_) => world.pending_removals.borrow_mut().push(native),
            }
        }
    }
}

pub(crate) fn world_pose(object: &GameObject) -> Result<(Pose, Vec3)> {
    let columns: [[f32; 4]; 4] = object.world_transform().into();
    let matrix = Mat4::from_cols_array_2d(&columns);
    if !matrix.is_finite() || matrix.determinant() <= 0.0 {
        bail!("physics transform must be finite and have a positive determinant");
    }
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    let reconstructed = Mat4::from_scale_rotation_translation(scale, rotation, translation);
    let original = matrix.to_cols_array();
    let rebuilt = reconstructed.to_cols_array();
    if !scale.is_finite()
        || scale.min_element() <= 0.0
        || !rotation.is_finite()
        || original
            .iter()
            .zip(rebuilt)
            .any(|(a, b)| (a - b).abs() > 1.0e-4 * a.abs().max(1.0))
    {
        bail!("physics transform must not contain shear");
    }
    Ok((Pose::new(translation, rotation)?, scale))
}
