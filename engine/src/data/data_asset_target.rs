use super::data_asset_cell::DataAssetCell;
use super::data_asset_deserialize_context::DataAssetDeserializeContext;
use super::data_asset_link::DataAssetLink;
use super::{DataAsset, DataAssetType};
use anyhow::{Result, bail};
use serde_yaml::Value;

pub trait DataAssetTarget: 'static {
    fn type_name() -> Option<&'static str>;

    fn cast<'a>(asset: &'a dyn DataAsset, type_name: &str) -> Option<&'a Self>;

    fn resolve_link(
        context: &DataAssetDeserializeContext,
        name: &str,
        type_name: Option<&str>,
    ) -> Result<DataAssetLink>;

    fn deserialize_nested(
        context: &DataAssetDeserializeContext,
        type_name: Option<&str>,
        value: Value,
    ) -> Result<Box<DataAssetCell>>;
}

impl<T: DataAssetType> DataAssetTarget for T {
    fn type_name() -> Option<&'static str> {
        Some(T::TYPE_NAME)
    }

    fn cast<'a>(asset: &'a dyn DataAsset, _type_name: &str) -> Option<&'a Self> {
        asset.as_any().downcast_ref::<T>()
    }

    fn resolve_link(
        context: &DataAssetDeserializeContext,
        name: &str,
        type_name: Option<&str>,
    ) -> Result<DataAssetLink> {
        if let Some(type_name) = type_name
            && type_name != T::TYPE_NAME
        {
            bail!(
                "linked data asset '{name}' has type '{}', expected '{}'",
                type_name,
                T::TYPE_NAME
            );
        }
        context.resolve_asset_link(name, T::TYPE_NAME)
    }

    fn deserialize_nested(
        context: &DataAssetDeserializeContext,
        type_name: Option<&str>,
        value: Value,
    ) -> Result<Box<DataAssetCell>> {
        if let Some(type_name) = type_name
            && type_name != T::TYPE_NAME
        {
            bail!(
                "nested data asset has type '{}', expected '{}'",
                type_name,
                T::TYPE_NAME
            );
        }
        context.create_nested_typed::<T>(value)
    }
}
