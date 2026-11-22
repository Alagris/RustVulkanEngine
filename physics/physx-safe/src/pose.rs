use glam::{Quat, Vec3};
use physx::math::{PxQuat, PxTransform, PxVec3};

use crate::error::PhysicsError;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    translation: Vec3,
    rotation: Quat,
}

impl Pose {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
    };

    pub fn new(translation: Vec3, rotation: Quat) -> Result<Self, PhysicsError> {
        if !translation.is_finite() || !rotation.is_finite() {
            return Err(PhysicsError::InvalidPose);
        }
        let rotation_length_squared = rotation.length_squared();
        if !rotation_length_squared.is_finite() || rotation_length_squared <= f32::EPSILON {
            return Err(PhysicsError::InvalidPose);
        }

        let pose = Self {
            translation,
            rotation: rotation.normalize(),
        };
        crate::invariant_assert!(pose.translation.is_finite());
        crate::invariant_assert!(
            (pose.rotation.length_squared() - 1.0).abs() <= 1.0e-5,
            "pose rotation must be normalized"
        );
        Ok(pose)
    }

    pub fn translation(self) -> Vec3 {
        self.translation
    }

    pub fn rotation(self) -> Quat {
        self.rotation
    }

    pub(crate) fn with_local_rotation(self, rotation: Quat) -> Self {
        Self {
            rotation: self.rotation * rotation,
            ..self
        }
    }

    pub(crate) fn to_native(self) -> PxTransform {
        let translation = PxVec3::new(self.translation.x, self.translation.y, self.translation.z);
        let rotation = PxQuat::new(
            self.rotation.x,
            self.rotation.y,
            self.rotation.z,
            self.rotation.w,
        );
        PxTransform::from_translation_rotation(&translation, &rotation)
    }

    pub(crate) fn from_native(pose: PxTransform) -> Option<Self> {
        let translation = pose.translation();
        let rotation = pose.rotation();
        let pose = Self::new(
            Vec3::new(translation.x(), translation.y(), translation.z()),
            Quat::from_xyzw(rotation.x(), rotation.y(), rotation.z(), rotation.w()),
        );
        crate::invariant_assert!(pose.is_ok(), "PhysX returned an invalid pose");
        pose.ok()
    }
}

impl Default for Pose {
    fn default() -> Self {
        Self::IDENTITY
    }
}
