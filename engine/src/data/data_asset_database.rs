use crate::data::data_asset::{
    DataAssetCell, DataAssetDeserializeContext, DataAssetKey, DataAssetLink, DataAssetRegistration,
    DataAssetType, DataRef, inventory, with_context,
};
use anyhow::{Context, Result, anyhow, bail};
use serde_yaml::{Mapping, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct DataAssetDatabase {
    assets: HashMap<DataAssetKey, Box<DataAssetCell>>,
    aliases: HashMap<DataAssetKey, Vec<DataAssetKey>>,
    awake: Vec<DataAssetLink>,
    pub(crate) context: Arc<DataAssetDeserializeContext>,
}

impl DataAssetDatabase {
    pub fn load_dir(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        if !root.is_dir() {
            bail!("data asset directory does not exist: {}", root.display());
        }
        let mut paths = Vec::new();
        let mut directories = vec![root.to_path_buf()];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(&directory)
                .with_context(|| format!("Failed to scan data assets in {}", directory.display()))?
            {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    directories.push(entry.path());
                } else if kind.is_file()
                    && entry
                        .path()
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("data"))
                {
                    paths.push(entry.path());
                }
            }
        }
        paths.sort();
        let mut documents = Vec::with_capacity(paths.len());
        for path in paths {
            let source = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read data asset {}", path.display()))?;
            let value: Value = serde_yaml::from_str(&source)
                .with_context(|| format!("Invalid data asset YAML in {}", path.display()))?;
            let fallback_name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|stem| !stem.trim().is_empty())
                .ok_or_else(|| anyhow!("invalid data asset filename: {}", path.display()))?;
            documents.push((fallback_name.to_owned(), value, path));
        }
        Self::from_documents(documents)
    }

    pub fn from_documents(documents: Vec<(String, Value, PathBuf)>) -> Result<Self> {
        let database = Self::from_documents_with_aliases(documents, HashMap::new(), false, true)?;
        database.awake_assets()?;
        Ok(database)
    }

    pub(crate) fn from_path_documents(
        mut documents: Vec<(String, Value, PathBuf)>,
        development: bool,
    ) -> Result<Self> {
        let mut aliases: HashMap<DataAssetKey, Vec<DataAssetKey>> = HashMap::new();
        for (path, value, _) in &documents {
            let type_name = value
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("data asset '{path}' requires a type"))?;
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    Path::new(path)
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or(path)
                });
            aliases
                .entry(DataAssetKey {
                    type_name: type_name.to_owned(),
                    name: name.to_owned(),
                })
                .or_default()
                .push(DataAssetKey {
                    type_name: type_name.to_owned(),
                    name: path.to_owned(),
                });
        }
        for (path, value, _) in &mut documents {
            let type_name = value["type"]
                .as_str()
                .expect("validated data type")
                .to_owned();
            let mapping = value
                .as_mapping_mut()
                .ok_or_else(|| anyhow!("data asset '{path}' must be a mapping"))?;
            mapping.insert(Value::String("name".into()), Value::String(path.clone()));
            if let Some(parent) = mapping.get_mut("parent") {
                let reference = parent
                    .as_str()
                    .or_else(|| parent.get("ref").and_then(Value::as_str))
                    .ok_or_else(|| anyhow!("invalid data asset parent in '{path}'"))?;
                let is_path = reference.to_ascii_lowercase().ends_with(".data")
                    || reference.contains('/')
                    || reference.contains('\\');
                let resolved = if is_path {
                    crate::assets::asset_ref::normalized_path(reference)?
                } else if development {
                    let candidates = aliases
                        .get(&DataAssetKey {
                            type_name,
                            name: reference.to_owned(),
                        })
                        .ok_or_else(|| anyhow!("parent data asset '{reference}' was not found"))?;
                    if candidates.len() != 1 {
                        bail!("ambiguous parent data asset '{reference}'; use a path");
                    }
                    candidates[0].name.clone()
                } else {
                    bail!(
                        "data asset name references are development-only; parent '{reference}' requires a path"
                    );
                };
                *parent = Value::String(resolved);
            }
        }
        Self::from_documents_with_aliases(
            documents,
            if development { aliases } else { HashMap::new() },
            true,
            development,
        )
    }

    pub(crate) fn cell_by_path(&self, path: &str, type_name: &str) -> Result<DataAssetLink> {
        self.assets
            .get(&DataAssetKey {
                name: path.to_owned(),
                type_name: type_name.to_owned(),
            })
            .map(|cell| cell.link())
            .ok_or_else(|| anyhow!("data asset '{path}' of type '{type_name}' was not found"))
    }

    fn from_documents_with_aliases(
        documents: Vec<(String, Value, PathBuf)>,
        aliases: HashMap<DataAssetKey, Vec<DataAssetKey>>,
        path_keys: bool,
        allow_names: bool,
    ) -> Result<Self> {
        let registrations = collect_registrations()?;
        let mut parsed = HashMap::new();
        for (fallback_name, document, path) in documents {
            let document = parse_document(document, &fallback_name, path)?;
            let key = DataAssetKey {
                type_name: document.type_name.clone(),
                name: document.name.clone(),
            };
            if parsed.contains_key(&key) {
                log::warn!(
                    "Ignoring duplicate data asset '{}' of type '{}' at {}",
                    key.name,
                    key.type_name,
                    document.path.display()
                );
                continue;
            }
            if !registrations.contains_key(key.type_name.as_str()) {
                bail!(
                    "unregistered data asset type '{}' in {}",
                    key.type_name,
                    document.path.display()
                );
            }
            parsed.insert(key, document);
        }

        let mut resolved = HashMap::new();
        let mut resolving = HashSet::new();
        for key in parsed.keys() {
            resolve_document(key, &parsed, &mut resolved, &mut resolving)?;
        }

        let assets = parsed
            .iter()
            .map(|(key, document)| {
                let registration = registrations[key.type_name.as_str()];
                (
                    key.clone(),
                    Box::new(DataAssetCell::new(
                        Some(document.name.clone()),
                        registration.type_name,
                    )),
                )
            })
            .collect::<HashMap<_, _>>();
        let weak_assets = assets
            .iter()
            .map(|(key, cell)| (key.clone(), cell.link()))
            .collect::<HashMap<_, _>>();
        let context = Arc::new(DataAssetDeserializeContext::new(
            registrations,
            weak_assets,
            aliases.clone(),
            path_keys,
            allow_names,
        ));

        for (key, cell) in &assets {
            let data = resolved
                .get(key)
                .ok_or_else(|| anyhow!("resolved data asset '{}' is missing", key.name))?;
            let registration = context.registration(&key.type_name)?;
            let asset = with_context(Arc::clone(&context), || {
                (registration.factory)(data, &context).with_context(|| {
                    format!(
                        "Failed to deserialize data asset '{}' of type '{}'",
                        key.name, key.type_name
                    )
                })
            })?;
            cell.set(asset)?;
        }

        let mut awake = assets.values().map(|cell| cell.link()).collect::<Vec<_>>();
        awake.extend(context.nested_assets()?);
        Ok(Self {
            assets,
            aliases,
            awake,
            context,
        })
    }

    pub(crate) fn awake_assets(&self) -> Result<()> {
        for cell in &self.awake {
            cell.with_value(|cell| {
                let asset = cell
                    .asset()
                    .ok_or_else(|| anyhow!("data asset cell was not initialized"))?;
                asset.awake(self)
            })
            .ok_or_else(|| anyhow!("data asset expired before awakening"))??;
        }
        Ok(())
    }

    pub(crate) fn resolve_reference_path(
        &self,
        reference: &str,
        type_name: Option<&str>,
    ) -> Result<String> {
        let reference = self.context.reference_name(reference)?;
        let mut candidates = self
            .assets
            .keys()
            .filter(|key| {
                key.name == reference && type_name.is_none_or(|name| name == key.type_name)
            })
            .map(|key| key.name.clone())
            .collect::<Vec<_>>();
        for (alias, keys) in &self.aliases {
            if alias.name == reference && type_name.is_none_or(|name| name == alias.type_name) {
                candidates.extend(keys.iter().map(|key| key.name.clone()));
            }
        }
        candidates.sort();
        candidates.dedup();
        let type_description = type_name
            .map(|name| format!(" of type '{name}'"))
            .unwrap_or_default();
        match candidates.as_slice() {
            [path] => Ok(path.clone()),
            [] => bail!(
                "data asset '{reference}'{type_description} was not found; names are development-only, use an asset-relative .data path"
            ),
            _ => bail!("ambiguous data asset '{reference}'; specify its type or use a path"),
        }
    }

    pub fn get<T: DataAssetType>(&self, name: &str) -> Option<DataRef<T>> {
        match self.try_get(name) {
            Ok(reference) => reference,
            Err(error) => {
                log::error!("Failed to resolve data asset: {error:#}");
                None
            }
        }
    }

    pub fn try_get<T: DataAssetType>(&self, name: &str) -> Result<Option<DataRef<T>>> {
        let name = self.context.reference_name(name)?;
        let mut key = DataAssetKey {
            type_name: T::TYPE_NAME.to_owned(),
            name: name.clone(),
        };
        if !self.assets.contains_key(&key)
            && let Some(candidates) = self.aliases.get(&key)
        {
            if candidates.len() != 1 {
                bail!(
                    "ambiguous data asset '{}' of type '{}'; use a path",
                    name,
                    T::TYPE_NAME
                );
            }
            key = candidates[0].clone();
        }
        let Some(cell) = self.assets.get(&key) else {
            return Ok(None);
        };
        Ok(Some(DataRef::linked(cell.link())))
    }

    pub fn get_by_name<T: DataAssetType>(&self, name: &str) -> Option<DataRef<T>> {
        self.get(name)
    }

    pub fn names<T: DataAssetType>(&self) -> Vec<&str> {
        let mut names = self
            .assets
            .keys()
            .filter(move |key| key.type_name == T::TYPE_NAME)
            .map(|key| key.name.as_str())
            .collect::<Vec<_>>();
        names.sort_unstable();
        names
    }

    pub fn registered_types(&self) -> Vec<&'static str> {
        self.context.registered_types()
    }
}

