use std::{cell::RefCell, collections::HashMap, marker::PhantomData, ptr::NonNull, rc::Rc};

use glam::{Quat, Vec3};
use physx::{
    controller::PxCapsuleController,
    controller_manager::PxControllerManager,
    geometry::{
        BoxGeometry, CapsuleGeometry, Geometry, PlaneGeometry, PxBoxGeometry, PxCapsuleGeometry,
        PxPlaneGeometry, PxSphereGeometry, SphereGeometry,
    },
    math::PxVec3,
    owner::Owner,
    physics::Physics,
    rigid_actor::RigidActor,
    rigid_body::{ForceMode, RigidBody},
    scene::Scene,
    traits::descriptor::SceneDescriptor,
};

use crate::{
    body::{BodyHandle, BodyTag, RigidBodyDesc, allocate_body_handle},
    collider::{CapsuleAxis, Collider, ColliderDesc, ColliderMaterial, GeometryKind},
    context::PhysicsContext,
    controller::{CapsuleController, ControllerHandle},
    error::PhysicsError,
    native::{
        NativeArticulation, NativeDynamic, NativeLink, NativeMaterial, NativeScene, NativeStatic,
        NoAdvance, NoCollision, NoConstraintBreak, NoTrigger, NoWakeSleep,
    },
    pose::Pose,
};

/// Owns the scene and every material before the PhysX context is dropped.
///
/// The wrapper deliberately exposes handles and copied values only. It is
/// neither `Send` nor `Sync`; use it from one thread until PhysX threading is
/// explicitly coordinated with the engine.
pub struct PhysicsWorld {
    // Controllers must be released before their scene and materials.
    pub(crate) controllers:
        Option<Owner<PxControllerManager<PxCapsuleController<ControllerHandle>>>>,
    pub(crate) scene: Owner<NativeScene>,
    bodies: HashMap<BodyHandle, BodyEntry>,
    pub(crate) controller_materials: HashMap<ControllerHandle, ControllerEntry>,
    physics: Rc<RefCell<PhysicsContext>>,
    _thread_affinity: PhantomData<Rc<()>>,
}

enum BodyActor {
    Static(NonNull<NativeStatic>),
    Dynamic(NonNull<NativeDynamic>),
}

struct BodyEntry {
    actor: BodyActor,
    collider: Collider,
    _material: Owner<NativeMaterial>,
}

pub(crate) struct ControllerEntry {
    pub(crate) collider: CapsuleController,
    pub(crate) _material: Owner<NativeMaterial>,
}

impl PhysicsWorld {
    pub fn new(gravity: Vec3) -> Result<Self, PhysicsError> {
        if !gravity.is_finite() {
            return Err(PhysicsError::InvalidGravity);
        }
        crate::invariant_assert!(gravity.is_finite());

        let physics = PhysicsContext::acquire()?;
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
            .borrow_mut()
            .physics
            .physics_mut()
            .create_scene(descriptor)
            .ok_or(PhysicsError::InitializationFailed)?;
        crate::invariant_assert!(scene.get_user_data() == &());

        Ok(Self {
            controllers: None,
            scene,
            bodies: HashMap::new(),
            controller_materials: HashMap::new(),
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
        let body = self.bodies.get(&handle)?;
        // The scene owns these actors until remove_body or world destruction.
        unsafe {
            match body.actor {
                BodyActor::Static(actor) => Pose::from_native(actor.as_ref().get_global_pose()),
                BodyActor::Dynamic(actor) => Pose::from_native(actor.as_ref().get_global_pose()),
            }
        }
    }

    pub fn set_pose(&mut self, handle: BodyHandle, pose: Pose) -> bool {
        let Some(body) = self.bodies.get_mut(&handle) else {
            return false;
        };
        // Exclusive access to the world prevents aliases to scene-owned actors.
        unsafe {
            match &mut body.actor {
                BodyActor::Static(actor) => actor.as_mut().set_global_pose(&pose.to_native(), true),
                BodyActor::Dynamic(actor) => {
                    actor.as_mut().set_global_pose(&pose.to_native(), true)
                }
            }
        }
        true
    }

    pub fn apply_force(&mut self, handle: BodyHandle, force: Vec3) -> Result<(), PhysicsError> {
        if !force.is_finite() {
            return Err(PhysicsError::InvalidForce);
        }
        crate::invariant_assert!(force.is_finite());
        if let Some(BodyEntry {
            actor: BodyActor::Dynamic(actor),
            ..
        }) = self.bodies.get_mut(&handle)
        {
            // The actor is alive in this scene and borrowed exclusively through the world.
            unsafe {
                actor.as_mut().add_force(
                    &PxVec3::new(force.x, force.y, force.z),
                    ForceMode::Force,
                    true,
                )
            };
            return Ok(());
        }
        Err(PhysicsError::BodyNotFound)
    }

    /// Returns a copyable collider reference that components can retain.
    pub fn collider(&self, handle: BodyHandle) -> Option<Collider> {
        self.bodies.get(&handle).map(|body| body.collider)
    }

    /// Removes the body and its collider. Stale and foreign handles are rejected.
    pub fn remove_body(&mut self, handle: BodyHandle) -> Result<(), PhysicsError> {
        let body = self
            .bodies
            .remove(&handle)
            .ok_or(PhysicsError::BodyNotFound)?;
        // Only actors created by this world are stored here. Remove from the scene
        // before releasing them, while the entry still owns the material.
        unsafe {
            match body.actor {
                BodyActor::Static(mut actor) => {
                    self.scene.remove_actor(actor.as_mut(), true);
                    std::ptr::drop_in_place(actor.as_ptr());
                }
                BodyActor::Dynamic(mut actor) => {
                    self.scene.remove_actor(actor.as_mut(), true);
                    std::ptr::drop_in_place(actor.as_ptr());
                }
            }
        }
        drop(body);
        Ok(())
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
        let mut actor = self
            .physics
            .borrow_mut()
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

        let actor_ptr = NonNull::from(&mut *actor);
        self.scene.add_static_actor(actor);
        let _previous = self.bodies.insert(
            handle,
            BodyEntry {
                actor: BodyActor::Static(actor_ptr),
                collider: Collider {
                    body: handle,
                    descriptor: collider,
                },
                _material: material,
            },
        );
        crate::invariant_assert!(_previous.is_none(), "body handles must be unique");
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
            .borrow_mut()
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

        let actor_ptr = NonNull::from(&mut *actor);
        self.scene.add_dynamic_actor(actor);
        let _previous = self.bodies.insert(
            handle,
            BodyEntry {
                actor: BodyActor::Dynamic(actor_ptr),
                collider: Collider {
                    body: handle,
                    descriptor: collider,
                },
                _material: material,
            },
        );
        crate::invariant_assert!(_previous.is_none(), "body handles must be unique");
        crate::invariant_assert!(
            self.pose(handle).is_some(),
            "new dynamic body is missing from its scene"
        );
        Ok(handle)
    }

    pub(crate) fn create_material(
        &mut self,
        material: ColliderMaterial,
    ) -> Result<Owner<NativeMaterial>, PhysicsError> {
        self.physics
            .borrow_mut()
            .physics
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
