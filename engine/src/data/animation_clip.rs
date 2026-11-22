use component_macros::DataAsset;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Trigger {
    pub time: f32,
    pub trigger: String,
}

#[derive(DataAsset, Debug, Deserialize, Serialize)]
pub struct AnimationClip {
    #[serde(default)]
    pub triggers: Vec<Trigger>,
}
