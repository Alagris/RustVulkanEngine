use glam::Vec3;
use physx_safe::{CapsuleAxis, ColliderGeometry, GeometryKind, PhysicsError};

#[test]
fn scales_boxes_spheres_and_capsules_in_their_local_axes() {
    let sphere = ColliderGeometry::sphere(2.0)
        .unwrap()
        .scaled(Vec3::splat(3.0))
        .unwrap();
    assert_eq!(sphere.kind(), GeometryKind::Sphere { radius: 6.0 });
    let cuboid = ColliderGeometry::cuboid(Vec3::new(1.0, 2.0, 3.0))
        .unwrap()
        .scaled(Vec3::new(2.0, 3.0, 4.0))
        .unwrap();
    assert_eq!(
        cuboid.kind(),
        GeometryKind::Box {
            half_extents: Vec3::new(2.0, 6.0, 12.0)
        }
    );
    for (axis, scale) in [
        (CapsuleAxis::X, Vec3::new(3.0, 2.0, 2.0)),
        (CapsuleAxis::Y, Vec3::new(2.0, 3.0, 2.0)),
        (CapsuleAxis::Z, Vec3::new(2.0, 2.0, 3.0)),
    ] {
        let capsule = ColliderGeometry::capsule(1.0, 2.0, axis)
            .unwrap()
            .scaled(scale)
            .unwrap();
        assert_eq!(
            capsule.kind(),
            GeometryKind::Capsule {
                radius: 2.0,
                half_height: 6.0,
                axis
            }
        );
    }
}

#[test]
fn rejects_invalid_or_unrepresentable_scales_and_overflow() {
    let sphere = ColliderGeometry::sphere(1.0).unwrap();
    for scale in [
        Vec3::ZERO,
        Vec3::splat(-1.0),
        Vec3::splat(f32::NAN),
        Vec3::splat(f32::INFINITY),
    ] {
        assert_eq!(sphere.scaled(scale), Err(PhysicsError::InvalidScale));
    }

    assert_eq!(
        sphere.scaled(Vec3::new(1.0, 2.0, 1.0)),
        Err(PhysicsError::UnsupportedScale)
    );
    let capsule = ColliderGeometry::capsule(1.0, 1.0, CapsuleAxis::Y).unwrap();
    assert_eq!(
        capsule.scaled(Vec3::new(1.0, 1.0, 2.0)),
        Err(PhysicsError::UnsupportedScale)
    );
    assert_eq!(
        ColliderGeometry::sphere(f32::MAX)
            .unwrap()
            .scaled(Vec3::splat(2.0)),
        Err(PhysicsError::InvalidSphereRadius)
    );
    assert_eq!(
        ColliderGeometry::plane()
            .scaled(Vec3::new(1.0, 2.0, 3.0))
            .unwrap()
            .kind(),
        GeometryKind::Plane
    );
}

#[test]
fn rejects_radial_mismatch_for_every_capsule_axis() {
    for (axis, scale) in [
        (CapsuleAxis::X, Vec3::new(1.0, 2.0, 3.0)),
        (CapsuleAxis::Y, Vec3::new(2.0, 1.0, 3.0)),
        (CapsuleAxis::Z, Vec3::new(2.0, 3.0, 1.0)),
    ] {
        assert_eq!(
            ColliderGeometry::capsule(1.0, 1.0, axis)
                .unwrap()
                .scaled(scale),
            Err(PhysicsError::UnsupportedScale)
        );
    }
}

#[test]
fn rejects_overflowing_box_and_capsule_dimensions_after_scaling() {
    let cube = ColliderGeometry::cuboid(Vec3::splat(f32::MAX)).unwrap();
    assert_eq!(
        cube.scaled(Vec3::splat(2.0)),
        Err(PhysicsError::InvalidBoxHalfExtents)
    );
    let capsule = ColliderGeometry::capsule(1.0, f32::MAX, CapsuleAxis::Y).unwrap();
    assert_eq!(
        capsule.scaled(Vec3::new(1.0, 2.0, 1.0)),
        Err(PhysicsError::InvalidCapsuleDimensions)
    );
}
