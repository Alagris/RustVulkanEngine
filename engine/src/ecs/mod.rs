pub mod animator;
pub mod camera;
pub mod component;
pub mod game_object;
pub mod mesh;
pub mod scene;
pub mod skinned_mesh;

pub use crate::anim::AnimationClip;
pub use animator::Animator;
pub use camera::Camera;
pub use component::{
    Component, ComponentInstance, ComponentMetadata, ComponentRegistry, ComponentRuleRegistration,
    ComponentWeak,
};
pub use component_macros::{Component, component};
pub use game_object::{GameObject, GameObjectWeak};
pub use mesh::{InstancedMesh, Mesh, ProceduralMesh};
pub use scene::{Prefab, Scene};
pub use skinned_mesh::{Bone, SkinnedMesh};
