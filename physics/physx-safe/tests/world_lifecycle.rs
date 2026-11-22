use glam::{Quat, Vec3};
use physx_safe::{
    CapsuleAxis, CapsuleControllerDesc, ColliderDesc, ColliderGeometry, ColliderMaterial,
    PhysicsError, PhysicsWorld, Pose, RigidBodyDesc,
};
use std::sync::Mutex;

// The shared foundation is deliberately bound to one thread at a time.
static WORLD_LOCK: Mutex<()> = Mutex::new(());

fn pose(position: Vec3) -> Pose {
    Pose::new(position, Quat::IDENTITY).unwrap()
}

fn collider(geometry: ColliderGeometry) -> ColliderDesc {
    ColliderDesc::new(geometry, ColliderMaterial::default())
}

fn controller(position: Vec3) -> CapsuleControllerDesc {
    CapsuleControllerDesc::new(position, 0.5, 1.0, 0.1, ColliderMaterial::default()).unwrap()
}

#[test]
fn static_and_dynamic_colliders_can_be_inspected_moved_and_removed() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut world = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let desc = collider(ColliderGeometry::sphere(1.0).unwrap()).with_local_pose(pose(Vec3::Y));
    let static_body = world.create_static_collider(pose(Vec3::X), desc).unwrap();
    let dynamic_body = world
        .create_dynamic_body(RigidBodyDesc::new(pose(Vec3::Y), 1.0, desc).unwrap())
        .unwrap();
    let stored = world.collider(static_body).unwrap();
    assert_eq!(stored.body(), static_body);
    assert_eq!(stored.descriptor(), desc);
    assert_eq!(stored.descriptor().local_pose(), pose(Vec3::Y));
    assert_eq!(world.pose(static_body), Some(pose(Vec3::X)));
    assert!(world.set_pose(static_body, pose(Vec3::Z)));
    assert_eq!(world.pose(static_body), Some(pose(Vec3::Z)));
    assert!(world.set_pose(dynamic_body, pose(Vec3::X * 2.0)));
    assert_eq!(world.pose(dynamic_body), Some(pose(Vec3::X * 2.0)));
    world.remove_body(static_body).unwrap();
    world.remove_body(dynamic_body).unwrap();
    assert!(world.collider(static_body).is_none());
    assert!(world.pose(dynamic_body).is_none());
    assert!(!world.set_pose(static_body, Pose::IDENTITY));
    assert_eq!(
        world.remove_body(static_body),
        Err(PhysicsError::BodyNotFound)
    );
    assert_eq!(
        world.apply_force(dynamic_body, Vec3::X),
        Err(PhysicsError::BodyNotFound)
    );
    world.step(1.0 / 60.0).unwrap();
}

#[test]
fn collider_handles_do_not_cross_worlds_or_become_valid_again() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut first = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let mut second = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let desc = collider(ColliderGeometry::cuboid(Vec3::ONE).unwrap());
    let old = first.create_static_collider(Pose::IDENTITY, desc).unwrap();
    assert_eq!(second.remove_body(old), Err(PhysicsError::BodyNotFound));
    first.remove_body(old).unwrap();
    let new = first.create_static_collider(Pose::IDENTITY, desc).unwrap();
    assert_ne!(old, new);
    assert_eq!(first.remove_body(old), Err(PhysicsError::BodyNotFound));
    assert!(first.collider(new).is_some());
    drop(first);
    assert_eq!(second.remove_body(new), Err(PhysicsError::BodyNotFound));
    second.create_static_collider(Pose::IDENTITY, desc).unwrap();
    second.step(1.0 / 60.0).unwrap();
}

