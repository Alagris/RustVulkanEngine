use crate::asset_database::{AssetDatabase, read_yaml};
use anyhow::{Context, Result, anyhow, bail};
use engine::data::yaml::merge_values;
use engine::ecs::{ComponentRegistry, GameObject, Scene, Unique};
use serde_yaml::{Mapping, Value};
use std::path::{Path, PathBuf};

/// Resolved editor-side prefab data; instances are ordinary scene GameObjects.
///
/// Mappings merge recursively. Arrays merge by `id` when every entry is identified; other arrays
/// replace inherited values. An id-keyed entry with `remove: true` deletes an inherited entry.
#[derive(Clone, Debug)]
pub struct Prefab {
    pub name: String,
    pub root: Value,
}

impl Prefab {
    pub fn load(name: &str, assets: &AssetDatabase) -> Result<Self> {
        let mut stack = Vec::new();
        let root = assets
            .resolve_prefab_root(name, &mut stack)
            .with_context(|| format!("Failed to resolve prefab '{name}'"))?;
        Ok(Self {
            name: name.to_owned(),
            root,
        })
    }

    pub fn instantiate(
        &self,
        scene: &mut Scene,
        registry: &ComponentRegistry,
    ) -> Result<Unique<GameObject>> {
        let source = serde_yaml::to_string(&self.root)
            .with_context(|| format!("Failed to serialize prefab '{}'", self.name))?;
        scene
            .instantiate_game_object(&source, registry)
            .with_context(|| format!("Failed to instantiate prefab '{}'", self.name))
    }

    pub fn expand_scene(scene: &mut Value, assets: &AssetDatabase) -> Result<()> {
        let root = scene
            .as_mapping_mut()
            .and_then(|scene| scene.get_mut(Value::String("root".to_owned())))
            .ok_or_else(|| anyhow!("scene must contain a root game object"))?;
        let mut stack = Vec::new();
        expand_game_object(root, assets, &mut stack)?;
        let source = serde_yaml::to_string(scene).context("Failed to serialize expanded scene")?;
        Scene::validate_yaml(&source).context("Expanded scene does not match the scene schema")
    }
}

pub(crate) fn load_prefab_path(
    path: &Path,
    assets: &AssetDatabase,
    stack: &mut Vec<PathBuf>,
) -> Result<Value> {
    let path = path
        .canonicalize()
        .with_context(|| format!("Failed to resolve prefab path {}", path.display()))?;
    if stack.contains(&path) {
        let mut chain = stack
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        chain.push(path.display().to_string());
        bail!(
            "cyclic prefab inheritance or instantiation: {}",
            chain.join(" -> ")
        );
    }
    stack.push(path.clone());
    let result = (|| {
        let mut document = read_yaml(&path)?;
        let mapping = document
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("{} must contain a mapping", path.display()))?;
        let parent = mapping
            .remove(Value::String("parent".to_owned()))
            .map(|parent| {
                parent
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow!("{} parent must be a prefab asset name", path.display()))
            })
            .transpose()?;
        let root_patch = mapping
            .remove(Value::String("root".to_owned()))
            .or_else(|| parent.as_ref().map(|_| Value::Mapping(Mapping::new())))
            .ok_or_else(|| {
                anyhow!(
                    "{} must define root or inherit a parent prefab",
                    path.display()
                )
            })?;
        if !mapping.is_empty() {
            bail!(
                "{} contains unsupported prefab fields: {}",
                path.display(),
                mapping
                    .keys()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        let base = match parent {
            Some(parent) => assets
                .resolve_prefab_root(&parent, stack)
                .with_context(|| {
                    format!("Parent prefab '{parent}' of {} is invalid", path.display())
                })?,
            None => Value::Mapping(Mapping::new()),
        };
        let mut root = merge_values(base, root_patch)?;
        if !root.is_mapping() {
            bail!("{} root must be a mapping", path.display());
        }
        expand_game_object(&mut root, assets, stack)
            .with_context(|| format!("Failed to expand prefab contents in {}", path.display()))?;
        let scene = scene_with_root(root.clone());
        let source = serde_yaml::to_string(&scene)?;
        Scene::validate_yaml(&source).with_context(|| {
            format!("{} does not resolve to a valid game object", path.display())
        })?;
        Ok(root)
    })();
    stack.pop();
    result
}

fn expand_game_object(
    object: &mut Value,
    assets: &AssetDatabase,
    stack: &mut Vec<PathBuf>,
) -> Result<()> {
    let mapping = object
        .as_mapping_mut()
        .ok_or_else(|| anyhow!("game object must be a mapping"))?;
    let prefab = mapping.remove(Value::String("prefab".to_owned()));
    if let Some(prefab) = prefab {
        let name = prefab
            .as_str()
            .ok_or_else(|| anyhow!("game object prefab must name a prefab asset"))?;
        let base = assets
            .resolve_prefab_root(name, stack)
            .with_context(|| format!("Failed to instantiate prefab '{name}'"))?;
        let patch = Value::Mapping(mapping.clone());
        *object = merge_values(base, patch)?;
        let mapping = object
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("resolved prefab root must be a mapping"))?;
        expand_children(mapping, assets, stack)?;
    } else {
        expand_children(mapping, assets, stack)?;
    }
    Ok(())
}

fn expand_children(
    mapping: &mut Mapping,
    assets: &AssetDatabase,
    stack: &mut Vec<PathBuf>,
) -> Result<()> {
    let Some(children) = mapping.get_mut(Value::String("children".to_owned())) else {
        return Ok(());
    };
    let children = children
        .as_sequence_mut()
        .ok_or_else(|| anyhow!("game object children must be a sequence"))?;
    for child in children {
        expand_game_object(child, assets, stack)?;
    }
    Ok(())
}

