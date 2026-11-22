use glam::Vec3;

use crate::{error::PhysicsError, pose::Pose};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapsuleAxis {
    X,
    Y,
    Z,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GeometryKind {
    Sphere {
        radius: f32,
    },
    Box {
        half_extents: Vec3,
    },
    Capsule {
        radius: f32,
        half_height: f32,
        axis: CapsuleAxis,
    },
    Plane,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColliderGeometry(pub(crate) GeometryKind);

impl ColliderGeometry {
    pub fn kind(self) -> GeometryKind {
        self.0
    }

    /// Scale dimensions in collider-local axes. Spheres require uniform scale;
    /// capsules require equal radial scale on their two perpendicular axes.
    pub fn scaled(self, scale: Vec3) -> Result<Self, PhysicsError> {
        if !scale.is_finite() || scale.min_element() <= 0.0 {
            return Err(PhysicsError::InvalidScale);
        }
        match self.0 {
            GeometryKind::Sphere { radius } => {
                if scale.x != scale.y || scale.x != scale.z {
                    return Err(PhysicsError::UnsupportedScale);
                }
                Self::sphere(radius * scale.x)
            }
            GeometryKind::Box { half_extents } => Self::cuboid(half_extents * scale),
            GeometryKind::Capsule {
                radius,
                half_height,
                axis,
            } => {
                let (axial, radial1, radial2) = match axis {
                    CapsuleAxis::X => (scale.x, scale.y, scale.z),
                    CapsuleAxis::Y => (scale.y, scale.x, scale.z),
                    CapsuleAxis::Z => (scale.z, scale.x, scale.y),
                };
                if radial1 != radial2 {
                    return Err(PhysicsError::UnsupportedScale);
                }
                Self::capsule(radius * radial1, half_height * axial, axis)
            }
            GeometryKind::Plane => Ok(self),
        }
    }

    pub fn sphere(radius: f32) -> Result<Self, PhysicsError> {
        if !radius.is_finite() || radius <= 0.0 {
            return Err(PhysicsError::InvalidSphereRadius);
        }
        crate::invariant_assert!(radius.is_finite() && radius > 0.0);
        Ok(Self(GeometryKind::Sphere { radius }))
    }

    pub fn cuboid(half_extents: Vec3) -> Result<Self, PhysicsError> {
        if !half_extents.is_finite() || half_extents.min_element() <= 0.0 {
            return Err(PhysicsError::InvalidBoxHalfExtents);
        }
        crate::invariant_assert!(half_extents.is_finite() && half_extents.min_element() > 0.0);
        Ok(Self(GeometryKind::Box { half_extents }))
    }

    pub fn capsule(radius: f32, half_height: f32, axis: CapsuleAxis) -> Result<Self, PhysicsError> {
        if !radius.is_finite() || !half_height.is_finite() || radius <= 0.0 || half_height <= 0.0 {
            return Err(PhysicsError::InvalidCapsuleDimensions);
        }
        crate::invariant_assert!(
            radius.is_finite() && radius > 0.0 && half_height.is_finite() && half_height > 0.0
        );
        Ok(Self(GeometryKind::Capsule {
            radius,
            half_height,
            axis,
        }))
    }

    pub fn plane() -> Self {
        Self(GeometryKind::Plane)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColliderMaterial {
    pub(crate) static_friction: f32,
    pub(crate) dynamic_friction: f32,
    pub(crate) restitution: f32,
}

impl ColliderMaterial {
    pub fn new(
        static_friction: f32,
        dynamic_friction: f32,
        restitution: f32,
    ) -> Result<Self, PhysicsError> {
        if !static_friction.is_finite()
            || !dynamic_friction.is_finite()
            || !restitution.is_finite()
            || static_friction < 0.0
            || dynamic_friction < 0.0
            || !(0.0..=1.0).contains(&restitution)
        {
            return Err(PhysicsError::InvalidMaterial);
        }
        crate::invariant_assert!(
            static_friction.is_finite()
                && dynamic_friction.is_finite()
                && restitution.is_finite()
                && static_friction >= 0.0
                && dynamic_friction >= 0.0
                && (0.0..=1.0).contains(&restitution)
        );
        Ok(Self {
            static_friction,
            dynamic_friction,
            restitution,
        })
    }
}

impl Default for ColliderMaterial {
    fn default() -> Self {
        Self {
            static_friction: 0.5,
            dynamic_friction: 0.5,
            restitution: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColliderDesc {
    pub(crate) geometry: ColliderGeometry,
    pub(crate) material: ColliderMaterial,
    pub(crate) local_pose: Pose,
}

impl ColliderDesc {
    pub fn new(geometry: ColliderGeometry, material: ColliderMaterial) -> Self {
        Self {
            geometry,
            material,
            local_pose: Pose::IDENTITY,
        }
    }

    pub fn with_local_pose(mut self, local_pose: Pose) -> Self {
        self.local_pose = local_pose;
        self
    }

    pub fn geometry(self) -> ColliderGeometry {
        self.geometry
    }

    pub fn material(self) -> ColliderMaterial {
        self.material
    }

    pub fn local_pose(self) -> Pose {
        self.local_pose
    }
}

/// A copyable reference to a collider owned by a particular physics world.
///
/// The handle becomes invalid when the body is removed or its world is dropped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Collider {
    pub(crate) body: crate::BodyHandle,
    pub(crate) descriptor: ColliderDesc,
}

impl Collider {
    pub fn body(self) -> crate::BodyHandle {
        self.body
    }

    pub fn descriptor(self) -> ColliderDesc {
        self.descriptor
    }
}