#[test]
fn capsule_controller_moves_and_reports_floor_and_wall_contacts() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut world = PhysicsWorld::new(Vec3::ZERO).unwrap();
    world
        .create_static_collider(
            pose(Vec3::new(0.0, -0.5, 0.0)),
            collider(ColliderGeometry::cuboid(Vec3::new(10.0, 0.5, 10.0)).unwrap()),
        )
        .unwrap();
    world
        .create_static_collider(
            pose(Vec3::new(2.0, 2.0, 0.0)),
            collider(ColliderGeometry::cuboid(Vec3::new(0.5, 2.0, 5.0)).unwrap()),
        )
        .unwrap();
    let c = world
        .create_capsule_controller(controller(Vec3::new(0.0, 3.0, 0.0)))
        .unwrap();
    let down = world
        .move_controller(c.handle(), Vec3::new(0.0, -5.0, 0.0), 0.0, 1.0 / 60.0)
        .unwrap();
    assert!(down.down, "expected floor contact: {down:?}");
    let pos = world.controller_position(c.handle()).unwrap();
    assert!(
        pos.y > 0.9 && pos.y < 1.5,
        "unexpected floor height: {pos:?}"
    );
    let right = world
        .move_controller(c.handle(), Vec3::new(5.0, 0.0, 0.0), 0.0, 1.0 / 60.0)
        .unwrap();
    assert!(right.sides, "expected wall contact: {right:?}");
    assert!(world.controller_position(c.handle()).unwrap().x < 1.1);
    world.step(1.0 / 60.0).unwrap();
}

#[test]
fn controllers_and_regular_actors_coexist_and_can_be_removed_in_any_order() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut world = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let c1 = world
        .create_capsule_controller(controller(Vec3::new(0.0, 3.0, 0.0)))
        .unwrap();
    let c2 = world
        .create_capsule_controller(controller(Vec3::new(5.0, 3.0, 0.0)))
        .unwrap();
    let body = world
        .create_dynamic_body(
            RigidBodyDesc::new(
                Pose::IDENTITY,
                1.0,
                collider(ColliderGeometry::capsule(0.5, 0.5, CapsuleAxis::Y).unwrap()),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(world.capsule_controller(c1.handle()), Some(c1));
    assert_eq!(world.pose(body), Some(Pose::IDENTITY));
    world.apply_force(body, Vec3::X).unwrap();
    world.remove_controller(c1.handle()).unwrap();
    assert_eq!(
        world.controller_position(c1.handle()),
        Err(PhysicsError::ControllerNotFound)
    );
    assert_eq!(
        world.remove_controller(c1.handle()),
        Err(PhysicsError::ControllerNotFound)
    );
    world
        .set_controller_position(c2.handle(), Vec3::new(6.0, 4.0, 0.0))
        .unwrap();
    assert_eq!(
        world.controller_position(c2.handle()).unwrap(),
        Vec3::new(6.0, 4.0, 0.0)
    );
    world.remove_body(body).unwrap();
    world.remove_controller(c2.handle()).unwrap();
    world.step(1.0 / 60.0).unwrap();
}

#[test]
fn controller_rejects_invalid_descriptors_movement_and_foreign_handles() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let material = ColliderMaterial::default();
    for (radius, height, step) in [
        (0.0, 1.0, 0.0),
        (1.0, -1.0, 0.0),
        (f32::NAN, 1.0, 0.0),
        (1.0, 1.0, -1.0),
        (1.0, 1.0, 4.0),
    ] {
        assert_eq!(
            CapsuleControllerDesc::new(Vec3::ZERO, radius, height, step, material),
            Err(PhysicsError::InvalidControllerDimensions)
        );
    }
    let mut first = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let mut second = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let c = first
        .create_capsule_controller(controller(Vec3::ZERO))
        .unwrap();
    assert_eq!(
        second.controller_position(c.handle()),
        Err(PhysicsError::ControllerNotFound)
    );
    assert_eq!(
        second.remove_controller(c.handle()),
        Err(PhysicsError::ControllerNotFound)
    );
    assert_eq!(
        first.set_controller_position(c.handle(), Vec3::splat(f32::NAN)),
        Err(PhysicsError::InvalidPose)
    );
    assert_eq!(
        first.move_controller(c.handle(), Vec3::splat(f32::INFINITY), 0.0, 0.1),
        Err(PhysicsError::InvalidControllerMovement)
    );
    assert_eq!(
        first.move_controller(c.handle(), Vec3::ZERO, -1.0, 0.1),
        Err(PhysicsError::InvalidControllerMovement)
    );
    assert_eq!(
        first.move_controller(c.handle(), Vec3::ZERO, 0.0, 0.0),
        Err(PhysicsError::InvalidTimeStep)
    );
    let invalid = CapsuleControllerDesc {
        radius: -1.0,
        ..controller(Vec3::ZERO)
    };
    assert_eq!(
        first.create_capsule_controller(invalid),
        Err(PhysicsError::InvalidControllerDimensions)
    );
    drop(first);
    assert_eq!(
        second.controller_position(c.handle()),
        Err(PhysicsError::ControllerNotFound)
    );
    assert_eq!(
        second.remove_controller(c.handle()),
        Err(PhysicsError::ControllerNotFound)
    );
}

