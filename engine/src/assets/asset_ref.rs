use super::{Prefab, UiDocument};
use crate::data::data_asset::{DataAssetLink, DataAssetType};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Prefab,
    Ui,
    Data,
}

impl AssetKind {
    pub(crate) fn from_path(path: &std::path::Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "prefab" => Some(Self::Prefab),
            "ui" | "xml" => Some(Self::Ui),
            "data" => Some(Self::Data),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssetSource {
    #[serde(rename = "type")]
    pub(crate) kind: AssetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
}

pub(crate) struct AssetCell {
    pub(crate) kind: AssetKind,
    pub(crate) value: OnceLock<LoadedAsset>,
}

pub(crate) enum LoadedAsset {
    Prefab(Prefab),
    Ui(UiDocument),
    Data(DataAssetFile),
}

pub struct DataAssetFile {
    pub(crate) cell: DataAssetLink,
    pub(crate) name: String,
}

impl DataAssetFile {
    pub fn with<T: DataAssetType, R>(&self, action: impl FnOnce(&T) -> R) -> Result<R> {
        self.cell
            .with_value(|cell| {
                let asset = cell
                    .asset()
                    .ok_or_else(|| anyhow!("data asset has not been initialized"))?;
                let asset = asset.as_any().downcast_ref::<T>().ok_or_else(|| {
                    anyhow!("data asset is not of expected type '{}'", T::TYPE_NAME)
                })?;
                Ok(action(asset))
            })
            .ok_or_else(|| anyhow!("data asset has expired"))?
    }

    pub(crate) fn serialize(&self) -> Result<serde_yaml::Value> {
        self.cell
            .with_value(|cell| {
                let asset = cell
                    .asset()
                    .ok_or_else(|| anyhow!("data asset has not been initialized"))?;
                Ok(serde_yaml::to_value(BTreeMap::from([
                    ("type", serde_yaml::Value::String(cell.type_name.to_owned())),
                    ("name", serde_yaml::Value::String(self.name.clone())),
                    ("data", asset.serialize_data()?),
                ]))?)
            })
            .ok_or_else(|| anyhow!("data asset has expired"))?
    }
}

pub trait AssetTarget: Sized {
    const KIND: AssetKind;
    #[doc(hidden)]
    fn access<R>(reference: &AssetReference, action: impl FnOnce(&Self) -> R) -> Result<R>;
}

macro_rules! target {
    ($ty:ty, $kind:ident) => {
        impl AssetTarget for $ty {
            const KIND: AssetKind = AssetKind::$kind;
            fn access<R>(reference: &AssetReference, action: impl FnOnce(&Self) -> R) -> Result<R> {
                let cell = reference
                    .cell
                    .upgrade()
                    .ok_or_else(|| anyhow!("asset '{}' has expired", reference.path))?;
                match cell
                    .value
                    .get()
                    .ok_or_else(|| anyhow!("asset '{}' has not been initialized", reference.path))?
                {
                    LoadedAsset::$kind(asset) => Ok(action(asset)),
                    _ => bail!("asset '{}' has the wrong type", reference.path),
                }
            }
        }
    };
}
target!(Prefab, Prefab);
target!(UiDocument, Ui);
target!(DataAssetFile, Data);

/// A resolved weak pointer. The path is retained only for diagnostics and serialization.
#[derive(Clone)]
pub struct AssetReference {
    pub(crate) cell: Weak<AssetCell>,
    pub(crate) path: String,
    pub(crate) kind: AssetKind,
}

impl std::fmt::Debug for AssetReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssetReference")
            .field("path", &self.path)
            .field("kind", &self.kind)
            .finish()
    }
}

impl AssetReference {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn kind(&self) -> AssetKind {
        self.kind
    }
    pub fn is_alive(&self) -> bool {
        self.cell.upgrade().is_some()
    }
    pub fn typed<T: AssetTarget>(&self) -> Result<AssetRef<T>> {
        if self.kind != T::KIND {
            bail!(
                "asset '{}' is {:?}, expected {:?}",
                self.path,
                self.kind,
                T::KIND
            );
        }
        Ok(AssetRef {
            reference: self.clone(),
            marker: PhantomData,
        })
    }
}

pub struct AssetRef<T: AssetTarget> {
    reference: AssetReference,
    marker: PhantomData<fn() -> T>,
}

impl<T: AssetTarget> Clone for AssetRef<T> {
    fn clone(&self) -> Self {
        Self {
            reference: self.reference.clone(),
            marker: PhantomData,
        }
    }
}
impl<T: AssetTarget> std::fmt::Debug for AssetRef<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reference.fmt(f)
    }
}
impl<T: AssetTarget> AssetRef<T> {
    pub fn path(&self) -> &str {
        self.reference.path()
    }
    pub fn is_alive(&self) -> bool {
        self.reference.is_alive()
    }
    pub fn with<R>(&self, action: impl FnOnce(&T) -> R) -> Result<R> {
        T::access(&self.reference, action)
    }
}

