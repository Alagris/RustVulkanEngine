use crate::ecs::collider::{ColliderSettings, PhysicsCollider, Plane, impl_static_collider};
use crate::ecs::component::ComponentInstance;
use serde::{Deserialize, Serialize};

#[crate::ecs::component]
#[derive(crate::ecs::Component)]
/// A static plane with a local +X normal.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct PlaneCollider {
    #[serde(flatten)]
    pub settings: ColliderSettings,
    #[serde(skip)]
    collider: PhysicsCollider<Plane>,
}

impl Default for PlaneCollider {
    fn default() -> Self {
        Self {
            instance: ComponentInstance::default(),
            settings: ColliderSettings::default(),
            collider: PhysicsCollider::default(),
        }
    }
}

impl_static_collider!(PlaneCollider, Plane, |_: &PlaneCollider| Ok::<
    _,
    physx_safe::PhysicsError,
>(
    physx_safe::ColliderGeometry::plane()
));
