use crate::assets::PrefabRef;
use crate::ecs::Component;
use serde::{Deserialize, Serialize};
use std::any::Any;

/// Asset wiring only; scattering will be implemented separately.
#[crate::ecs::component]
#[derive(crate::ecs::Component, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Foliage {
    pub prefabs: Vec<PrefabRef>,
}

impl Component for Foliage {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
