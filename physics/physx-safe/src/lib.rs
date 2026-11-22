#[cfg(any(test, feature = "memory-tracking"))]
macro_rules! invariant_assert {
    ($condition:expr) => {
        assert!($condition);
    };
    ($condition:expr, $($argument:tt)+) => {
        assert!($condition, $($argument)+);
    };
}

#[cfg(not(any(test, feature = "memory-tracking")))]
macro_rules! invariant_assert {
    ($condition:expr) => {};
    ($condition:expr, $($argument:tt)+) => {};
}

pub(crate) use invariant_assert;

mod body;
mod collider;
mod context;
mod controller;
mod controller_world;
mod error;
mod native;
mod pose;
mod world;

pub use body::{BodyHandle, RigidBodyDesc};
pub use collider::{
    CapsuleAxis, Collider, ColliderDesc, ColliderGeometry, ColliderMaterial, GeometryKind,
};
pub use controller::{
    CapsuleController, CapsuleControllerDesc, ControllerHandle, ControllerMoveResult,
};
pub use error::PhysicsError;
pub use pose::Pose;
pub use world::PhysicsWorld;

#[cfg(test)]
mod tests;
