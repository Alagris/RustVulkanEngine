use crate::data::data_asset::DataRef;
use crate::data::keymap::KeyMap;
use component_macros::DataAsset;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(DataAsset, Debug, Deserialize, Serialize)]
pub struct Input {
    #[serde(default)]
    pub keymaps: BTreeMap<String, DataRef<KeyMap>>,
}
