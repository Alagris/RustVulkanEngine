use crate::data::data_asset_database::DataAssetDatabase;
use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_yaml::{Mapping, Value};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub use anyhow as anyhow_crate;

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

pub type DataAssetFactory = fn(&Value, &DataAssetDeserializeContext) -> Result<Box<dyn DataAsset>>;

pub struct DataAssetRegistration {
    pub type_name: &'static str,
    pub factory: DataAssetFactory,
}

pub use inventory;
pub use serde_yaml;
inventory::collect!(DataAssetRegistration);

pub struct DataAssetCell {
    pub(crate) name: Option<String>,
    pub(crate) registration: &'static DataAssetRegistration,
    value: OnceLock<Box<dyn DataAsset>>,
}

impl DataAssetCell {
    pub(crate) fn new(name: Option<String>, registration: &'static DataAssetRegistration) -> Self {
        Self {
            name,
            registration,
            value: OnceLock::new(),
        }
    }

    pub(crate) fn set(&self, asset: Box<dyn DataAsset>) -> Result<()> {
        self.value
            .set(asset)
            .map_err(|_| anyhow!("data asset cell was initialized more than once"))
    }

    pub(crate) fn asset(&self) -> Option<&dyn DataAsset> {
        self.value.get().map(Box::as_ref)
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct DataAssetKey {
    pub(crate) type_name: String,
    pub(crate) name: String,
}

pub struct DataAssetDeserializeContext {
    registrations: HashMap<&'static str, &'static DataAssetRegistration>,
    assets: HashMap<DataAssetKey, Weak<DataAssetCell>>,
    nested: Mutex<Vec<Weak<DataAssetCell>>>,
}

impl DataAssetDeserializeContext {
    pub(crate) fn new(
        registrations: HashMap<&'static str, &'static DataAssetRegistration>,
        assets: HashMap<DataAssetKey, Weak<DataAssetCell>>,
    ) -> Self {
        Self {
            registrations,
            assets,
            nested: Mutex::new(Vec::new()),
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
        let (cell, name, type_name) = self.resolve_asset_link(name, T::TYPE_NAME)?;
        Ok(DataRef::linked(cell, name, type_name))
    }

    pub(crate) fn registration(&self, type_name: &str) -> Result<&'static DataAssetRegistration> {
        self.registrations
            .get(type_name)
            .copied()
            .ok_or_else(|| anyhow!("unregistered data asset type '{type_name}'"))
    }

    pub(crate) fn resolve_asset_link(
        &self,
        name: &str,
        type_name: &str,
    ) -> Result<(Arc<DataAssetCell>, String, String)> {
        let key = DataAssetKey {
            type_name: type_name.to_owned(),
            name: name.to_owned(),
        };
        let cell = self
            .assets
            .get(&key)
            .and_then(Weak::upgrade)
            .ok_or_else(|| {
                anyhow!(
                    "data asset '{}' of type '{}' was not found",
                    key.name,
                    key.type_name
                )
            })?;
        Ok((cell, key.name, key.type_name))
    }

    pub(crate) fn has_asset(&self, name: &str, type_name: &str) -> bool {
        self.assets
            .get(&DataAssetKey {
                type_name: type_name.to_owned(),
                name: name.to_owned(),
            })
            .and_then(Weak::upgrade)
            .is_some()
    }

    fn create_nested_typed<T: DataAssetType>(&self, value: Value) -> Result<Arc<DataAssetCell>> {
        let registration = self.registration(T::TYPE_NAME)?;
        let asset = (registration.factory)(&value, self).with_context(|| {
            format!("Failed to deserialize nested '{}' data asset", T::TYPE_NAME)
        })?;
        if !asset.as_any().is::<T>() {
            bail!(
                "data asset factory for '{}' returned a different type",
                T::TYPE_NAME
            );
        }
        let cell = Arc::new(DataAssetCell::new(None, registration));
        cell.set(asset)?;
        self.register_nested(&cell)?;
        Ok(cell)
    }

    pub(crate) fn create_nested_asset(
        &self,
        type_name: &str,
        value: &Value,
    ) -> Result<Arc<DataAssetCell>> {
        let registration = self.registration(type_name)?;
        let asset = (registration.factory)(value, self)
            .with_context(|| format!("Failed to deserialize nested '{type_name}' data asset"))?;
        let cell = Arc::new(DataAssetCell::new(None, registration));
        cell.set(asset)?;
        self.register_nested(&cell)?;
        Ok(cell)
    }

    fn register_nested(&self, cell: &Arc<DataAssetCell>) -> Result<()> {
        self.nested
            .lock()
            .map_err(|_| anyhow!("nested data asset list lock was poisoned"))?
            .push(Arc::downgrade(cell));
        Ok(())
    }

    pub(crate) fn nested_assets(&self) -> Result<Vec<Arc<DataAssetCell>>> {
        self.nested
            .lock()
            .map_err(|_| anyhow!("nested data asset list lock was poisoned"))
            .map(|nested| nested.iter().filter_map(Weak::upgrade).collect())
    }
}

thread_local! {
    static ACTIVE_CONTEXTS: RefCell<Vec<Arc<DataAssetDeserializeContext>>> = const { RefCell::new(Vec::new()) };
}

struct ContextGuard;

impl Drop for ContextGuard {
    fn drop(&mut self) {
        ACTIVE_CONTEXTS.with(|contexts| {
            contexts.borrow_mut().pop();
        });
    }
}

pub(crate) fn with_context<R>(
    context: Arc<DataAssetDeserializeContext>,
    action: impl FnOnce() -> Result<R>,
) -> Result<R> {
    ACTIVE_CONTEXTS.with(|contexts| contexts.borrow_mut().push(context));
    let guard = ContextGuard;
    let result = action();
    drop(guard);
    result
}

fn current_context() -> Result<Arc<DataAssetDeserializeContext>> {
    ACTIVE_CONTEXTS.with(|contexts| {
        contexts
            .borrow()
            .last()
            .cloned()
            .ok_or_else(|| anyhow!("DataRef can only be deserialized while loading a data asset"))
    })
}

pub trait DataAssetTarget: 'static {
    fn type_name() -> Option<&'static str>;

    fn cast<'a>(asset: &'a dyn DataAsset, cell: &'a DataAssetCell) -> Option<&'a Self>;

    fn resolve_link(
        context: &DataAssetDeserializeContext,
        name: &str,
        type_name: Option<&str>,
    ) -> Result<(Arc<DataAssetCell>, String, String)>;

    fn deserialize_nested(
        context: &DataAssetDeserializeContext,
        type_name: Option<&str>,
        value: Value,
    ) -> Result<Arc<DataAssetCell>>;
}

impl<T> DataAssetTarget for T
where
    T: DataAssetType,
{
    fn type_name() -> Option<&'static str> {
        Some(T::TYPE_NAME)
    }

