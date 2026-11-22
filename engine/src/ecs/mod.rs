pub mod animator;
pub mod box_collider;
pub mod camera;
pub mod camera_follow;
pub mod capsule_collider;
pub mod character_controller;
pub mod collider;
pub mod component;
pub mod directional_light;
pub mod game_object;
mod game_object_serde;
pub mod instanced_mesh;
pub mod mesh;
pub(crate) mod mesh_serde;
pub mod plane_collider;
pub mod procedural_mesh;
pub mod scene;
mod scene_serde;
pub mod skinned_mesh;
pub mod sphere_collider;
mod weak_component_list;

pub use crate::anim::AnimationClip;
pub use animator::Animator;
pub use box_collider::BoxCollider;
pub use camera::Camera;
pub use camera_follow::{CameraFollow, CameraFollowMode};
pub use capsule_collider::CapsuleCollider;
pub use character_controller::CharacterController;
pub use collider::{
    BoxShape, Capsule, Collider, ColliderPrimitive, ColliderSettings, NativeCollider,
    PhysicsCollider, Plane, Sphere,
};
pub use component::{
    Component, ComponentFactory, ComponentInstance, ComponentMetadata, ComponentRegistration,
    ComponentRegistry, ComponentSerializer, ComponentTypeInfo, ComponentWeak, Features, Unique,
};
pub use component_macros::{Component, component};
pub use directional_light::DirectionalLight;
pub use game_object::{GameObject, GameObjectWeak};
pub use game_object_serde::TransformDefinition;
pub use instanced_mesh::InstancedMesh;
pub use mesh::Mesh;
pub use plane_collider::PlaneCollider;
pub use procedural_mesh::ProceduralMesh;
pub use scene::Scene;
pub use skinned_mesh::{Bone, SkinnedMesh};
pub use sphere_collider::SphereCollider;
