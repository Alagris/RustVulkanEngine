pub mod animator;
pub mod camera;
pub mod component;
pub mod directional_light;
pub mod game_object;
mod game_object_serde;
pub mod instanced_mesh;
pub mod mesh;
pub(crate) mod mesh_serde;
pub mod procedural_mesh;
pub mod scene;
mod scene_serde;
pub mod skinned_mesh;

pub use crate::anim::AnimationClip;
pub use animator::Animator;
pub use camera::Camera;
pub use component::{
    Component, ComponentFactory, ComponentInstance, ComponentMetadata, ComponentRegistration,
    ComponentRegistry, ComponentSerializer, ComponentTypeInfo, ComponentWeak, Unique,
};
pub use component_macros::{Component, component};
pub use directional_light::DirectionalLight;
pub use game_object::{GameObject, GameObjectWeak};
pub use instanced_mesh::InstancedMesh;
pub use mesh::Mesh;
pub use procedural_mesh::ProceduralMesh;
pub use scene::{Prefab, Scene};
pub use skinned_mesh::{Bone, SkinnedMesh};