    fn cast<'a>(asset: &'a dyn DataAsset, _cell: &'a DataAssetCell) -> Option<&'a Self> {
        asset.as_any().downcast_ref::<T>()
    }

    fn resolve_link(
        context: &DataAssetDeserializeContext,
        name: &str,
        type_name: Option<&str>,
    ) -> Result<(Arc<DataAssetCell>, String, String)> {
        if type_name.is_some_and(|type_name| type_name != T::TYPE_NAME) {
            bail!(
                "linked data asset '{name}' has type '{}', expected '{}'",
                type_name.unwrap_or_default(),
                T::TYPE_NAME
            );
        }
        context.resolve_asset_link(name, T::TYPE_NAME)
    }

    fn deserialize_nested(
        context: &DataAssetDeserializeContext,
        type_name: Option<&str>,
        value: Value,
    ) -> Result<Arc<DataAssetCell>> {
        if type_name.is_some_and(|type_name| type_name != T::TYPE_NAME) {
            bail!(
                "nested data asset has type '{}', expected '{}'",
                type_name.unwrap_or_default(),
                T::TYPE_NAME
            );
        }
        context.create_nested_typed::<T>(value)
    }
}

enum DataRefStorage {
    Owned(Arc<DataAssetCell>),
    Linked {
        cell: Weak<DataAssetCell>,
        name: String,
        type_name: String,
    },
}

