use physx::{
    actor::ActorMap,
    articulation_link::PxArticulationLink,
    articulation_reduced_coordinate::PxArticulationReducedCoordinate,
    material::PxMaterial,
    math::PxTransform,
    physics::PhysicsFoundation,
    prelude::{AdvanceCallback, CollisionCallback, ConstraintBreakCallback, TriggerCallback},
    rigid_body::RigidBodyMap,
    rigid_dynamic::PxRigidDynamic,
    rigid_static::PxRigidStatic,
    scene::PxScene,
    shape::PxShape,
    simulation_event_callback::WakeSleepCallback,
};

use crate::body::BodyTag;

pub(crate) type NativeMaterial = PxMaterial<()>;
pub(crate) type NativeShape = PxShape<(), NativeMaterial>;
pub(crate) type NativeDynamic = PxRigidDynamic<BodyTag, NativeShape>;
pub(crate) type NativeStatic = PxRigidStatic<BodyTag, NativeShape>;
pub(crate) type NativeLink = PxArticulationLink<BodyTag, NativeShape>;
pub(crate) type NativeArticulation = PxArticulationReducedCoordinate<(), NativeLink>;

pub(crate) struct NoCollision;
impl CollisionCallback for NoCollision {
    fn on_collision(
        &mut self,
        _header: &physx_sys::PxContactPairHeader,
        _pairs: &[physx_sys::PxContactPair],
    ) {
    }
}

pub(crate) struct NoTrigger;
impl TriggerCallback for NoTrigger {
    fn on_trigger(&mut self, _pairs: &[physx_sys::PxTriggerPair]) {}
}

pub(crate) struct NoConstraintBreak;
impl ConstraintBreakCallback for NoConstraintBreak {
    fn on_constraint_break(&mut self, _constraints: &[physx_sys::PxConstraintInfo]) {}
}

pub(crate) struct NoWakeSleep;
impl WakeSleepCallback<NativeLink, NativeStatic, NativeDynamic> for NoWakeSleep {
    fn on_wake_sleep(
        &mut self,
        _actors: &[&ActorMap<NativeLink, NativeStatic, NativeDynamic>],
        _is_waking: bool,
    ) {
    }
}

pub(crate) struct NoAdvance;
impl AdvanceCallback<NativeLink, NativeDynamic> for NoAdvance {
    fn on_advance(
        &self,
        _actors: &[&RigidBodyMap<NativeLink, NativeDynamic>],
        _transforms: &[PxTransform],
    ) {
    }
}

pub(crate) type NativeScene = PxScene<
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
>;

#[cfg(not(any(test, feature = "memory-tracking")))]
pub(crate) type NativePhysics = PhysicsFoundation<physx::foundation::DefaultAllocator, NativeShape>;
#[cfg(any(test, feature = "memory-tracking"))]
pub(crate) type NativePhysics =
    PhysicsFoundation<physx::foundation::TrackingAllocator, NativeShape>;
