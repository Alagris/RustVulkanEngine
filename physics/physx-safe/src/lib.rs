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
mod error;
mod native;
mod pose;
mod world;

pub use body::{BodyHandle, RigidBodyDesc};
pub use collider::{CapsuleAxis, ColliderDesc, ColliderGeometry, ColliderMaterial};
pub use error::PhysicsError;
pub use pose::Pose;
pub use world::PhysicsWorld;

#[cfg(test)]
mod tests;