pub struct DataRef<T: ?Sized + DataAssetTarget> {
    storage: DataRefStorage,
    marker: PhantomData<fn() -> T>,
}

impl<T: ?Sized + DataAssetTarget> DataRef<T> {
    fn owned(cell: Arc<DataAssetCell>) -> Self {
        Self {
            storage: DataRefStorage::Owned(cell),
            marker: PhantomData,
        }
    }

    pub(crate) fn linked(cell: Arc<DataAssetCell>, name: String, type_name: String) -> Self {
        Self {
            storage: DataRefStorage::Linked {
                cell: Arc::downgrade(&cell),
                name,
                type_name,
            },
            marker: PhantomData,
        }
    }

    fn upgrade(&self) -> Option<Arc<DataAssetCell>> {
        match &self.storage {
            DataRefStorage::Owned(cell) => Some(Arc::clone(cell)),
            DataRefStorage::Linked { cell, .. } => cell.upgrade(),
        }
    }

    pub fn is_alive(&self) -> bool {
        self.upgrade().is_some()
    }

    pub fn is_nested(&self) -> bool {
        matches!(&self.storage, DataRefStorage::Owned(_))
    }

    pub fn with<R>(&self, action: impl FnOnce(&T) -> R) -> Option<R> {
        let cell = self.upgrade()?;
        let asset = cell.asset()?;
        T::cast(asset, &cell).map(action)
    }
}

impl<T: ?Sized + DataAssetTarget> std::fmt::Debug for DataRef<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (name, type_name) = match &self.storage {
            DataRefStorage::Owned(cell) => (
                cell.name.as_deref().unwrap_or("<nested>"),
                cell.registration.type_name,
            ),
            DataRefStorage::Linked {
                name, type_name, ..
            } => (name.as_str(), type_name.as_str()),
        };
        formatter
            .debug_struct("DataRef")
            .field("type", &type_name)
            .field("name", &name)
            .field("nested", &self.is_nested())
            .finish()
    }
}

impl<T: ?Sized + DataAssetTarget> Serialize for DataRef<T> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut mapping = Mapping::new();
        match &self.storage {
            DataRefStorage::Owned(cell) => {
                let asset = cell.asset().ok_or_else(|| {
                    serde::ser::Error::custom("nested data asset has not been initialized")
                })?;
                mapping.insert(
                    Value::String("type".to_owned()),
                    Value::String(cell.registration.type_name.to_owned()),
                );
                mapping.insert(
                    Value::String("data".to_owned()),
                    asset.serialize_data().map_err(serde::ser::Error::custom)?,
                );
            }
            DataRefStorage::Linked {
                name, type_name, ..
            } => {
                mapping.insert(Value::String("ref".to_owned()), Value::String(name.clone()));
                mapping.insert(
                    Value::String("type".to_owned()),
                    Value::String(type_name.clone()),
                );
            }
        }
        Value::Mapping(mapping).serialize(serializer)
    }
}

impl<'de, T: ?Sized + DataAssetTarget> Deserialize<'de> for DataRef<T> {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;
        let context = current_context().map_err(serde::de::Error::custom)?;
        let mapping = value.as_mapping_mut();
        let reference_key = Value::String("ref".to_owned());
        if let Some(mapping) = mapping {
            if let Some(reference) = mapping.remove(&reference_key) {
                let name = reference
                    .as_str()
                    .filter(|name| !name.trim().is_empty())
                    .ok_or_else(|| {
                        serde::de::Error::custom("DataRef ref must be a non-empty name")
                    })?
                    .to_owned();
                let type_name = mapping
                    .remove(&Value::String("type".to_owned()))
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
                let (cell, name, type_name) =
                    T::resolve_link(&context, &name, type_name.as_deref())
                        .map_err(serde::de::Error::custom)?;
                return Ok(Self::linked(cell, name, type_name));
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
                    .map_err(serde::de::Error::custom)?;
                return Ok(Self::owned(cell));
            }
        }
        let cell =
            T::deserialize_nested(&context, None, value).map_err(serde::de::Error::custom)?;
        Ok(Self::owned(cell))
    }
}
