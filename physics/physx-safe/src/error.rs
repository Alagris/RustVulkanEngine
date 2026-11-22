#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicsError {
    InvalidGravity,
    InvalidPose,
    InvalidSphereRadius,
    InvalidBoxHalfExtents,
    InvalidCapsuleDimensions,
    InvalidMaterial,
    InvalidMass,
    InvalidForce,
    InvalidTimeStep,
    BodyNotFound,
    DynamicPlaneCollider,
    InitializationFailed,
    BodyHandleExhausted,
    NativeObjectCreationFailed,
    SimulationFailed(u32),
}

impl std::fmt::Display for PhysicsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGravity => formatter.write_str("gravity must contain finite values"),
            Self::InvalidPose => formatter.write_str("pose must be finite with a nonzero rotation"),
            Self::InvalidSphereRadius => {
                formatter.write_str("sphere radius must be finite and positive")
            }
            Self::InvalidBoxHalfExtents => {
                formatter.write_str("box half extents must be finite and positive")
            }
            Self::InvalidCapsuleDimensions => {
                formatter.write_str("capsule radius and half-height must be finite and positive")
            }
            Self::InvalidMaterial => formatter
                .write_str("friction must be nonnegative and restitution must be in [0, 1]"),
            Self::InvalidMass => {
                formatter.write_str("dynamic body mass must be finite and positive")
            }
            Self::InvalidForce => formatter.write_str("force must contain finite values"),
            Self::InvalidTimeStep => formatter.write_str("time step must be finite and positive"),
            Self::BodyNotFound => {
                formatter.write_str("the body handle does not exist in this world")
            }
            Self::DynamicPlaneCollider => {
                formatter.write_str("PhysX plane colliders can only be attached to static actors")
            }
            Self::InitializationFailed => formatter.write_str("PhysX initialization failed"),
            Self::BodyHandleExhausted => {
                formatter.write_str("PhysX body handle space is exhausted")
            }
            Self::NativeObjectCreationFailed => {
                formatter.write_str("PhysX failed to create a native object")
            }
            Self::SimulationFailed(code) => {
                write!(formatter, "PhysX simulation failed with error code {code}")
            }
        }
    }
}

impl std::error::Error for PhysicsError {}
