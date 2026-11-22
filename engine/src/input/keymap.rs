use crate::data::data_asset::DataRef;
use crate::input::key_binding::KeyBinding;
use component_macros::DataAsset;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(DataAsset, Debug, Deserialize, Serialize)]
pub struct KeyMap {
    #[serde(default)]
    pub bindings: BTreeMap<String, DataRef<KeyBinding>>,
}
