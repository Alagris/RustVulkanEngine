use glam::{Quat, Vec3};

use crate::{
    CapsuleAxis, ColliderDesc, ColliderGeometry, ColliderMaterial, PhysicsError, PhysicsWorld,
    Pose, RigidBodyDesc,
};

#[test]
fn collider_dimensions_must_be_finite_and_positive() {
    assert_eq!(
        ColliderGeometry::sphere(f32::NAN),
        Err(PhysicsError::InvalidSphereRadius)
    );
    assert_eq!(
        ColliderGeometry::cuboid(Vec3::new(1.0, 0.0, 1.0)),
        Err(PhysicsError::InvalidBoxHalfExtents)
    );
    assert_eq!(
        ColliderGeometry::capsule(1.0, f32::INFINITY, CapsuleAxis::Y),
        Err(PhysicsError::InvalidCapsuleDimensions)
    );
}

#[test]
fn pose_normalizes_rotation_and_rejects_zero_quaternion() {
    let pose = Pose::new(Vec3::ONE, Quat::from_xyzw(0.0, 0.0, 0.0, 2.0)).unwrap();
    assert_eq!(pose.rotation(), Quat::IDENTITY);
    assert_eq!(
        Pose::new(Vec3::ZERO, Quat::from_xyzw(0.0, 0.0, 0.0, 0.0)),
        Err(PhysicsError::InvalidPose)
    );
}

#[test]
fn material_coefficients_are_validated() {
    assert!(ColliderMaterial::new(0.2, 0.1, 1.0).is_ok());
    assert_eq!(
        ColliderMaterial::new(-0.1, 0.2, 0.0),
        Err(PhysicsError::InvalidMaterial)
    );
    assert_eq!(
        ColliderMaterial::new(0.2, 0.1, 1.1),
        Err(PhysicsError::InvalidMaterial)
    );
}

#[test]
fn dynamic_descriptors_reject_planes_and_invalid_mass() {
    let collider = ColliderDesc::new(ColliderGeometry::plane(), ColliderMaterial::default());
    assert_eq!(
        RigidBodyDesc::new(Pose::IDENTITY, 1.0, collider),
        Err(PhysicsError::DynamicPlaneCollider)
    );

    let collider = ColliderDesc::new(
        ColliderGeometry::sphere(1.0).unwrap(),
        ColliderMaterial::default(),
    );
    assert_eq!(
        RigidBodyDesc::new(Pose::IDENTITY, 0.0, collider),
        Err(PhysicsError::InvalidMass)
    );
}

#[test]
fn world_creates_and_steps_a_dynamic_collider_and_releases_native_memory() {
    let memory_before = physx::foundation::memory_tracking::snapshot();
    let mut world = PhysicsWorld::new(Vec3::new(0.0, -9.81, 0.0)).unwrap();
    let collider = ColliderDesc::new(
        ColliderGeometry::sphere(0.5).unwrap(),
        ColliderMaterial::default(),
    );
    let descriptor = RigidBodyDesc::new(
        Pose::new(Vec3::new(0.0, 10.0, 0.0), Quat::IDENTITY).unwrap(),
        1.0,
        collider,
    )
    .unwrap();
    let body = world.create_dynamic_body(descriptor).unwrap();
    let initial_height = world.pose(body).unwrap().translation().y;

    for _ in 0..5 {
        world.step(1.0 / 60.0).unwrap();
    }

    let final_height = world.pose(body).unwrap().translation().y;
    assert!(final_height < initial_height);
    drop(world);

    let memory_after = physx::foundation::memory_tracking::snapshot();
    assert!(memory_after.allocations > memory_before.allocations);
    assert_eq!(
        memory_after.allocations - memory_before.allocations,
        memory_after.deallocations - memory_before.deallocations,
        "PhysX allocator callback allocation count did not balance"
    );
    assert_eq!(
        memory_after.bytes_allocated - memory_before.bytes_allocated,
        memory_after.bytes_deallocated - memory_before.bytes_deallocated,
        "PhysX allocator callback bytes did not balance"
    );
    assert_eq!(
        memory_after.live_allocations,
        memory_before.live_allocations
    );
    assert_eq!(memory_after.live_bytes, memory_before.live_bytes);
    assert_eq!(
        memory_after.invalid_deallocations,
        memory_before.invalid_deallocations
    );
    assert_eq!(
        memory_after.duplicate_allocations,
        memory_before.duplicate_allocations
    );
    physx::foundation::memory_tracking::assert_no_errors();
}
