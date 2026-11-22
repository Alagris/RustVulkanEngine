use anyhow::{Context, Result, anyhow, bail};
#[cfg(debug_assertions)]
use engine::ecs::component::{ComponentRegistration, inventory};
use engine::render_pipeline::{preprocess_shader_file, shader_asset_name_from_path};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetType {
    Fbx,
    Material,
    Pipeline,
    Shader,
    Png,
    Text,
    Data,
    Scene,
    Prefab,
}

impl AssetType {
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "fbx" => Some(Self::Fbx),
            "mat" => Some(Self::Material),
            "pipe" => Some(Self::Pipeline),
            "shader" => Some(Self::Shader),
            "png" => Some(Self::Png),
            "txt" => Some(Self::Text),
            "data" => Some(Self::Data),
            "scene" => Some(Self::Scene),
            "prefab" => Some(Self::Prefab),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Fbx => "fbx",
            Self::Material => "mat",
            Self::Pipeline => "pipe",
            Self::Shader => "shader",
            Self::Png => "png",
            Self::Text => "txt",
            Self::Data => "data",
            Self::Scene => "scene",
            Self::Prefab => "prefab",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetRef {
    pub name: String,
    pub asset_type: AssetType,
}

type AssetProcessor = Arc<dyn Fn(&Path, &Path) -> Result<()> + Send + Sync>;

pub struct AssetDatabase {
    root: PathBuf,
    assets: BTreeMap<AssetRef, PathBuf>,
    processors: HashMap<AssetType, AssetProcessor>,
}

impl AssetDatabase {
    pub fn scan(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize().with_context(|| {
            format!(
                "Failed to resolve asset directory {}",
                root.as_ref().display()
            )
        })?;
        if !root.is_dir() {
            bail!("asset root is not a directory: {}", root.display());
        }
        let mut assets = BTreeMap::new();
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory)
                .with_context(|| format!("Failed to scan {}", directory.display()))?
            {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if file_type.is_dir() {
                    pending.push(entry.path());
                    continue;
                }
                if !file_type.is_file() {
                    continue;
                }
                let path = entry.path();
                let Some(asset_type) = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .and_then(AssetType::from_extension)
                else {
                    continue;
                };
                #[cfg(debug_assertions)]
                if matches!(asset_type, AssetType::Scene | AssetType::Prefab) {
                    let document = read_yaml(&path)?;
                    validate_component_constraints(&document).with_context(|| {
                        format!("Invalid component constraints in {}", path.display())
                    })?;
                }
                let file_name = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .filter(|stem| !stem.trim().is_empty())
                    .ok_or_else(|| anyhow!("asset has an invalid filename: {}", path.display()))?
                    .to_owned();
                let name = if asset_type == AssetType::Shader {
                    shader_asset_name_from_path(&path).with_context(|| {
                        format!(
                            "Failed to read declared shader name from {}",
                            path.display()
                        )
                    })?
                } else {
                    file_name
                };
                let key = AssetRef { name, asset_type };
                let canonical = path
                    .canonicalize()
                    .with_context(|| format!("Failed to resolve asset {}", path.display()))?;
                if let Some(previous) = assets.insert(key.clone(), canonical) {
                    bail!(
                        "duplicate {:?} asset name '{}': {} and {}",
                        key.asset_type,
                        key.name,
                        previous.display(),
                        path.display()
                    );
                }
            }
        }
        Ok(Self {
            root,
            assets,
            processors: HashMap::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn assets(&self) -> impl Iterator<Item = (&AssetRef, &PathBuf)> {
        self.assets.iter()
    }

    pub fn resolve(&self, reference: &AssetRef) -> Result<&Path> {
        self.assets
            .get(reference)
            .map(PathBuf::as_path)
            .ok_or_else(|| {
                anyhow!(
                    "asset {:?} named '{}' was not found under {}",
                    reference.asset_type,
                    reference.name,
                    self.root.display()
                )
            })
    }

    pub fn register_processor(
        &mut self,
        asset_type: AssetType,
        processor: impl Fn(&Path, &Path) -> Result<()> + Send + Sync + 'static,
    ) {
        self.processors.insert(asset_type, Arc::new(processor));
    }

    pub fn prepare_development_pipeline(&self, pipeline: &Path) -> Result<PathBuf> {
        let pipeline = pipeline
            .canonicalize()
            .with_context(|| format!("Failed to resolve pipeline {}", pipeline.display()))?;
        let mut pipeline_value = read_yaml(&pipeline)?;
        let passes = pipeline_value
            .get_mut("passes")
            .and_then(serde_yaml::Value::as_sequence_mut)
            .ok_or_else(|| anyhow!("pipeline must contain a passes sequence"))?;
        let output = std::env::temp_dir()
            .join("rust-vulkan-game-engine")
            .join("development")
            .join(format!("{}-{}", std::process::id(), file_nonce()));
        fs::create_dir_all(output.join("materials"))?;
        for pass in passes {
            let material_name = pass
                .get("material")
                .and_then(serde_yaml::Value::as_str)
                .ok_or_else(|| anyhow!("each pipeline pass must name a material asset"))?;
            let material_path = self.resolve(&AssetRef {
                name: material_name.to_owned(),
                asset_type: AssetType::Material,
            })?;
            let mut material_value = read_yaml(material_path)?;
            let material_table = material_value
                .get_mut("material")
                .and_then(serde_yaml::Value::as_mapping_mut)
                .ok_or_else(|| {
                    anyhow!(
                        "{} must contain a material mapping",
                        material_path.display()
                    )
                })?;
            let shader_name = material_table
                .get("shader")
                .and_then(serde_yaml::Value::as_str)
                .ok_or_else(|| {
                    anyhow!(
                        "{} material must name a shader asset",
                        material_path.display()
                    )
                })?;
            let shader_path = self.resolve(&AssetRef {
                name: shader_name.to_owned(),
                asset_type: AssetType::Shader,
            })?;
            material_table.insert(
                serde_yaml::Value::String("shader".to_owned()),
                serde_yaml::Value::String(shader_path.display().to_string()),
            );
            let material_output = output
                .join("materials")
                .join(format!("{}.mat", material_name));
            fs::write(&material_output, serde_yaml::to_string(&material_value)?)
                .with_context(|| format!("Failed to write {}", material_output.display()))?;
            pass.as_mapping_mut()
                .ok_or_else(|| anyhow!("pipeline pass must be a mapping"))?
                .insert(
                    serde_yaml::Value::String("material".to_owned()),
                    serde_yaml::Value::String(material_output.display().to_string()),
                );
        }
        let resolved_pipeline = output.join("pipeline.pipe");
        fs::write(&resolved_pipeline, serde_yaml::to_string(&pipeline_value)?)
            .with_context(|| format!("Failed to write {}", resolved_pipeline.display()))?;
        Ok(resolved_pipeline)
    }

    pub fn package(&self, pipeline: &Path, output: &Path) -> Result<PathBuf> {
        let pipeline = pipeline
            .canonicalize()
            .with_context(|| format!("Failed to resolve pipeline {}", pipeline.display()))?;
        let mut pipeline_value = read_yaml(&pipeline)?;
        let passes = pipeline_value
            .get("passes")
            .and_then(serde_yaml::Value::as_sequence)
            .ok_or_else(|| anyhow!("pipeline must contain a passes sequence"))?;
        let mut materials = BTreeSet::new();
        let mut shaders = BTreeSet::new();
        for pass in passes.iter() {
            let name = pass
                .get("material")
                .and_then(serde_yaml::Value::as_str)
                .ok_or_else(|| anyhow!("each pipeline pass must name a material asset"))?
                .to_owned();
            materials.insert(name.to_owned());
        }
        for material_name in &materials {
            let material_path = self.resolve(&AssetRef {
                name: material_name.clone(),
                asset_type: AssetType::Material,
            })?;
            let material_value = read_yaml(material_path)?;
            let shader_name = material_value
                .get("material")
                .and_then(|material| material.get("shader"))
                .and_then(serde_yaml::Value::as_str)
                .ok_or_else(|| {
                    anyhow!(
                        "{} material must name a shader asset",
                        material_path.display()
                    )
                })?;
            shaders.insert(shader_name.to_owned());
        }

        for pass in pipeline_value
            .get_mut("passes")
            .and_then(serde_yaml::Value::as_sequence_mut)
            .ok_or_else(|| anyhow!("pipeline must contain a passes sequence"))?
        {
            let name = pass
                .get("material")
                .and_then(serde_yaml::Value::as_str)
                .ok_or_else(|| anyhow!("each pipeline pass must name a material asset"))?
                .to_owned();
            pass.as_mapping_mut()
                .ok_or_else(|| anyhow!("pipeline pass must be a mapping"))?
                .insert(
                    serde_yaml::Value::String("material".to_owned()),
                    serde_yaml::Value::String(format!("materials/{name}.mat")),
                );
        }

        let output = output.to_path_buf();
        fs::create_dir_all(&output)?;
        let stage = std::env::temp_dir()
            .join("rust-vulkan-game-engine")
            .join("packaging")
            .join(format!("{}-{}", std::process::id(), file_nonce()));
        fs::create_dir_all(&stage)?;

        let package_result = (|| {
            let pipeline_name = pipeline
                .file_name()
                .ok_or_else(|| anyhow!("pipeline path has no filename"))?;
            let staged_pipeline = stage.join(pipeline_name);
            fs::write(&staged_pipeline, serde_yaml::to_string(&pipeline_value)?)
                .with_context(|| format!("Failed to stage {}", pipeline.display()))?;
            let packaged_pipeline = output.join(pipeline_name);
            self.process(AssetType::Pipeline, &staged_pipeline, &packaged_pipeline)?;

            for material_name in &materials {
                let source = self.resolve(&AssetRef {
                    name: material_name.clone(),
                    asset_type: AssetType::Material,
                })?;
                let mut value = read_yaml(source)?;
                let material_table = value
                    .get_mut("material")
                    .and_then(serde_yaml::Value::as_mapping_mut)
                    .ok_or_else(|| {
                        anyhow!("{} must contain a material mapping", source.display())
                    })?;
                let shader_name = material_table
                    .get("shader")
                    .and_then(serde_yaml::Value::as_str)
                    .ok_or_else(|| {
                        anyhow!("{} material must name a shader asset", source.display())
                    })?;
                material_table.insert(
                    serde_yaml::Value::String("shader".to_owned()),
                    serde_yaml::Value::String(format!("../shaders/{shader_name}.shader")),
                );
                let staged_material = stage.join(format!("{material_name}.mat"));
                fs::write(&staged_material, serde_yaml::to_string(&value)?)
                    .with_context(|| format!("Failed to stage {}", source.display()))?;
                self.process(
                    AssetType::Material,
                    &staged_material,
                    &output
                        .join("materials")
                        .join(format!("{material_name}.mat")),
                )?;
            }

            for shader_name in &shaders {
                let source = self.resolve(&AssetRef {
                    name: shader_name.clone(),
                    asset_type: AssetType::Shader,
                })?;
                let expanded_shader = preprocess_shader_file(source)?;
                let staged_shader = stage.join(format!("{shader_name}.shader"));
                fs::write(&staged_shader, expanded_shader)
                    .with_context(|| format!("Failed to stage shader {}", source.display()))?;
                self.process(
                    AssetType::Shader,
                    &staged_shader,
                    &output.join("shaders").join(format!("{shader_name}.shader")),
                )?;
            }
            Ok(output.join(pipeline_name))
        })();

        let cleanup_result = fs::remove_dir_all(&stage).with_context(|| {
            format!(
                "Failed to clean packaging staging directory {}",
                stage.display()
            )
        });
        match (package_result, cleanup_result) {
            (Ok(path), Ok(())) => Ok(path),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    pub(crate) fn process(
        &self,
        asset_type: AssetType,
        source: &Path,
        destination: &Path,
    ) -> Result<()> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        if let Some(processor) = self.processors.get(&asset_type) {
            processor(source, destination)
        } else {
            fs::copy(source, destination)
                .with_context(|| format!("Failed to package {}", source.display()))?;
            Ok(())
        }
    }
}

pub(crate) fn read_yaml(path: &Path) -> Result<serde_yaml::Value> {
    let text =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    serde_yaml::from_str(&text).with_context(|| format!("Invalid YAML in {}", path.display()))
}

#[cfg(debug_assertions)]
fn validate_component_constraints(scene: &serde_yaml::Value) -> Result<()> {
    let root = scene
        .get("root")
        .ok_or_else(|| anyhow!("scene or prefab must contain a root game object"))?;
    validate_game_object(root, "root")
}

#[cfg(debug_assertions)]
fn validate_game_object(object: &serde_yaml::Value, path: &str) -> Result<()> {
    let name = object
        .get("name")
        .and_then(serde_yaml::Value::as_str)
        .ok_or_else(|| anyhow!("{path} must have a string name"))?;
    let object_path = format!("{path} ({name})");
    let empty_components = Vec::new();
    let components = match object.get("components") {
        Some(components) => components
            .as_sequence()
            .ok_or_else(|| anyhow!("{object_path} components must be a sequence"))?,
        None => &empty_components,
    };
    let component_types = components
        .iter()
        .map(|component| {
            component
                .get("type")
                .and_then(serde_yaml::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("{object_path} contains a component without a string type"))
        })
        .collect::<Result<Vec<_>>>()?;

    for component_type in &component_types {
        let mut registrations = inventory::iter::<ComponentRegistration>
            .into_iter()
            .filter(|rules| rules.type_name == component_type);
        let Some(rules) = registrations.next() else {
            continue;
        };
        if registrations.next().is_some() {
            bail!(
                "component rule metadata for '{}' is registered more than once",
                component_type
            );
        }
        if rules.unique
            && component_types
                .iter()
                .filter(|candidate| *candidate == component_type)
                .count()
                > 1
        {
            bail!(
                "{object_path} contains more than one unique component '{}'",
                component_type
            );
        }
        for requirement in rules.required_components {
            let required_name = requirement
                .rsplit("::")
                .next()
                .unwrap_or(requirement)
                .trim();
            if !component_types
                .iter()
                .any(|candidate| candidate == required_name)
            {
                bail!(
                    "{object_path} component '{}' requires component '{}'",
                    component_type,
                    required_name
                );
            }
        }
    }

    if let Some(children) = object.get("children") {
        let children = children
            .as_sequence()
            .ok_or_else(|| anyhow!("{object_path} children must be a sequence"))?;
        for (index, child) in children.iter().enumerate() {
            validate_game_object(child, &format!("{object_path}.children[{index}]"))?;
        }
    }
    Ok(())
}

pub(crate) fn file_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after the Unix epoch")
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(debug_assertions)]
    #[engine::ecs::component]
    #[derive(engine::ecs::Component)]
    #[require(Camera, engine::ecs::Mesh)]
    struct EditorRuleProbe {}

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "editor-{name}-{}-{}",
            std::process::id(),
            file_nonce()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_sample_project(root: &Path) {
        fs::create_dir_all(root.join("materials")).unwrap();
        fs::create_dir_all(root.join("shaders").join("common")).unwrap();
        fs::write(
            root.join("pipeline.pipe"),
            r#"
pipeline:
  name: Test
passes:
  - name: Forward
    material: lit
    shader_pass: Main
    color_target: swapchain
"#,
        )
        .unwrap();
        fs::write(
            root.join("materials").join("lit.mat"),
            "material:\n  name: Lit\n  shader: LitShader\n",
        )
        .unwrap();
        let shader = r#"shader "LitShader" {
    pass "Main" {
        face "back"
        queue "opaque"
        hlsl {
            #include "common/vertex.hlsl"
        }
    }
}"#;
        fs::write(root.join("shaders").join("lit.shader"), shader).unwrap();
        fs::write(
            root.join("shaders").join("common").join("vertex.hlsl"),
            "struct VertexInput { float3 position : POSITION; };\n",
        )
        .unwrap();
        fs::write(
            root.join("shaders").join("unused.shader"),
            shader.replace("LitShader", "UnusedShader"),
        )
        .unwrap();
    }