struct ParsedDocument {
    type_name: String,
    name: String,
    parent: Option<String>,
    data: Value,
    path: PathBuf,
}

fn collect_registrations() -> Result<HashMap<&'static str, &'static DataAssetRegistration>> {
    let mut registrations = HashMap::new();
    for registration in inventory::iter::<DataAssetRegistration> {
        if registrations
            .insert(registration.type_name, registration)
            .is_some()
        {
            bail!(
                "multiple DataAsset registrations use type name '{}'",
                registration.type_name
            );
        }
    }
    Ok(registrations)
}

fn parse_document(value: Value, fallback_name: &str, path: PathBuf) -> Result<ParsedDocument> {
    let mut mapping = value
        .as_mapping()
        .cloned()
        .ok_or_else(|| anyhow!("data asset root must be a mapping in {}", path.display()))?;
    let type_name = take_string(&mut mapping, "type")?
        .ok_or_else(|| anyhow!("data asset type is required in {}", path.display()))?;
    let name = take_string(&mut mapping, "name")?.unwrap_or_else(|| fallback_name.to_owned());
    if name.trim().is_empty() {
        bail!("data asset name must not be empty in {}", path.display());
    }
    let parent = match mapping.remove(Value::String("parent".to_owned())) {
        Some(Value::String(parent)) => Some(parent),
        Some(Value::Mapping(mut parent)) => take_string(&mut parent, "ref")?,
        Some(Value::Null) | None => None,
        Some(_) => bail!("data asset parent must be a name in {}", path.display()),
    };
    let data = mapping
        .remove(Value::String("data".to_owned()))
        .unwrap_or_else(|| Value::Mapping(mapping));
    Ok(ParsedDocument {
        type_name,
        name,
        parent,
        data,
        path,
    })
}

