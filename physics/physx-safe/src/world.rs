use std::{marker::PhantomData, rc::Rc};

use glam::{Quat, Vec3};
#[cfg(any(test, feature = "memory-tracking"))]
use physx::foundation::TrackingAllocator;
use physx::{
    geometry::{
        BoxGeometry, CapsuleGeometry, Geometry, PlaneGeometry, PxBoxGeometry, PxCapsuleGeometry,
        PxPlaneGeometry, PxSphereGeometry, SphereGeometry,
    },
    math::PxVec3,
    owner::Owner,
    physics::{Physics, PhysicsFoundationBuilder},
    rigid_actor::RigidActor,
    rigid_body::{ForceMode, RigidBody},
    rigid_dynamic::RigidDynamic,
    rigid_static::RigidStatic,
    scene::Scene,
    traits::descriptor::SceneDescriptor,
};

use crate::{
    body::{BodyHandle, BodyTag, RigidBodyDesc, allocate_body_handle},
    collider::{CapsuleAxis, ColliderDesc, ColliderMaterial, GeometryKind},
    error::PhysicsError,
    native::{
        NativeArticulation, NativeDynamic, NativeLink, NativeMaterial, NativePhysics, NativeScene,
        NativeShape, NativeStatic, NoAdvance, NoCollision, NoConstraintBreak, NoTrigger,
        NoWakeSleep,
    },
    pose::Pose,
};

/// Owns the scene and every material before the PhysX context is dropped.
///
/// The wrapper deliberately exposes handles and copied values only. It is
/// neither `Send` nor `Sync`; use it from one thread until PhysX threading is
/// explicitly coordinated with the engine.
pub struct PhysicsWorld {
    scene: Owner<NativeScene>,
    materials: Vec<Owner<NativeMaterial>>,
    physics: NativePhysics,
    _thread_affinity: PhantomData<Rc<()>>,
}

impl PhysicsWorld {
    pub fn new(gravity: Vec3) -> Result<Self, PhysicsError> {
        if !gravity.is_finite() {
            return Err(PhysicsError::InvalidGravity);
        }
        crate::invariant_assert!(gravity.is_finite());

        #[cfg(not(any(test, feature = "memory-tracking")))]
        let foundation = PhysicsFoundationBuilder::default();
        #[cfg(any(test, feature = "memory-tracking"))]
        let foundation = PhysicsFoundationBuilder::new(TrackingAllocator::default());
        let mut physics = foundation
            .build::<NativeShape>()
            .ok_or(PhysicsError::InitializationFailed)?;
        let mut descriptor: SceneDescriptor<
            (),
            NativeLink,
            NativeStatic,
            NativeDynamic,
            NativeArticulation,
            NoCollision,
            NoTrigger,
            NoConstraintBreak,
            NoWakeSleep,
            NoAdvance,
        > = SceneDescriptor::new(());
        descriptor.gravity = PxVec3::new(gravity.x, gravity.y, gravity.z);
        let scene = physics
            .physics_mut()
            .create_scene(descriptor)
            .ok_or(PhysicsError::InitializationFailed)?;
        crate::invariant_assert!(scene.get_user_data() == &());

        Ok(Self {
            scene,
            materials: Vec::new(),
            physics,
            _thread_affinity: PhantomData,
        })
    }

    pub fn create_static_collider(
        &mut self,
        pose: Pose,
        collider: ColliderDesc,
    ) -> Result<BodyHandle, PhysicsError> {
        let handle = allocate_body_handle()?;
        match collider.geometry.0 {
            GeometryKind::Sphere { radius } => {
                let geometry = PxSphereGeometry::new(radius);
                self.create_static_with_geometry(handle, pose, collider, geometry, Quat::IDENTITY)
            }
            GeometryKind::Box { half_extents } => {
                let geometry = PxBoxGeometry::new(half_extents.x, half_extents.y, half_extents.z);
                self.create_static_with_geometry(handle, pose, collider, geometry, Quat::IDENTITY)
            }
            GeometryKind::Capsule {
                radius,
                half_height,
                axis,
            } => {
                let geometry = PxCapsuleGeometry::new(radius, half_height);
                self.create_static_with_geometry(
                    handle,
                    pose,
                    collider,
                    geometry,
                    capsule_alignment(axis),
                )
            }
            GeometryKind::Plane => {
                let geometry = PxPlaneGeometry::new();
                self.create_static_with_geometry(handle, pose, collider, geometry, Quat::IDENTITY)
            }
        }
    }

    pub fn create_dynamic_body(
        &mut self,
        descriptor: RigidBodyDesc,
    ) -> Result<BodyHandle, PhysicsError> {
        if !descriptor.mass.is_finite() || descriptor.mass <= 0.0 {
            return Err(PhysicsError::InvalidMass);
        }
        crate::invariant_assert!(descriptor.mass.is_finite() && descriptor.mass > 0.0);
        let handle = allocate_body_handle()?;
        let collider = descriptor.collider;
        match collider.geometry.0 {
            GeometryKind::Sphere { radius } => {
                let geometry = PxSphereGeometry::new(radius);
                self.create_dynamic_with_geometry(
                    handle,
                    descriptor.pose,
                    descriptor.mass,
                    collider,
                    geometry,
                    Quat::IDENTITY,
                )
            }
            GeometryKind::Box { half_extents } => {
                let geometry = PxBoxGeometry::new(half_extents.x, half_extents.y, half_extents.z);
                self.create_dynamic_with_geometry(
                    handle,
                    descriptor.pose,
                    descriptor.mass,
                    collider,
                    geometry,
                    Quat::IDENTITY,
                )
            }
            GeometryKind::Capsule {
                radius,
                half_height,
                axis,
            } => {
                let geometry = PxCapsuleGeometry::new(radius, half_height);
                self.create_dynamic_with_geometry(
                    handle,
                    descriptor.pose,
                    descriptor.mass,
                    collider,
                    geometry,
                    capsule_alignment(axis),
                )
            }
            GeometryKind::Plane => Err(PhysicsError::DynamicPlaneCollider),
        }
    }

