use crate::data::data_asset::DataRef;
use crate::data::input_type::InputType;
use crate::data::modifier::Modifier;
use component_macros::DataAsset;
use serde::{Deserialize, Serialize};

#[derive(DataAsset, Debug, Deserialize, Serialize)]
pub struct KeyBinding {
    pub key: String,
    pub input_type: InputType,
    #[serde(default)]
    pub modifiers: Vec<DataRef<dyn Modifier>>,
}