fn take_string(mapping: &mut Mapping, key: &str) -> Result<Option<String>> {
    match mapping.remove(Value::String(key.to_owned())) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value)),
        Some(_) => bail!("data asset field '{key}' must be a non-empty string"),
        None => Ok(None),
    }
}

fn resolve_document(
    key: &DataAssetKey,
    documents: &HashMap<DataAssetKey, ParsedDocument>,
    resolved: &mut HashMap<DataAssetKey, Value>,
    resolving: &mut HashSet<DataAssetKey>,
) -> Result<Value> {
    if let Some(value) = resolved.get(key) {
        return Ok(value.clone());
    }
    let document = documents
        .get(key)
        .ok_or_else(|| anyhow!("parent data asset '{}' was not found", key.name))?;
    if !resolving.insert(key.clone()) {
        bail!(
            "data asset inheritance cycle detected at '{}' of type '{}'",
            key.name,
            key.type_name
        );
    }
    let value = if let Some(parent_name) = &document.parent {
        let parent_key = DataAssetKey {
            type_name: key.type_name.clone(),
            name: parent_name.clone(),
        };
        let parent =
            resolve_document(&parent_key, documents, resolved, resolving).with_context(|| {
                format!(
                    "Failed to resolve parent '{}' for data asset '{}'",
                    parent_name, key.name
                )
            })?;
        crate::data::yaml::merge_values(parent, document.data.clone())?
    } else {
        document.data.clone()
    };
    let was_resolving = resolving.remove(key);
    assert!(
        was_resolving,
        "resolution bookkeeping must contain every in-flight data asset"
    );
    resolved.insert(key.clone(), value.clone());
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::data_asset::{DataAsset, DataAssetDeserializeContext, DataRef};
    use component_macros::DataAsset;
    use serde::{Deserialize, Serialize};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(DataAsset, Debug, Deserialize, Serialize)]
    struct TestAsset {
        value: String,
        #[serde(default)]
        link: Option<DataRef<TestAsset>>,
        #[serde(default)]
        nested: Option<DataRef<TestAsset>>,
    }

    #[derive(DataAsset, Debug, Deserialize, Serialize)]
    struct LifetimeAsset {
        value: u32,
        #[serde(default)]
        link: Option<DataRef<LifetimeAsset>>,
        #[serde(default)]
        nested: Option<DataRef<LifetimeAsset>>,
        #[serde(skip)]
        drops: Arc<AtomicUsize>,
    }

    impl Drop for LifetimeAsset {
        fn drop(&mut self) {
            assert_eq!(
                self.drops.fetch_add(1, Ordering::SeqCst),
                0,
                "data asset must be destroyed exactly once"
            );
        }
    }

    #[derive(DataAsset, Debug, Serialize)]
    #[data_asset(
        deserialize_with = deserialize_custom_asset,
        serialize_with = serialize_custom_asset
    )]
    struct CustomAsset {
        value: String,
    }

    fn deserialize_custom_asset(
        value: &Value,
        _context: &DataAssetDeserializeContext,
    ) -> Result<CustomAsset> {
        let value = value
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("custom data asset requires a string value"))?;
        Ok(CustomAsset {
            value: value.to_owned(),
        })
    }

    fn serialize_custom_asset(asset: &CustomAsset) -> Result<Value> {
        let mut value = serde_yaml::to_value(asset)?;
        value["value"] = Value::String(format!("custom:{}", asset.value));
        Ok(value)
    }

    fn document(yaml: &str, filename: &str) -> (String, Value, PathBuf) {
        (
            filename.to_owned(),
            serde_yaml::from_str(yaml).unwrap(),
            PathBuf::from(filename),
        )
    }

    #[test]
    fn loads_cycles_nested_assets_and_inheritance() {
        let database = DataAssetDatabase::from_documents(vec![
            document(
                "type: TestAsset\nname: A\ndata:\n  value: child\n  link: { ref: B }\n  nested: { value: nested }\n",
                "a.data",
            ),
            document(
                "type: TestAsset\nname: B\nparent: Base\ndata:\n  link: { ref: A }\n",
                "b.data",
            ),
            document(
                "type: TestAsset\nname: Base\ndata:\n  value: inherited\n",
                "base.data",
            ),
        ])
        .unwrap();

        let a = database.get::<TestAsset>("A").unwrap();
        assert_eq!(
            a.with(|asset| asset.value.clone()),
            Some("child".to_owned())
        );
        assert_eq!(
            a.with(|asset| asset
                .link
                .as_ref()
                .and_then(|link| { link.with(|linked| linked.link.is_some()) })),
            Some(Some(true))
        );
        assert_eq!(
            a.with(|asset| asset.nested.as_ref().map(DataRef::is_nested)),
            Some(Some(true))
        );
        assert_eq!(
            database
                .get::<TestAsset>("B")
                .unwrap()
                .with(|asset| asset.value.clone()),
            Some("inherited".to_owned())
        );
    }

    #[test]
    fn cyclic_links_and_nested_owners_are_destroyed_without_leaks() {
        let database = DataAssetDatabase::from_documents(vec![
            document("type: LifetimeAsset\nname: A\ndata: {value: 1, link: {ref: B}, nested: {value: 3, link: {ref: A}}}", "a.data"),
            document("type: LifetimeAsset\nname: B\ndata: {value: 2, link: {ref: A}}", "b.data"),
        ]).unwrap();
        let a = database.get::<LifetimeAsset>("A").unwrap();
        let b = database.get::<LifetimeAsset>("B").unwrap();
        let a_drops = a.with(|asset| Arc::clone(&asset.drops)).unwrap();
        let b_drops = b.with(|asset| Arc::clone(&asset.drops)).unwrap();
        let nested_drops = a
            .with(|asset| {
                asset
                    .nested
                    .as_ref()
                    .unwrap()
                    .with(|nested| Arc::clone(&nested.drops))
                    .unwrap()
            })
            .unwrap();
        assert_eq!(
            a.with(|asset| asset.link.as_ref().unwrap().with(|other| other.value)),
            Some(Some(2))
        );
        let serialized = serde_yaml::to_value(&a).unwrap();
        assert_eq!(serialized["ref"], "A");
        assert_eq!(serialized["type"], "LifetimeAsset");
        drop(database);
        assert!(!a.is_alive());
        assert!(!b.is_alive());
        assert!(a.with(|_| ()).is_none());
        for drops in [a_drops, b_drops, nested_drops] {
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        }
        assert!(
            serde_yaml::to_string(&a)
                .unwrap_err()
                .to_string()
                .contains("cannot serialize expired")
        );
        assert!(format!("{a:?}").contains("<expired>"));
    }

    #[test]
    fn inline_assets_are_uniquely_owned_outside_the_database() {
        let database = DataAssetDatabase::from_documents(vec![document(
            "type: LifetimeAsset\nname: Root\ndata: {value: 1}",
            "root.data",
        )])
        .unwrap();
        let inline: DataRef<LifetimeAsset> = with_context(Arc::clone(&database.context), || {
            Ok(serde_yaml::from_value(serde_yaml::from_str(
                "value: 2\nlink: {ref: Root}\nnested: {value: 3}",
            )?)?)
        })
        .unwrap();
        let drops = inline.with(|asset| Arc::clone(&asset.drops)).unwrap();
        let nested_drops = inline
            .with(|asset| {
                asset
                    .nested
                    .as_ref()
                    .unwrap()
                    .with(|nested| Arc::clone(&nested.drops))
                    .unwrap()
            })
            .unwrap();
        assert!(inline.is_nested());
        let serialized = serde_yaml::to_value(&inline).unwrap();
        assert_eq!(serialized["type"], "LifetimeAsset");
        assert_eq!(serialized["data"]["link"]["ref"], "Root");
        assert_eq!(serialized["data"]["nested"]["data"]["value"], 3);
        drop(database);
        assert!(inline.is_alive());
        assert_eq!(
            inline.with(|asset| asset.link.as_ref().unwrap().is_alive()),
            Some(false)
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(inline);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(nested_drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_the_database_inside_a_callback_expires_links_without_invalidating_the_borrow() {
        let database = DataAssetDatabase::from_documents(vec![
            document("type: LifetimeAsset\nname: Root\ndata: {value: 1, nested: {value: 2, link: {ref: Root}}}", "root.data"),
        ]).unwrap();
        let root = database.get::<LifetimeAsset>("Root").unwrap();
        let drops = root.with(|asset| Arc::clone(&asset.drops)).unwrap();
        let result = root.with(|asset| {
            drop(database);
            assert!(!root.is_alive());
            assert!(root.with(|_| ()).is_none());
            assert_eq!(
                drops.load(Ordering::SeqCst),
                0,
                "the current callback must retain a valid borrow"
            );
            let nested = asset.nested.as_ref().unwrap();
            assert!(
                nested.is_alive(),
                "inline assets belong to their still-borrowed parent"
            );
            assert_eq!(
                nested.with(|nested| nested.link.as_ref().unwrap().is_alive()),
                Some(false)
            );
            asset.value + nested.with(|nested| nested.value).unwrap()
        });
        assert_eq!(result, Some(3));
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn concurrent_owner_destruction_waits_only_for_active_callbacks() {
        use std::sync::mpsc;
        use std::time::Duration;
        let database = DataAssetDatabase::from_documents(vec![document(
            "type: LifetimeAsset\nname: Root\ndata: {value: 42}",
            "root.data",
        )])
        .unwrap();
        let root = database.get::<LifetimeAsset>("Root").unwrap();
        let observer = database.get::<LifetimeAsset>("Root").unwrap();
        let drops = root.with(|asset| Arc::clone(&asset.drops)).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            root.with(|asset| {
                entered_tx.send(()).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                asset.value
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(database);
        let alive = observer.is_alive();
        let dropped_while_reading = drops.load(Ordering::SeqCst);
        resume_tx.send(()).unwrap();
        assert_eq!(reader.join().unwrap(), Some(42));
        assert!(!alive);
        assert_eq!(dropped_while_reading, 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn ignores_duplicate_assets_with_the_same_type_and_name() {
        let database = DataAssetDatabase::from_documents(vec![
            document(
                "type: TestAsset\nname: duplicate\ndata: { value: first }\n",
                "first.data",
            ),
            document(
                "type: TestAsset\nname: duplicate\ndata: { value: second }\n",
                "second.data",
            ),
        ])
        .unwrap();
        assert_eq!(
            database
                .get::<TestAsset>("duplicate")
                .unwrap()
                .with(|asset| asset.value.clone()),
            Some("first".to_owned())
        );
    }

    #[test]
    fn uses_custom_deserializer_and_serializer_hooks() {
        let database = DataAssetDatabase::from_documents(vec![document(
            "type: CustomAsset\nname: custom\ndata: { value: original }\n",
            "custom.data",
        )])
        .unwrap();
        let asset = database.get::<CustomAsset>("custom").unwrap();
        assert_eq!(
            asset.with(|asset| asset.value.clone()),
            Some("original".to_owned())
        );
        assert_eq!(
            asset.with(|asset| asset.serialize_data().unwrap()),
            Some(serde_yaml::from_str("value: custom:original").unwrap())
        );
    }

    fn error_of(documents: Vec<(String, Value, PathBuf)>) -> String {
        match DataAssetDatabase::from_documents(documents) {
            Ok(_) => panic!("expected the document set to be rejected"),
            Err(error) => format!("{error:#}"),
        }
    }

    #[test]
    fn rejects_unregistered_asset_types() {
        let error = error_of(vec![document(
            "type: NoSuchAssetType\nname: broken\ndata: {}\n",
            "broken.data",
        )]);
        assert!(
            error.contains("unregistered data asset type 'NoSuchAssetType'"),
            "unexpected error: {error}"
        );
        assert!(error.contains("broken.data"), "unexpected error: {error}");
    }

    #[test]
    fn rejects_inheritance_cycles_and_missing_parents() {
        let error = error_of(vec![
            document("type: TestAsset\nname: A\nparent: B\ndata: {}\n", "a.data"),
            document("type: TestAsset\nname: B\nparent: A\ndata: {}\n", "b.data"),
        ]);
        assert!(
            error.contains("data asset inheritance cycle detected"),
            "unexpected error: {error}"
        );

        let error = error_of(vec![document(
            "type: TestAsset\nname: Orphan\nparent: Ghost\ndata: { value: v }\n",
            "orphan.data",
        )]);
        assert!(
            error.contains("parent data asset 'Ghost' was not found"),
            "unexpected error: {error}"
        );

        let error = error_of(vec![document(
            "type: TestAsset\nname: SelfParent\nparent: SelfParent\ndata: { value: v }\n",
            "self.data",
        )]);
        assert!(
            error.contains("data asset inheritance cycle detected at 'SelfParent'"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rejects_malformed_documents() {
        let error = error_of(vec![document("- not\n- a\n- mapping\n", "list.data")]);
        assert!(
            error.contains("data asset root must be a mapping"),
            "unexpected error: {error}"
        );

        let error = error_of(vec![document("name: untyped\ndata: {}\n", "untyped.data")]);
        assert!(
            error.contains("data asset type is required"),
            "unexpected error: {error}"
        );

        let error = error_of(vec![document(
            "type: TestAsset\nname: \"  \"\ndata: {}\n",
            "blank.data",
        )]);
        assert!(
            error.contains("data asset field 'name' must be a non-empty string"),
            "unexpected error: {error}"
        );

        let error = error_of(vec![document(
            "type: TestAsset\nname: badparent\nparent: [list]\ndata: {}\n",
            "badparent.data",
        )]);
        assert!(
            error.contains("data asset parent must be a name"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn missing_data_block_uses_remaining_fields_as_data() {
        let database = DataAssetDatabase::from_documents(vec![document(
            "type: TestAsset\nname: inline\nvalue: inline-data\n",
            "inline.data",
        )])
        .unwrap();
        assert_eq!(
            database
                .get::<TestAsset>("inline")
                .unwrap()
                .with(|asset| asset.value.clone()),
            Some("inline-data".to_owned())
        );
    }

    #[test]
    fn load_dir_rejects_missing_directories() {
        let missing = std::env::temp_dir().join("engine-data-assets-do-not-exist");
        let error = match DataAssetDatabase::load_dir(&missing) {
            Ok(_) => panic!("expected the missing directory to be rejected"),
            Err(error) => format!("{error:#}"),
        };
        assert!(
            error.contains("data asset directory does not exist"),
            "unexpected error: {error}"
        );
    }
}
