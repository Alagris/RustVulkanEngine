use crate::ecs::collider::{Capsule, ColliderSettings, PhysicsCollider, impl_static_collider};
use crate::ecs::component::ComponentInstance;
use serde::{Deserialize, Serialize};

#[crate::ecs::component]
#[derive(crate::ecs::Component)]
/// A static Y-axis capsule synchronized with its game object.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct CapsuleCollider {
    pub radius: f32,
    pub half_height: f32,
    #[serde(flatten)]
    pub settings: ColliderSettings,
    #[serde(skip)]
    collider: PhysicsCollider<Capsule>,
}

impl Default for CapsuleCollider {
    fn default() -> Self {
        Self {
            instance: ComponentInstance::default(),
            radius: 0.5,
            half_height: 0.5,
            settings: ColliderSettings::default(),
            collider: PhysicsCollider::default(),
        }
    }
}

impl_static_collider!(CapsuleCollider, Capsule, |this: &CapsuleCollider| {
    physx_safe::ColliderGeometry::capsule(this.radius, this.half_height, physx_safe::CapsuleAxis::Y)
});
