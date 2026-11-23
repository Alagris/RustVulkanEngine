use super::asset_ref::{
    AssetCell, AssetContext, AssetSource, AssetTarget, LoadedAsset, normalized_path,
};
use super::{
    AssetKind, AssetRef, AssetReference, DataAssetFile, Prefab, PrefabResolver, UiDocument,
    load_prefab_path,
};
use crate::data::DataAssetDatabase;
use crate::ecs::{ComponentRegistry, Scene};
use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde_yaml::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// Authoring-time resolution only. Runtime templates never retain this resolver.
pub trait AssetNameResolver {
    fn resolve_asset_name(&self, name: &str, kind: AssetKind) -> Result<PathBuf>;
}

/// Owns loaded assets strongly; all injected references are weak.
pub struct AssetStore {
    root: PathBuf,
    cells: HashMap<String, Arc<AssetCell>>,
    context: Arc<AssetContext>,
    data: Arc<DataAssetDatabase>,
}

impl AssetStore {
    /// Path-only runtime loading. No names are accepted, even in a debug build.
    pub fn load(root: impl AsRef<Path>) -> Result<Self> {
        Self::build(root.as_ref(), None)
    }

    pub fn load_development(
        root: impl AsRef<Path>,
        resolver: &dyn AssetNameResolver,
    ) -> Result<Self> {
        Self::build(root.as_ref(), Some(resolver))
    }

    fn build(root: &Path, names: Option<&dyn AssetNameResolver>) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("Failed to resolve asset directory {}", root.display()))?;
        if !root.is_dir() {
            bail!("asset root is not a directory: {}", root.display());
        }
        let mut files = Vec::new();
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    pending.push(entry.path());
                } else if kind.is_file() && AssetKind::from_path(&entry.path()).is_some() {
                    files.push(entry.path());
                }
            }
        }
        files.sort();
        let cells = files
            .iter()
            .map(|path| {
                let relative = relative_path(&root, path)?;
                let kind = AssetKind::from_path(path).expect("filtered asset extension");
                Ok((
                    relative,
                    Arc::new(AssetCell {
                        kind,
                        value: OnceLock::new(),
                    }),
                ))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        let context = Arc::new(AssetContext {
            root: root.clone(),
            paths: cells
                .iter()
                .map(|(path, cell)| (path.clone(), Arc::downgrade(cell)))
                .collect(),
            data: OnceLock::new(),
        });
        let resolver = Paths { root: &root, names };
        let resolve = |source: &AssetSource| -> Result<AssetReference> {
            let source = resolved_source(&root, source, names)?;
            context.resolve(&source)
        };
        let mut data_documents = Vec::new();
        for path in &files {
            if AssetKind::from_path(path) == Some(AssetKind::Data) {
                let mut value = read_yaml(path)?;
                normalize_value(&mut value, &resolve, None)
                    .with_context(|| format!("Invalid asset references in {}", path.display()))?;
                data_documents.push((relative_path(&root, path)?, value, path.clone()));
            }
        }
        // All weak cells exist before any factory runs, so reference cycles do not recurse.
        let data =
            Arc::new(context.with(|| {
                DataAssetDatabase::from_path_documents(data_documents, names.is_some())
            })?);
        assert!(
            context.data.set(Arc::clone(&data.context)).is_ok(),
            "data context must be set once"
        );
        for path in &files {
            let key = relative_path(&root, path)?;
            let cell = &cells[&key];
            let asset = match cell.kind {
                AssetKind::Prefab => {
                    let mut root_value = load_prefab_path(path, &resolver, &mut Vec::new())?;
                    normalize_value(&mut root_value, &resolve, Some(&data)).with_context(|| {
                        format!("Invalid asset references in {}", path.display())
                    })?;
                    LoadedAsset::Prefab(Prefab {
                        name: path
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .ok_or_else(|| anyhow!("invalid prefab filename"))?
                            .to_owned(),
                        root: root_value,
                        context: Arc::clone(&context),
                    })
                }
                AssetKind::Ui => {
                    LoadedAsset::Ui(context.with(|| UiDocument::load(path, &resolve))?)
                }
                AssetKind::Data => {
                    let value = read_yaml(path)?;
                    let type_name = value
                        .get("type")
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow!("data asset '{key}' requires a type"))?;
                    let name = value
                        .get("name")
                        .and_then(Value::as_str)
                        .or_else(|| path.file_stem().and_then(|stem| stem.to_str()))
                        .ok_or_else(|| anyhow!("invalid data asset name"))?
                        .to_owned();
                    LoadedAsset::Data(DataAssetFile {
                        cell: data.cell_by_path(&key, type_name)?,
                        name,
                    })
                }
            };
            if cell.value.set(asset).is_err() {
                bail!("asset '{key}' was initialized more than once");
            }
        }
        assert!(
            cells.values().all(|cell| cell.value.get().is_some()),
            "every asset must be initialized"
        );
        context.with(|| data.awake_assets())?;
        Ok(Self {
            root,
            cells,
            context,
            data,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn data_assets(&self) -> &DataAssetDatabase {
        &self.data
    }
    pub fn data_assets_handle(&self) -> Arc<DataAssetDatabase> {
        Arc::clone(&self.data)
    }

    pub fn get<T: AssetTarget>(&self, path: &str) -> Result<AssetRef<T>> {
        self.context
            .resolve(&AssetSource {
                kind: T::KIND,
                path: Some(path.to_owned()),
                name: None,
            })?
            .typed()
    }

    pub fn deserialize<T: DeserializeOwned>(&self, value: Value) -> Result<T> {
        self.context.with(|| {
            serde_yaml::from_value(value).context("Failed to deserialize resolved asset references")
        })
    }

    pub fn with<R>(&self, action: impl FnOnce() -> Result<R>) -> Result<R> {
        self.context.with(action)
    }

    pub fn load_scene(
        &self,
        path: impl AsRef<Path>,
        registry: &ComponentRegistry,
    ) -> Result<Scene> {
        let path = path.as_ref();
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        self.context.with(|| Scene::load(&path, registry))
    }

    pub fn resolve_value(&self, value: &mut Value, names: &dyn AssetNameResolver) -> Result<()> {
        normalize_value(
            value,
            &|source| {
                self.context
                    .resolve(&resolved_source(&self.root, source, Some(names))?)
            },
            Some(&self.data),
        )
    }

    /// Write only path-based, expanded runtime files; preserve asset-relative identities.
    pub fn package(&self, output: &Path) -> Result<()> {
        for (path, cell) in &self.cells {
            let destination = output.join(Path::new(path));
            fs::create_dir_all(
                destination
                    .parent()
                    .ok_or_else(|| anyhow!("asset destination has no parent"))?,
            )?;
            let source = match cell.value.get().expect("initialized asset store") {
                LoadedAsset::Prefab(prefab) => serde_yaml::to_string(
                    &std::collections::BTreeMap::from([("root", &prefab.root)]),
                )?,
                LoadedAsset::Ui(document) => document.to_xml(),
                LoadedAsset::Data(data) => serde_yaml::to_string(&data.serialize()?)?,
            };
            fs::write(&destination, source)
                .with_context(|| format!("Failed to package asset {}", destination.display()))?;
        }
        Ok(())
    }
}

struct Paths<'a> {
    root: &'a Path,
    names: Option<&'a dyn AssetNameResolver>,
}
impl PrefabResolver for Paths<'_> {
    fn resolve_prefab_root(&self, reference: &str, stack: &mut Vec<PathBuf>) -> Result<Value> {
        let source =
            if Path::new(reference).extension().is_some() || reference.contains(['/', '\\']) {
                AssetSource {
                    kind: AssetKind::Prefab,
                    path: Some(reference.to_owned()),
                    name: None,
                }
            } else {
                AssetSource {
                    kind: AssetKind::Prefab,
                    name: Some(reference.to_owned()),
                    path: None,
                }
            };
        let source = resolved_source(self.root, &source, self.names)?;
        load_prefab_path(
            &self
                .root
                .join(source.path.expect("resolved source has path")),
            self,
            stack,
        )
    }
}

