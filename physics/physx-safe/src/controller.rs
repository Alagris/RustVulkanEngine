use glam::Vec3;

use crate::{ColliderMaterial, PhysicsError};

/// Dimensions use a full cylindrical height, excluding the hemispherical ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapsuleControllerDesc {
    pub position: Vec3,
    pub radius: f32,
    pub height: f32,
    pub step_offset: f32,
    pub material: ColliderMaterial,
}

impl CapsuleControllerDesc {
    pub fn new(
        position: Vec3,
        radius: f32,
        height: f32,
        step_offset: f32,
        material: ColliderMaterial,
    ) -> Result<Self, PhysicsError> {
        let desc = Self {
            position,
            radius,
            height,
            step_offset,
            material,
        };
        desc.validate()?;
        Ok(desc)
    }

    pub(crate) fn validate(&self) -> Result<(), PhysicsError> {
        if !self.position.is_finite() {
            return Err(PhysicsError::InvalidPose);
        }
        if !self.radius.is_finite()
            || self.radius <= 0.0
            || !self.height.is_finite()
            || self.height <= 0.0
            || !self.step_offset.is_finite()
            || self.step_offset < 0.0
            || !(self.height + 2.0 * self.radius).is_finite()
            || self.step_offset > self.height + 2.0 * self.radius
        {
            return Err(PhysicsError::InvalidControllerDimensions);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControllerHandle(pub(crate) crate::BodyHandle);

/// Copyable reference to a world-owned PhysX capsule controller.
///
/// The descriptor is the creation-time snapshot; query the world for the current position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapsuleController {
    pub(crate) handle: ControllerHandle,
    pub(crate) descriptor: CapsuleControllerDesc,
}

impl CapsuleController {
    pub fn handle(self) -> ControllerHandle {
        self.handle
    }

    pub fn descriptor(self) -> CapsuleControllerDesc {
        self.descriptor
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerMoveResult {
    pub sides: bool,
    pub up: bool,
    pub down: bool,
}
