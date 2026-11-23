use serde::{Deserialize, Serialize};

/// Placeholder for terrain-layer configuration.
#[derive(Debug, Default, Deserialize, Serialize, crate::data::DataAsset)]
#[serde(default, deny_unknown_fields)]
pub struct TerrainLayer {
    pub label: String,
}
