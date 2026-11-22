use crate::ecs::collider::{BoxShape, ColliderSettings, PhysicsCollider, impl_static_collider};
use crate::ecs::component::ComponentInstance;
use glam::Vec3;
use serde::{Deserialize, Serialize};

#[crate::ecs::component]
#[derive(crate::ecs::Component)]
/// A static box synchronized with its game object.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct BoxCollider {
    pub half_extents: Vec3,
    #[serde(flatten)]
    pub settings: ColliderSettings,
    #[serde(skip)]
    collider: PhysicsCollider<BoxShape>,
}

impl Default for BoxCollider {
    fn default() -> Self {
        Self {
            instance: ComponentInstance::default(),
            half_extents: Vec3::splat(0.5),
            settings: ColliderSettings::default(),
            collider: PhysicsCollider::default(),
        }
    }
}

impl_static_collider!(BoxCollider, BoxShape, |this: &BoxCollider| {
    physx_safe::ColliderGeometry::cuboid(this.half_extents)
});