fn scene_with_root(root: Value) -> Value {
    let mut scene = Mapping::new();
    scene.insert(Value::String("root".to_owned()), root);
    Value::Mapping(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_database::file_nonce;
    use std::fs;

    fn temp_dir() -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "editor-prefab-{}-{}",
            std::process::id(),
            file_nonce()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn inherits_mapping_values_and_merges_id_keyed_children() {
        let root = temp_dir();
        fs::write(
            root.join("Base.prefab"),
            r#"
root:
  name: BaseCar
  transform:
    translation: [0.0, 0.0, 0.0]
  children:
    - id: wheel
      name: Wheel
      transform:
        translation: [1.0, 0.0, 0.0]
    - id: old-part
      name: OldPart
"#,
        )
        .unwrap();
        let base_source = fs::read_to_string(root.join("Base.prefab")).unwrap();
        fs::write(
            root.join("SportsCar.prefab"),
            r#"
parent: Base
root:
  name: SportsCar
  transform:
    translation: [5.0, 0.0, 0.0]
  children:
    - id: wheel
      transform:
        translation: [2.0, 0.0, 0.0]
    - id: old-part
      remove: true
    - id: trunk
      name: Trunk
"#,
        )
        .unwrap();

        let assets = AssetDatabase::scan(&root).unwrap();
        let prefab = assets.load_prefab("SportsCar").unwrap();
        assert_eq!(prefab.root["name"].as_str(), Some("SportsCar"));
        assert_eq!(
            prefab.root["transform"]["translation"][0].as_f64(),
            Some(5.0)
        );
        assert_eq!(prefab.root["children"].as_sequence().unwrap().len(), 2);
        assert_eq!(prefab.root["children"][0]["name"].as_str(), Some("Wheel"));
        assert_eq!(
            prefab.root["children"][0]["transform"]["translation"][0].as_f64(),
            Some(2.0)
        );
        assert_eq!(prefab.root["children"][1]["name"].as_str(), Some("Trunk"));

        let mut scene: Value = serde_yaml::from_str(
            r#"
root:
  name: World
  children:
    - prefab: SportsCar
      name: PlacedCar
      transform:
        translation: [10.0, 0.0, 0.0]
"#,
        )
        .unwrap();
        Prefab::expand_scene(&mut scene, &assets).unwrap();
        assert_eq!(
            scene["root"]["children"][0]["name"].as_str(),
            Some("PlacedCar")
        );
        assert_eq!(
            scene["root"]["children"][0]["children"][0]["name"].as_str(),
            Some("Wheel")
        );

        fs::write(
            root.join("Base.prefab"),
            base_source.replace("name: Wheel", "name: UpdatedWheel"),
        )
        .unwrap();
        assert_eq!(
            assets.load_prefab("SportsCar").unwrap().root["children"][0]["name"].as_str(),
            Some("Wheel"),
            "resolved prefab data should be served from the cache"
        );
        assets.clear_prefab_cache().unwrap();
        assert_eq!(
            assets.load_prefab("SportsCar").unwrap().root["children"][0]["name"].as_str(),
            Some("UpdatedWheel")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn instantiate_returns_a_scene_managed_game_object() {
        let root = temp_dir();
        fs::write(
            root.join("CameraRig.prefab"),
            "root:\n  name: CameraRig\n  components:\n    - type: Camera\n",
        )
        .unwrap();
        fs::write(
            root.join("world.scene"),
            "root:\n  name: World\n  children:\n    - prefab: CameraRig\n",
        )
        .unwrap();
        let assets = AssetDatabase::scan(&root).unwrap();
        let prefab = assets.load_prefab("CameraRig").unwrap();
        let expanded_scene = assets
            .prepare_development_scene(&root.join("world.scene"))
            .unwrap();
        let expanded_source = fs::read_to_string(&expanded_scene).unwrap();
        assert!(!expanded_source.contains("prefab:"));
        Scene::validate_yaml(&expanded_source).unwrap();
        let package_output = temp_dir();
        let packaged_scene = package_output.join("world.scene");
        assets
            .process_scene(&root.join("world.scene"), &packaged_scene)
            .unwrap();
        assert!(
            !fs::read_to_string(&packaged_scene)
                .unwrap()
                .contains("prefab:")
        );
        let registry = ComponentRegistry::new();
        let mut scene = Scene::from_yaml("root:\n  name: World\n", &registry).unwrap();
        scene.initialize().unwrap();

        let instance = prefab.instantiate(&mut scene, &registry).unwrap();
        assert_eq!(instance.borrow().name, "CameraRig");
        assert_eq!(instance.borrow().components(), 1);
        assert_eq!(scene.component_count(), 1);
        fs::remove_dir_all(expanded_scene.parent().unwrap()).unwrap();
        fs::remove_dir_all(package_output).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_cyclic_prefab_inheritance() {
        let root = temp_dir();
        fs::write(
            root.join("First.prefab"),
            "parent: Second\nroot:\n  name: First\n",
        )
        .unwrap();
        fs::write(
            root.join("Second.prefab"),
            "parent: First\nroot:\n  name: Second\n",
        )
        .unwrap();
        let error = AssetDatabase::scan(&root).err().unwrap();
        assert!(format!("{error:#}").contains("cyclic prefab inheritance"));
        fs::remove_dir_all(root).unwrap();
    }
}
