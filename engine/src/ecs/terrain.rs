use crate::data::{DataRef, terrain_layer::TerrainLayer};
use crate::ecs::Component;
use serde::{Deserialize, Serialize};
use std::any::Any;

/// Asset wiring only; terrain generation and rendering are not implemented.
#[crate::ecs::component]
#[derive(crate::ecs::Component, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Terrain {
    pub layer: DataRef<TerrainLayer>,
}

impl Component for Terrain {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
