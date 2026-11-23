use crate::data::data_asset_database::DataAssetDatabase;
use anyhow::Result;
use serde_yaml::Value;
use std::any::Any;

pub use super::data_asset_cell::DataAssetCell;
pub use super::data_asset_deserialize_context::DataAssetDeserializeContext;
pub(crate) use super::data_asset_key::DataAssetKey;
pub use super::data_asset_link::DataAssetLink;
pub use super::data_asset_registration::{DataAssetFactory, DataAssetRegistration};
pub(crate) use super::data_asset_scope::with_context;
pub use super::data_asset_target::DataAssetTarget;
pub use super::data_ref::DataRef;
pub use anyhow as anyhow_crate;
pub use inventory;
pub use serde_yaml;

pub trait DataAsset: Any + Send + Sync {
    fn awake(&self, _database: &DataAssetDatabase) -> Result<()> {
        Ok(())
    }

    fn as_any(&self) -> &dyn Any;

    fn serialize_data(&self) -> Result<Value>;
}

pub trait DataAssetType: DataAsset {
    const TYPE_NAME: &'static str;
}
