use crate::ecs::collider::{ColliderSettings, PhysicsCollider, Sphere, impl_static_collider};
use crate::ecs::component::ComponentInstance;
use serde::{Deserialize, Serialize};

#[crate::ecs::component]
#[derive(crate::ecs::Component)]
/// A static sphere synchronized with its game object.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct SphereCollider {
    pub radius: f32,
    #[serde(flatten)]
    pub settings: ColliderSettings,
    #[serde(skip)]
    collider: PhysicsCollider<Sphere>,
}

impl Default for SphereCollider {
    fn default() -> Self {
        Self {
            instance: ComponentInstance::default(),
            radius: 0.5,
            settings: ColliderSettings::default(),
            collider: PhysicsCollider::default(),
        }
    }
}

impl_static_collider!(SphereCollider, Sphere, |this: &SphereCollider| {
    physx_safe::ColliderGeometry::sphere(this.radius)
});
