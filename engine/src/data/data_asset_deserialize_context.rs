use super::data_asset_cell::DataAssetCell;
use super::data_asset_key::DataAssetKey;
use super::data_asset_link::DataAssetLink;
use super::data_asset_registration::DataAssetRegistration;
use super::data_asset_scope::current_context;
use super::{DataAssetType, DataRef};
use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde_yaml::Value;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct DataAssetDeserializeContext {
    registrations: HashMap<&'static str, &'static DataAssetRegistration>,
    assets: HashMap<DataAssetKey, DataAssetLink>,
    nested: Mutex<Vec<DataAssetLink>>,
    aliases: HashMap<DataAssetKey, Vec<DataAssetKey>>,
    path_keys: bool,
    allow_names: bool,
}

impl DataAssetDeserializeContext {
    pub(crate) fn new(
        registrations: HashMap<&'static str, &'static DataAssetRegistration>,
        assets: HashMap<DataAssetKey, DataAssetLink>,
        aliases: HashMap<DataAssetKey, Vec<DataAssetKey>>,
        path_keys: bool,
        allow_names: bool,
    ) -> Self {
        Self {
            registrations,
            assets,
            nested: Mutex::new(Vec::new()),
            aliases,
            path_keys,
            allow_names,
        }
    }

    pub fn deserialize<T: DeserializeOwned>(&self, value: Value) -> Result<T> {
        let context = current_context()?;
        if !std::ptr::eq(context.as_ref(), self) {
            bail!("data asset deserialization context is not active");
        }
        serde_yaml::from_value(value).context("Failed to deserialize data asset data")
    }

    pub fn link<T: DataAssetType>(&self, name: &str) -> Result<DataRef<T>> {
        Ok(DataRef::linked(
            self.resolve_asset_link(name, T::TYPE_NAME)?,
        ))
    }

    pub(crate) fn registration(&self, type_name: &str) -> Result<&'static DataAssetRegistration> {
        self.registrations
            .get(type_name)
            .copied()
            .ok_or_else(|| anyhow!("unregistered data asset type '{type_name}'"))
    }

    pub(crate) fn registered_types(&self) -> Vec<&'static str> {
        let mut types = self.registrations.keys().copied().collect::<Vec<_>>();
        types.sort_unstable();
        types
    }

    pub(crate) fn resolve_asset_link(&self, name: &str, type_name: &str) -> Result<DataAssetLink> {
        let name = self.reference_name(name)?;
        let mut key = DataAssetKey {
            type_name: type_name.to_owned(),
            name: name.clone(),
        };
        if !self.assets.contains_key(&key)
            && let Some(candidates) = self.aliases.get(&key)
        {
            if candidates.len() != 1 {
                bail!(
                    "ambiguous data asset '{}' of type '{}'; use an asset-relative path",
                    name,
                    type_name
                );
            }
            key = candidates[0].clone();
        }
        self.assets
            .get(&key)
            .filter(|link| link.is_alive())
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "data asset '{}' of type '{}' was not found",
                    key.name,
                    key.type_name
                )
            })
    }

    pub(crate) fn reference_name(&self, name: &str) -> Result<String> {
        let name = if self.path_keys
            && crate::assets::AssetKind::from_path(std::path::Path::new(name))
                == Some(crate::assets::AssetKind::Data)
        {
            crate::assets::asset_ref::normalized_path(name)?
        } else {
            if !self.allow_names {
                bail!(
                    "data asset name references are development-only; '{name}' requires an asset-relative .data path"
                );
            }
            name.to_owned()
        };
        Ok(name)
    }

    pub(crate) fn has_asset(&self, name: &str, type_name: &str) -> bool {
        let key = DataAssetKey {
            type_name: type_name.to_owned(),
            name: name.to_owned(),
        };
        self.aliases.contains_key(&key)
            || self.assets.get(&key).is_some_and(DataAssetLink::is_alive)
    }

    pub(crate) fn create_nested_typed<T: DataAssetType>(
        &self,
        value: Value,
    ) -> Result<Box<DataAssetCell>> {
        let cell = self.create_nested_asset(T::TYPE_NAME, &value)?;
        if !cell.asset().is_some_and(|asset| asset.as_any().is::<T>()) {
            bail!(
                "data asset factory for '{}' returned a different type",
                T::TYPE_NAME
            );
        }
        Ok(cell)
    }

    pub(crate) fn create_nested_asset(
        &self,
        type_name: &str,
        value: &Value,
    ) -> Result<Box<DataAssetCell>> {
        let registration = self.registration(type_name)?;
        let asset = (registration.factory)(value, self)
            .with_context(|| format!("Failed to deserialize nested '{type_name}' data asset"))?;
        let cell = Box::new(DataAssetCell::new(None, registration.type_name));
        cell.set(asset)?;
        self.nested
            .lock()
            .map_err(|_| anyhow!("nested data asset list lock was poisoned"))?
            .push(cell.link());
        Ok(cell)
    }

    pub(crate) fn nested_assets(&self) -> Result<Vec<DataAssetLink>> {
        self.nested
            .lock()
            .map_err(|_| anyhow!("nested data asset list lock was poisoned"))
            .map(|nested| {
                nested
                    .iter()
                    .filter(|link| link.is_alive())
                    .cloned()
                    .collect()
            })
    }
}