pub type PrefabRef = AssetRef<Prefab>;
pub type UiDocumentRef = AssetRef<UiDocument>;
pub type AssetReferences = BTreeMap<String, AssetReference>;

impl Serialize for AssetReference {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        BTreeMap::from([(
            "asset",
            AssetSource {
                kind: self.kind,
                path: Some(self.path.clone()),
                name: None,
            },
        )])
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for AssetReference {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wrapper {
            asset: AssetSource,
        }
        let source = Wrapper::deserialize(deserializer)?.asset;
        current_context()
            .and_then(|context| context.resolve(&source))
            .map_err(|error| serde::de::Error::custom(format!("{error:#}")))
    }
}
impl<T: AssetTarget> Serialize for AssetRef<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.reference.serialize(serializer)
    }
}
impl<'de, T: AssetTarget> Deserialize<'de> for AssetRef<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let reference = if let Some(path) = value.as_str() {
            current_context().and_then(|context| {
                context.resolve(&AssetSource {
                    kind: T::KIND,
                    path: Some(path.to_owned()),
                    name: None,
                })
            })
        } else {
            serde_yaml::from_value::<AssetReference>(value).map_err(anyhow::Error::from)
        }
        .map_err(|error| serde::de::Error::custom(format!("{error:#}")))?;
        reference.typed().map_err(serde::de::Error::custom)
    }
}

impl<T: AssetTarget> crate::ui::elem::AttributeValue for AssetRef<T> {
    fn attribute_value(&self) -> Option<std::borrow::Cow<'_, str>> {
        Some(std::borrow::Cow::Borrowed(self.path()))
    }
    fn parse_attribute(value: Option<&str>) -> Result<Self> {
        let path = value.ok_or_else(|| anyhow!("asset reference cannot be removed"))?;
        current_context()?
            .resolve(&AssetSource {
                kind: T::KIND,
                path: Some(path.to_owned()),
                name: None,
            })?
            .typed()
    }
}

pub(crate) struct AssetContext {
    pub(crate) root: PathBuf,
    pub(crate) paths: HashMap<String, Weak<AssetCell>>,
    pub(crate) data: OnceLock<Arc<crate::data::data_asset::DataAssetDeserializeContext>>,
}
impl std::fmt::Debug for AssetContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AssetContext")
            .field("asset_count", &self.paths.len())
            .finish()
    }
}
impl AssetContext {
    pub(crate) fn resolve(&self, source: &AssetSource) -> Result<AssetReference> {
        if source.name.is_some() {
            bail!(
                "asset name references are development-only; expected an asset-root-relative path"
            );
        }
        let path = source
            .path
            .as_deref()
            .ok_or_else(|| anyhow!("asset reference requires a path"))?;
        let path = normalized_path(path)?;
        let cell = self
            .paths
            .get(&path)
            .and_then(Weak::upgrade)
            .ok_or_else(|| anyhow!("asset '{path}' was not found or has expired"))?;
        if cell.kind != source.kind {
            bail!(
                "asset '{path}' is {:?}, expected {:?}",
                cell.kind,
                source.kind
            );
        }
        Ok(AssetReference {
            cell: Arc::downgrade(&cell),
            path,
            kind: source.kind,
        })
    }
    pub(crate) fn with<R>(self: &Arc<Self>, action: impl FnOnce() -> Result<R>) -> Result<R> {
        let depth = ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let depth = active.len();
            active.push(Arc::clone(self));
            depth
        });
        struct Guard(usize);
        impl Drop for Guard {
            fn drop(&mut self) {
                ACTIVE.with(|active| {
                    let mut active = active.borrow_mut();
                    assert_eq!(
                        active.len(),
                        self.0 + 1,
                        "asset context stack must remain balanced"
                    );
                    active.pop().expect("active asset context must exist");
                });
            }
        }
        let _guard = Guard(depth);
        match self.data.get() {
            Some(data) => crate::data::data_asset::with_context(Arc::clone(data), action),
            None => action(),
        }
    }
}
thread_local! {
    static ACTIVE: RefCell<Vec<Arc<AssetContext>>> = const { RefCell::new(Vec::new()) };
}
pub(crate) fn current_context() -> Result<Arc<AssetContext>> {
    active_context()
        .ok_or_else(|| anyhow!("asset references require an AssetStore loading context"))
}
pub(crate) fn active_context() -> Option<Arc<AssetContext>> {
    ACTIVE.with(|active| active.borrow().last().cloned())
}

pub(crate) fn normalized_path(source: &str) -> Result<String> {
    let path = PathBuf::from(source.replace('\\', "/"));
    if source.is_empty()
        || path.is_absolute()
        || source.contains(':')
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        bail!(
            "asset path '{source}' must be relative to the asset directory and cannot contain '..'"
        );
    }
    let value = path
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if value.is_empty() {
        bail!("asset path must name a file");
    }
    Ok(value)
}
