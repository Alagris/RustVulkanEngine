use super::data_asset_cell::{DataAssetCell, DataAssetValue};
use super::data_asset_link::DataAssetLink;
use super::data_asset_scope::current_context;
use super::data_asset_target::DataAssetTarget;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_yaml::{Mapping, Value};
use std::marker::PhantomData;

enum DataRefStorage {
    Owned(Box<DataAssetCell>),
    Linked(DataAssetLink),
}

pub struct DataRef<T: ?Sized + DataAssetTarget> {
    storage: DataRefStorage,
    marker: PhantomData<fn() -> T>,
}

impl<T: ?Sized + DataAssetTarget> DataRef<T> {
    fn owned(cell: Box<DataAssetCell>) -> Self {
        Self {
            storage: DataRefStorage::Owned(cell),
            marker: PhantomData,
        }
    }

    pub(crate) fn linked(cell: DataAssetLink) -> Self {
        Self {
            storage: DataRefStorage::Linked(cell),
            marker: PhantomData,
        }
    }

    fn with_cell<R>(&self, action: impl FnOnce(&DataAssetValue) -> R) -> Option<R> {
        match &self.storage {
            DataRefStorage::Owned(cell) => Some(action(cell.value())),
            DataRefStorage::Linked(cell) => cell.with_value(action),
        }
    }

    pub fn is_alive(&self) -> bool {
        self.with_cell(|_| ()).is_some()
    }

    pub fn is_nested(&self) -> bool {
        matches!(&self.storage, DataRefStorage::Owned(_))
    }

    pub fn with<R>(&self, action: impl FnOnce(&T) -> R) -> Option<R> {
        self.with_cell(|cell| {
            let asset = T::cast(cell.asset()?, cell.type_name)?;
            Some(action(asset))
        })
        .flatten()
    }
}

impl<T: ?Sized + DataAssetTarget> std::fmt::Debug for DataRef<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with_cell(|cell| {
            formatter
                .debug_struct("DataRef")
                .field("type", &cell.type_name)
                .field("name", &cell.name.as_deref().unwrap_or("<nested>"))
                .field("nested", &self.is_nested())
                .finish()
        })
        .unwrap_or_else(|| formatter.write_str("DataRef(<expired>)"))
    }
}

impl<T: ?Sized + DataAssetTarget> Serialize for DataRef<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let value = self
            .with_cell(|cell| -> Result<Value> {
                let mut mapping = Mapping::new();
                mapping.insert(
                    Value::String("type".to_owned()),
                    Value::String(cell.type_name.to_owned()),
                );
                if self.is_nested() {
                    let asset = cell
                        .asset()
                        .ok_or_else(|| anyhow!("nested data asset has not been initialized"))?;
                    mapping.insert(Value::String("data".to_owned()), asset.serialize_data()?);
                } else {
                    let name = cell
                        .name
                        .as_ref()
                        .ok_or_else(|| anyhow!("linked data asset has no name"))?;
                    mapping.insert(Value::String("ref".to_owned()), Value::String(name.clone()));
                }
                Ok(Value::Mapping(mapping))
            })
            .ok_or_else(|| {
                serde::ser::Error::custom("cannot serialize expired data asset reference")
            })?
            .map_err(serde::ser::Error::custom)?;
        value.serialize(serializer)
    }
}

impl<'de, T: ?Sized + DataAssetTarget> Deserialize<'de> for DataRef<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let mut value = Value::deserialize(deserializer)?;
        let context = current_context().map_err(serde::de::Error::custom)?;
        if let Some(mapping) = value.as_mapping_mut() {
            if let Some(reference) = mapping.remove(Value::String("ref".to_owned())) {
                let name = reference
                    .as_str()
                    .filter(|name| !name.trim().is_empty())
                    .ok_or_else(|| {
                        serde::de::Error::custom("DataRef ref must be a non-empty name")
                    })?;
                let type_name = mapping
                    .remove(Value::String("type".to_owned()))
                    .map(|value| {
                        value.as_str().map(str::to_owned).ok_or_else(|| {
                            serde::de::Error::custom("DataRef type must be a string")
                        })
                    })
                    .transpose()?;
                if !mapping.is_empty() {
                    return Err(serde::de::Error::custom(
                        "linked DataRef may only contain ref and optional type",
                    ));
                }
                let cell = T::resolve_link(&context, name, type_name.as_deref())
                    .map_err(|error| serde::de::Error::custom(format!("{error:#}")))?;
                return Ok(Self::linked(cell));
            }
            let type_key = Value::String("type".to_owned());
            let data_key = Value::String("data".to_owned());
            if mapping.contains_key(&type_key) && mapping.contains_key(&data_key) {
                let type_name = mapping
                    .remove(&type_key)
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .ok_or_else(|| {
                        serde::de::Error::custom("nested DataRef type must be a string")
                    })?;
                let nested_data = mapping
                    .remove(&data_key)
                    .ok_or_else(|| serde::de::Error::custom("nested DataRef must contain data"))?;
                if !mapping.is_empty() {
                    return Err(serde::de::Error::custom(
                        "nested DataRef may only contain type and data",
                    ));
                }
                let cell = T::deserialize_nested(&context, Some(&type_name), nested_data)
                    .map_err(|error| serde::de::Error::custom(format!("{error:#}")))?;
                return Ok(Self::owned(cell));
            }
        }
        let cell = T::deserialize_nested(&context, None, value)
            .map_err(|error| serde::de::Error::custom(format!("{error:#}")))?;
        Ok(Self::owned(cell))
    }
}
