use crate::data::data_asset::{
    DataAssetCell, DataAssetDeserializeContext, DataAssetKey, DataAssetRegistration, DataAssetType,
    DataRef, inventory, with_context,
};
use anyhow::{Context, Result, anyhow, bail};
use serde_yaml::{Mapping, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

pub struct DataAssetDatabase {
    assets: HashMap<DataAssetKey, Arc<DataAssetCell>>,
    registrations: HashMap<&'static str, &'static DataAssetRegistration>,
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
                    Arc::new(DataAssetCell::new(
                        Some(document.name.clone()),
                        registration,
                    )),
                )
            })
            .collect::<HashMap<_, _>>();
        let weak_assets = assets
            .iter()
            .map(|(key, cell)| (key.clone(), Arc::downgrade(cell)))
            .collect::<HashMap<_, Weak<_>>>();
        let context = Arc::new(DataAssetDeserializeContext::new(
            registrations.clone(),
            weak_assets,
        ));

        for (key, cell) in &assets {
            let data = resolved
                .get(key)
                .ok_or_else(|| anyhow!("resolved data asset '{}' is missing", key.name))?;
            let registration = registrations[key.type_name.as_str()];
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

        let database = Self {
            assets,
            registrations,
        };
        let mut awake_assets = database.assets.values().cloned().collect::<Vec<_>>();
        awake_assets.extend(context.nested_assets()?);
        for cell in awake_assets {
            let asset = cell
                .asset()
                .ok_or_else(|| anyhow!("data asset cell was not initialized"))?;
            asset.awake(&database)?;
        }

        Ok(database)
    }

    pub fn get<T: DataAssetType>(&self, name: &str) -> Option<DataRef<T>> {
        let key = DataAssetKey {
            type_name: T::TYPE_NAME.to_owned(),
            name: name.to_owned(),
        };
        let cell = self.assets.get(&key)?;
        Some(DataRef::linked(
            Arc::clone(cell),
            name.to_owned(),
            T::TYPE_NAME.to_owned(),
        ))
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
        let mut types = self.registrations.keys().copied().collect::<Vec<_>>();
        types.sort_unstable();
        types
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
    resolving.remove(key);
    resolved.insert(key.clone(), value.clone());
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::data_asset::{DataAsset, DataAssetDeserializeContext, DataRef};
    use component_macros::DataAsset;
    use serde::{Deserialize, Serialize};

    #[derive(DataAsset, Debug, Deserialize, Serialize)]
    struct TestAsset {
        value: String,
        #[serde(default)]
        link: Option<DataRef<TestAsset>>,
        #[serde(default)]
        nested: Option<DataRef<TestAsset>>,
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
}