#[test]
fn foundation_is_shared_on_one_thread_and_released_after_the_last_world() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let first = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let mut second = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let conflict = std::thread::spawn(|| PhysicsWorld::new(Vec3::ZERO).err())
        .join()
        .unwrap();
    assert_eq!(conflict, Some(PhysicsError::FoundationOnAnotherThread));
    drop(first);
    second.step(0.01).unwrap();
    drop(second);
    assert!(
        std::thread::spawn(|| PhysicsWorld::new(Vec3::ZERO).is_ok())
            .join()
            .unwrap()
    );
}

#[test]
fn controller_reports_ceiling_contact_and_unobstructed_motion() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut world = PhysicsWorld::new(Vec3::ZERO).unwrap();
    world
        .create_static_collider(
            pose(Vec3::new(0.0, 4.0, 0.0)),
            collider(ColliderGeometry::cuboid(Vec3::new(5.0, 0.5, 5.0)).unwrap()),
        )
        .unwrap();
    let c = world
        .create_capsule_controller(controller(Vec3::ZERO))
        .unwrap();
    let clear = world
        .move_controller(c.handle(), Vec3::X, 0.0, 0.1)
        .unwrap();
    assert!(!clear.sides && !clear.up && !clear.down);
    let up = world
        .move_controller(c.handle(), Vec3::Y * 10.0, 0.0, 0.1)
        .unwrap();
    assert!(up.up, "expected ceiling contact: {up:?}");
}

#[test]
fn repeated_controller_and_body_removal_leaves_world_usable() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut world = PhysicsWorld::new(Vec3::ZERO).unwrap();
    for i in 0..50 {
        let body = world
            .create_static_collider(
                pose(Vec3::X * i as f32),
                collider(ColliderGeometry::sphere(0.5).unwrap()),
            )
            .unwrap();
        let c = world
            .create_capsule_controller(controller(Vec3::Y * 5.0))
            .unwrap();
        world
            .move_controller(c.handle(), Vec3::X, 0.0, 0.1)
            .unwrap();
        world.remove_body(body).unwrap();
        world.remove_controller(c.handle()).unwrap();
    }
    let c = world
        .create_capsule_controller(controller(Vec3::Y))
        .unwrap();
    assert_eq!(world.controller_position(c.handle()).unwrap(), Vec3::Y);
}

#[test]
fn dynamic_sphere_lands_on_static_box() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut world = PhysicsWorld::new(Vec3::new(0.0, -9.81, 0.0)).unwrap();
    world
        .create_static_collider(
            pose(Vec3::new(0.0, -0.5, 0.0)),
            collider(ColliderGeometry::cuboid(Vec3::new(10.0, 0.5, 10.0)).unwrap()),
        )
        .unwrap();
    let sphere = world
        .create_dynamic_body(
            RigidBodyDesc::new(
                pose(Vec3::Y * 4.0),
                1.0,
                collider(ColliderGeometry::sphere(0.5).unwrap()),
            )
            .unwrap(),
        )
        .unwrap();
    for _ in 0..240 {
        world.step(1.0 / 60.0).unwrap();
    }
    let y = world.pose(sphere).unwrap().translation().y;
    assert!(
        (0.45..0.6).contains(&y),
        "sphere did not land on the floor: {y}"
    );
}

#[test]
fn each_supported_static_geometry_and_capsule_axis_can_be_created() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut world = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let geometries = [
        ColliderGeometry::sphere(0.5).unwrap(),
        ColliderGeometry::cuboid(Vec3::ONE).unwrap(),
        ColliderGeometry::plane(),
        ColliderGeometry::capsule(0.5, 1.0, CapsuleAxis::X).unwrap(),
        ColliderGeometry::capsule(0.5, 1.0, CapsuleAxis::Y).unwrap(),
        ColliderGeometry::capsule(0.5, 1.0, CapsuleAxis::Z).unwrap(),
    ];
    for (index, geometry) in geometries.into_iter().enumerate() {
        let desc = collider(geometry);
        let body = world
            .create_static_collider(pose(Vec3::X * index as f32 * 5.0), desc)
            .unwrap();
        assert_eq!(
            world.collider(body).unwrap().descriptor().geometry(),
            geometry
        );
        world.remove_body(body).unwrap();
    }
}