    pub fn step(&mut self, time_step: f32) -> Result<(), PhysicsError> {
        if !time_step.is_finite() || time_step <= 0.0 {
            return Err(PhysicsError::InvalidTimeStep);
        }
        crate::invariant_assert!(time_step.is_finite() && time_step > 0.0);

        // No callback task or scratch buffer is supplied, and this call blocks
        // until fetch_results completes the simulation.
        unsafe {
            self.scene
                .step(time_step, None, None, true)
                .map_err(PhysicsError::SimulationFailed)
        }
    }

    pub fn pose(&mut self, handle: BodyHandle) -> Option<Pose> {
        for actor in self.scene.get_dynamic_actors() {
            if actor.get_user_data().handle == handle {
                return Pose::from_native(actor.get_global_pose());
            }
        }
        for actor in self.scene.get_static_actors() {
            if actor.get_user_data().handle == handle {
                return Pose::from_native(actor.get_global_pose());
            }
        }
        None
    }

    pub fn set_pose(&mut self, handle: BodyHandle, pose: Pose) -> bool {
        let native_pose = pose.to_native();
        for actor in self.scene.get_dynamic_actors() {
            if actor.get_user_data().handle == handle {
                actor.set_global_pose(&native_pose, true);
                return true;
            }
        }
        for actor in self.scene.get_static_actors() {
            if actor.get_user_data().handle == handle {
                actor.set_global_pose(&native_pose, true);
                return true;
            }
        }
        false
    }

    pub fn apply_force(&mut self, handle: BodyHandle, force: Vec3) -> Result<(), PhysicsError> {
        if !force.is_finite() {
            return Err(PhysicsError::InvalidForce);
        }
        crate::invariant_assert!(force.is_finite());
        for actor in self.scene.get_dynamic_actors() {
            if actor.get_user_data().handle == handle {
                actor.add_force(
                    &PxVec3::new(force.x, force.y, force.z),
                    ForceMode::Force,
                    true,
                );
                return Ok(());
            }
        }
        Err(PhysicsError::BodyNotFound)
    }

    fn create_static_with_geometry<G: Geometry>(
        &mut self,
        handle: BodyHandle,
        pose: Pose,
        collider: ColliderDesc,
        geometry: G,
        axis_rotation: Quat,
    ) -> Result<BodyHandle, PhysicsError> {
        let mut material = self.create_material(collider.material)?;
        let local_pose = collider.local_pose.with_local_rotation(axis_rotation);
        let actor = self
            .physics
            .physics_mut()
            .create_rigid_static(
                pose.to_native(),
                &geometry,
                &mut material,
                local_pose.to_native(),
                BodyTag::new(handle),
            )
            .ok_or(PhysicsError::NativeObjectCreationFailed)?;

        self.scene.add_static_actor(actor);
        self.materials.push(material);
        crate::invariant_assert!(
            self.pose(handle).is_some(),
            "new static body is missing from its scene"
        );
        Ok(handle)
    }

    fn create_dynamic_with_geometry<G: Geometry>(
        &mut self,
        handle: BodyHandle,
        pose: Pose,
        mass: f32,
        collider: ColliderDesc,
        geometry: G,
        axis_rotation: Quat,
    ) -> Result<BodyHandle, PhysicsError> {
        let mut material = self.create_material(collider.material)?;
        let local_pose = collider.local_pose.with_local_rotation(axis_rotation);
        let mut actor = self
            .physics
            .physics_mut()
            .create_rigid_dynamic(
                pose.to_native(),
                &geometry,
                &mut material,
                1.0,
                local_pose.to_native(),
                BodyTag::new(handle),
            )
            .ok_or(PhysicsError::NativeObjectCreationFailed)?;
        actor.set_mass(mass);

        self.scene.add_dynamic_actor(actor);
        self.materials.push(material);
        crate::invariant_assert!(
            self.pose(handle).is_some(),
            "new dynamic body is missing from its scene"
        );
        Ok(handle)
    }

    fn create_material(
        &mut self,
        material: ColliderMaterial,
    ) -> Result<Owner<NativeMaterial>, PhysicsError> {
        self.physics
            .physics_mut()
            .create_material(
                material.static_friction,
                material.dynamic_friction,
                material.restitution,
                (),
            )
            .ok_or(PhysicsError::NativeObjectCreationFailed)
    }
}

fn capsule_alignment(axis: CapsuleAxis) -> Quat {
    match axis {
        CapsuleAxis::X => Quat::IDENTITY,
        CapsuleAxis::Y => Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        CapsuleAxis::Z => Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
    }
}