    #[test]
    fn recognizes_supported_asset_extensions_case_insensitively() {
        for (extension, expected) in [
            ("fbx", AssetType::Fbx),
            ("MAT", AssetType::Material),
            ("pipe", AssetType::Pipeline),
            ("shader", AssetType::Shader),
            ("png", AssetType::Png),
            ("txt", AssetType::Text),
            ("data", AssetType::Data),
        ] {
            assert_eq!(AssetType::from_extension(extension), Some(expected));
        }
        assert_eq!(AssetType::from_extension("scene"), Some(AssetType::Scene));
        assert_eq!(AssetType::from_extension("prefab"), Some(AssetType::Prefab));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn scan_rejects_missing_required_and_duplicate_unique_components() {
        let _probe = EditorRuleProbe {
            instance: engine::ecs::ComponentInstance::default(),
        };
        let custom_root = temp_dir("custom-component-rules");
        fs::write(
            custom_root.join("invalid.scene"),
            "root:\n  name: Root\n  components:\n    - type: EditorRuleProbe\n    - type: Camera\n",
        )
        .unwrap();
        let error = AssetDatabase::scan(&custom_root)
            .err()
            .expect("custom component metadata must be discovered automatically");
        assert!(format!("{error:#}").contains("requires component 'Mesh'"));
        fs::remove_dir_all(custom_root).unwrap();

        let missing_required_root = temp_dir("missing-required-component");
        fs::write(
            missing_required_root.join("invalid.scene"),
            "root:\n  name: Root\n  components:\n    - type: Animator\n",
        )
        .unwrap();
        let error = AssetDatabase::scan(&missing_required_root)
            .err()
            .expect("Animator without SkinnedMesh must fail validation");
        assert!(format!("{error:#}").contains("requires component 'SkinnedMesh'"));
        fs::remove_dir_all(missing_required_root).unwrap();

        let duplicate_unique_root = temp_dir("duplicate-unique-component");
        fs::write(
            duplicate_unique_root.join("invalid.scene"),
            "root:\n  name: Root\n  components:\n    - type: Camera\n    - type: Camera\n",
        )
        .unwrap();
        let error = AssetDatabase::scan(&duplicate_unique_root)
            .err()
            .expect("duplicate Camera components must fail validation");
        assert!(format!("{error:#}").contains("more than one unique component 'Camera'"));
        fs::remove_dir_all(duplicate_unique_root).unwrap();
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_scan_skips_component_rule_validation() {
        let root = temp_dir("release-component-validation");
        fs::write(
            root.join("invalid.scene"),
            "root:\n  name: Root\n  components:\n    - type: Animator\n",
        )
        .unwrap();
        assert!(AssetDatabase::scan(&root).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scans_nested_assets_and_resolves_by_name_and_type() {
        let root = temp_dir("scan");
        write_sample_project(&root);
        let database = AssetDatabase::scan(&root).unwrap();
        let shader = database
            .resolve(&AssetRef {
                name: "LitShader".to_owned(),
                asset_type: AssetType::Shader,
            })
            .unwrap();
        assert!(shader.ends_with("lit.shader"));
        assert!(
            database
                .resolve(&AssetRef {
                    name: "lit".to_owned(),
                    asset_type: AssetType::Shader,
                })
                .is_err()
        );
        assert_eq!(database.assets().count(), 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_duplicate_names_within_an_asset_type() {
        let root = temp_dir("duplicate");
        fs::create_dir_all(root.join("first")).unwrap();
        fs::create_dir_all(root.join("second")).unwrap();
        let shader = r#"shader "Shared" { pass "Main" { face "back" queue "opaque" hlsl { struct VertexInput { float3 p : POSITION; }; } } }"#;
        fs::write(root.join("first").join("shared.shader"), shader).unwrap();
        fs::write(root.join("second").join("shared.shader"), shader).unwrap();
        assert!(
            AssetDatabase::scan(&root)
                .err()
                .unwrap()
                .to_string()
                .contains("duplicate Shader")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn development_pipeline_resolves_material_and_shader_without_moving_assets() {
        let root = temp_dir("development");
        write_sample_project(&root);
        let database = AssetDatabase::scan(&root).unwrap();
        let prepared = database
            .prepare_development_pipeline(&root.join("pipeline.pipe"))
            .unwrap();
        let pipeline = read_yaml(&prepared).unwrap();
        let material_path = PathBuf::from(
            pipeline["passes"][0]["material"]
                .as_str()
                .expect("resolved material path"),
        );
        assert!(material_path.is_absolute());
        let material = read_yaml(&material_path).unwrap();
        let shader_path = PathBuf::from(
            material["material"]["shader"]
                .as_str()
                .expect("resolved shader path"),
        );
        assert!(shader_path.is_absolute());
        assert!(shader_path.is_file());
        assert_eq!(
            read_yaml(&root.join("materials").join("lit.mat")).unwrap()["material"]["shader"]
                .as_str(),
            Some("LitShader")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn release_package_rewrites_dependencies_and_runs_processors() {
        let root = temp_dir("package-assets");
        let output = temp_dir("package-output");
        write_sample_project(&root);
        let mut database = AssetDatabase::scan(&root).unwrap();
        database.register_processor(AssetType::Pipeline, |source, destination| {
            let mut text = fs::read_to_string(source)?;
            text.push_str("\n# pipeline processed\n");
            fs::write(destination, text)?;
            Ok(())
        });
        database.register_processor(AssetType::Material, |source, destination| {
            let mut text = fs::read_to_string(source)?;
            text.push_str("\n# material processed\n");
            fs::write(destination, text)?;
            Ok(())
        });
        database.register_processor(AssetType::Shader, |source, destination| {
            let mut text = String::from("// shader processed\n");
            text.push_str(&fs::read_to_string(source)?);
            fs::write(destination, text)?;
            Ok(())
        });

        let packaged = database
            .package(&root.join("pipeline.pipe"), &output)
            .unwrap();
        let pipeline = read_yaml(&packaged).unwrap();
        assert_eq!(
            pipeline["passes"][0]["material"].as_str(),
            Some("materials/lit.mat")
        );
        assert!(
            fs::read_to_string(&packaged)
                .unwrap()
                .contains("# pipeline processed")
        );
        let material_path = output.join("materials").join("lit.mat");
        let material = read_yaml(&material_path).unwrap();
        assert_eq!(
            material["material"]["shader"].as_str(),
            Some("../shaders/LitShader.shader")
        );
        assert!(
            fs::read_to_string(&material_path)
                .unwrap()
                .contains("# material processed")
        );
        let packaged_shader =
            fs::read_to_string(output.join("shaders").join("LitShader.shader")).unwrap();
        assert!(packaged_shader.starts_with("// shader processed"));
        assert!(packaged_shader.contains("struct VertexInput"));
        assert!(!packaged_shader.contains("#include"));
        assert!(!output.join("shaders").join("UnusedShader.shader").exists());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(output).unwrap();
    }
}