#[test]
fn invalid_operations_leave_the_world_usable() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut world = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let body = world
        .create_static_collider(
            Pose::IDENTITY,
            collider(ColliderGeometry::sphere(1.0).unwrap()),
        )
        .unwrap();
    for time in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(world.step(time), Err(PhysicsError::InvalidTimeStep));
    }
    assert_eq!(
        world.apply_force(body, Vec3::ZERO),
        Err(PhysicsError::BodyNotFound)
    );
    assert_eq!(
        world.apply_force(body, Vec3::splat(f32::NAN)),
        Err(PhysicsError::InvalidForce)
    );
    assert_eq!(world.pose(body), Some(Pose::IDENTITY));
    world.step(0.1).unwrap();
}

#[test]
fn controllers_in_different_scenes_do_not_interact() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut first = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let mut second = PhysicsWorld::new(Vec3::ZERO).unwrap();
    first
        .create_static_collider(
            pose(Vec3::new(2.0, 0.0, 0.0)),
            collider(ColliderGeometry::cuboid(Vec3::ONE).unwrap()),
        )
        .unwrap();
    let one = first
        .create_capsule_controller(controller(Vec3::ZERO))
        .unwrap();
    let two = second
        .create_capsule_controller(controller(Vec3::ZERO))
        .unwrap();
    assert!(
        first
            .move_controller(one.handle(), Vec3::X * 5.0, 0.0, 0.1)
            .unwrap()
            .sides
    );
    assert!(
        !second
            .move_controller(two.handle(), Vec3::X * 5.0, 0.0, 0.1)
            .unwrap()
            .sides
    );
    assert!(second.controller_position(two.handle()).unwrap().x > 4.9);
}

#[test]
fn controller_honors_minimum_distance_and_can_teleport() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut world = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let c = world
        .create_capsule_controller(controller(Vec3::Y * 2.0))
        .unwrap();
    let result = world
        .move_controller(c.handle(), Vec3::X * 0.1, 0.2, 0.1)
        .unwrap();
    assert!(!result.sides && !result.up && !result.down);
    assert_eq!(
        world.controller_position(c.handle()).unwrap(),
        Vec3::Y * 2.0
    );
    world
        .set_controller_position(c.handle(), Vec3::new(4.0, 5.0, 6.0))
        .unwrap();
    assert_eq!(
        world.controller_position(c.handle()).unwrap(),
        Vec3::new(4.0, 5.0, 6.0)
    );
}

#[test]
fn controller_validates_time_and_distance_without_changing_position() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut world = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let c = world
        .create_capsule_controller(controller(Vec3::Y))
        .unwrap();
    for time in [-1.0, 0.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            world.move_controller(c.handle(), Vec3::X, 0.0, time),
            Err(PhysicsError::InvalidTimeStep)
        );
    }

    for distance in [-1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            world.move_controller(c.handle(), Vec3::X, distance, 0.1),
            Err(PhysicsError::InvalidControllerMovement)
        );
    }
    assert_eq!(world.controller_position(c.handle()).unwrap(), Vec3::Y);
}

#[test]
fn controller_blocks_against_another_controller() {
    let _guard = WORLD_LOCK.lock().unwrap();
    let mut world = PhysicsWorld::new(Vec3::ZERO).unwrap();
    let first = world
        .create_capsule_controller(controller(Vec3::ZERO))
        .unwrap();
    let second = world
        .create_capsule_controller(controller(Vec3::X * 3.0))
        .unwrap();
    let result = world
        .move_controller(first.handle(), Vec3::X * 5.0, 0.0, 0.1)
        .unwrap();
    assert!(result.sides, "expected controller collision: {result:?}");
    assert!(world.controller_position(first.handle()).unwrap().x < 2.0);
    assert_eq!(
        world.controller_position(second.handle()).unwrap(),
        Vec3::X * 3.0
    );
}
