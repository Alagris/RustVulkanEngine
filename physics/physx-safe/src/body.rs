use std::sync::atomic::{AtomicU64, Ordering};

use crate::{
    collider::{ColliderDesc, GeometryKind},
    error::PhysicsError,
    pose::Pose,
};

static NEXT_BODY_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyHandle(pub(crate) u64);

pub(crate) struct BodyTag {
    pub(crate) handle: BodyHandle,
    _padding: [usize; 2],
}

impl BodyTag {
    pub(crate) fn new(handle: BodyHandle) -> Self {
        Self {
            handle,
            _padding: [0; 2],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigidBodyDesc {
    pub pose: Pose,
    pub mass: f32,
    pub collider: ColliderDesc,
}

impl RigidBodyDesc {
    pub fn new(pose: Pose, mass: f32, collider: ColliderDesc) -> Result<Self, PhysicsError> {
        if !mass.is_finite() || mass <= 0.0 {
            return Err(PhysicsError::InvalidMass);
        }
        if matches!(collider.geometry.0, GeometryKind::Plane) {
            return Err(PhysicsError::DynamicPlaneCollider);
        }
        crate::invariant_assert!(mass.is_finite() && mass > 0.0);
        crate::invariant_assert!(!matches!(collider.geometry.0, GeometryKind::Plane));
        Ok(Self {
            pose,
            mass,
            collider,
        })
    }
}

pub(crate) fn allocate_body_handle() -> Result<BodyHandle, PhysicsError> {
    let handle = NEXT_BODY_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        })
        .map(BodyHandle)
        .map_err(|_| PhysicsError::BodyHandleExhausted)?;
    crate::invariant_assert!(handle.0 != 0, "body handle zero is reserved");
    Ok(handle)
}
