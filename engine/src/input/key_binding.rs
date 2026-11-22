use crate::data::data_asset::{DataAssetDeserializeContext, DataRef};
use crate::input::input_type::InputType;
use crate::input::input_type::source_type;
use crate::input::modifier::Modifier;
use anyhow::{Result, bail};
use component_macros::DataAsset;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;

#[derive(DataAsset, Debug, Deserialize, Serialize)]
#[data_asset(deserialize_with = deserialize_key_binding)]
pub struct KeyBinding {
    pub key: u32,
    pub input_type: InputType,
    #[serde(default)]
    pub modifiers: Vec<DataRef<dyn Modifier>>,
}

fn deserialize_key_binding(
    value: &Value,
    context: &DataAssetDeserializeContext,
) -> Result<KeyBinding> {
    let binding: KeyBinding = context.deserialize(value.clone())?;
    let mut input_type = source_type(binding.key);
    for modifier in &binding.modifiers {
        let (modifier_input, modifier_output) = modifier
            .with(|modifier| (modifier.input_type(), modifier.output_type()))
            .ok_or_else(|| anyhow::anyhow!("modifier for '{}' is no longer alive", binding.key))?;
        if modifier_input != input_type {
            bail!(
                "modifier for input '{}' expects {:?}, but the previous stage produces {:?}",
                binding.key,
                modifier_input,
                input_type
            );
        }
        input_type = modifier_output;
    }
    if binding.input_type != input_type {
        bail!(
            "input binding '{}' declares type {:?}, but its modifiers produce {:?}",
            binding.key,
            binding.input_type,
            input_type
        );
    }
    Ok(binding)
}
