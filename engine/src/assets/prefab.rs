use super::asset_ref::AssetContext;
use crate::data::yaml::merge_values;
use crate::ecs::{ComponentRegistry, GameObject, Scene, Unique};
use anyhow::{Context, Result, anyhow, bail};
use serde_yaml::{Mapping, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Resolves inheritance and eager prefab instances without depending on the editor.
pub trait PrefabResolver {
    fn resolve_prefab_root(&self, reference: &str, stack: &mut Vec<PathBuf>) -> Result<Value>;
}

/// Expanded template. Asset references are weak; the owning AssetStore must remain alive.
#[derive(Clone, Debug)]
pub struct Prefab {
    pub name: String,
    pub root: Value,
    pub(crate) context: Arc<AssetContext>,
}

impl Prefab {
    pub fn instantiate(
        &self,
        scene: &mut Scene,
        registry: &ComponentRegistry,
    ) -> Result<Unique<GameObject>> {
        let source = serde_yaml::to_string(&self.root)?;
        self.context
            .with(|| scene.instantiate_game_object(&source, registry))
            .with_context(|| format!("Failed to instantiate prefab '{}'", self.name))
    }

    pub fn expand_scene(scene: &mut Value, resolver: &impl PrefabResolver) -> Result<()> {
        let root = scene
            .get_mut("root")
            .ok_or_else(|| anyhow!("scene must contain a root game object"))?;
        expand_game_object(root, resolver, &mut Vec::new())?;
        Scene::validate_yaml(&serde_yaml::to_string(scene)?)
            .context("Expanded scene does not match the scene schema")
    }
}

pub fn load_prefab_path(
    path: &Path,
    resolver: &impl PrefabResolver,
    stack: &mut Vec<PathBuf>,
) -> Result<Value> {
    let path = path
        .canonicalize()
        .with_context(|| format!("Failed to resolve prefab path {}", path.display()))?;
    if stack.contains(&path) {
        let chain = stack
            .iter()
            .chain(std::iter::once(&path))
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        bail!(
            "cyclic prefab inheritance or instantiation: {}",
            chain.join(" -> ")
        );
    }
    stack.push(path.clone());
    let depth = stack.len();
    let result = (|| {
        let mut document: Value = serde_yaml::from_str(&fs::read_to_string(&path)?)?;
        let mapping = document
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("{} must contain a mapping", path.display()))?;
        let parent = mapping
            .remove("parent")
            .map(|parent| {
                parent.as_str().map(str::to_owned).ok_or_else(|| {
                    anyhow!(
                        "{} parent must be a prefab asset name or path",
                        path.display()
                    )
                })
            })
            .transpose()?;
        let patch = mapping
            .remove("root")
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
            Some(parent) => resolver
                .resolve_prefab_root(&parent, stack)
                .with_context(|| {
                    format!("Parent prefab '{parent}' of {} is invalid", path.display())
                })?,
            None => Value::Mapping(Mapping::new()),
        };
        let mut root = merge_values(base, patch)?;
        expand_game_object(&mut root, resolver, stack)?;
        let scene = Value::Mapping(Mapping::from_iter([(
            Value::String("root".into()),
            root.clone(),
        )]));
        Scene::validate_yaml(&serde_yaml::to_string(&scene)?).with_context(|| {
            format!("{} does not resolve to a valid game object", path.display())
        })?;
        Ok(root)
    })();
    assert_eq!(
        stack.len(),
        depth,
        "prefab resolution stack must be balanced"
    );
    assert_eq!(
        stack.last(),
        Some(&path),
        "prefab resolution stack top changed"
    );
    stack.pop();
    result.with_context(|| format!("Failed to load prefab {}", path.display()))
}

fn expand_game_object(
    object: &mut Value,
    resolver: &impl PrefabResolver,
    stack: &mut Vec<PathBuf>,
) -> Result<()> {
    let mapping = object
        .as_mapping_mut()
        .ok_or_else(|| anyhow!("game object must be a mapping"))?;
    if let Some(prefab) = mapping.remove("prefab") {
        let reference = prefab
            .as_str()
            .ok_or_else(|| anyhow!("game object prefab must name a prefab asset or path"))?;
        let base = resolver
            .resolve_prefab_root(reference, stack)
            .with_context(|| format!("Failed to instantiate prefab '{reference}'"))?;
        *object = merge_values(base, Value::Mapping(mapping.clone()))?;
    }
    if let Some(children) = object.get_mut("children") {
        for child in children
            .as_sequence_mut()
            .ok_or_else(|| anyhow!("game object children must be a sequence"))?
        {
            expand_game_object(child, resolver, stack)?;
        }
    }
    debug_assert!(object.get("prefab").is_none());
    Ok(())
}
