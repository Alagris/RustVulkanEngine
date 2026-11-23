use super::data_asset::DataAsset;
use super::data_asset_deserialize_context::DataAssetDeserializeContext;
use anyhow::Result;
use serde_yaml::Value;

pub type DataAssetFactory = fn(&Value, &DataAssetDeserializeContext) -> Result<Box<dyn DataAsset>>;

pub struct DataAssetRegistration {
    pub type_name: &'static str,
    pub factory: DataAssetFactory,
}

inventory::collect!(DataAssetRegistration);