fn relative_path(root: &Path, path: &Path) -> Result<String> {
    let path = path.canonicalize()?;
    let relative = path
        .strip_prefix(root)
        .with_context(|| format!("asset {} is outside {}", path.display(), root.display()))?;
    normalized_path(
        relative
            .to_str()
            .ok_or_else(|| anyhow!("asset paths must be valid UTF-8"))?,
    )
}

fn resolved_source(
    root: &Path,
    source: &AssetSource,
    names: Option<&dyn AssetNameResolver>,
) -> Result<AssetSource> {
    let path = match (&source.path, &source.name) {
        (Some(path), None) => normalized_path(path)?,
        (None, Some(name)) => {
            if name.trim().is_empty() {
                bail!("asset name must not be empty");
            }
            let resolver = names.ok_or_else(|| {
                anyhow!("asset name references are development-only; '{name}' requires a path")
            })?;
            relative_path(root, &resolver.resolve_asset_name(name, source.kind)?)?
        }
        _ => bail!("asset reference requires exactly one of path or name"),
    };
    Ok(AssetSource {
        kind: source.kind,
        path: Some(path),
        name: None,
    })
}

fn normalize_value(
    value: &mut Value,
    resolve: &impl Fn(&AssetSource) -> Result<AssetReference>,
    data: Option<&DataAssetDatabase>,
) -> Result<()> {
    match value {
        Value::Mapping(mapping) if mapping.contains_key("asset") => {
            if mapping.len() != 1 {
                bail!("asset reference wrapper may only contain 'asset'");
            }
            let source: AssetSource = serde_yaml::from_value(mapping["asset"].clone())
                .context("invalid asset reference")?;
            *value = serde_yaml::to_value(resolve(&source)?)?;
        }
        Value::Mapping(mapping)
            if data.is_some()
                && mapping.contains_key("ref")
                && mapping
                    .keys()
                    .all(|key| matches!(key.as_str(), Some("ref" | "type"))) =>
        {
            let reference = mapping["ref"]
                .as_str()
                .ok_or_else(|| anyhow!("data asset ref must be a string"))?;
            let path = data
                .expect("data context supplied")
                .resolve_reference_path(reference, mapping.get("type").and_then(Value::as_str))?;
            mapping.insert(Value::String("ref".into()), Value::String(path));
        }
        Value::Mapping(mapping) => {
            for value in mapping.values_mut() {
                normalize_value(value, resolve, data)?;
            }
        }
        Value::Sequence(values) => {
            for value in values {
                normalize_value(value, resolve, data)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn read_yaml(path: &Path) -> Result<Value> {
    serde_yaml::from_str(
        &fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?,
    )
    .with_context(|| format!("Invalid YAML in {}", path.display()))
}
