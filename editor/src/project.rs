use crate::asset_database::{AssetDatabase, AssetRef, AssetType, file_nonce};
use anyhow::{Context, Result, anyhow, bail};
pub use engine::settings::RuntimeSettings;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    #[serde(flatten)]
    pub runtime: RuntimeSettings,
    pub editor_scene: PathBuf,
}

pub struct Project {
    root: PathBuf,
    assets: AssetDatabase,
    settings: Settings,
}

impl Project {
    pub fn load(settings_path: impl AsRef<Path>) -> Result<Self> {
        let settings_path = settings_path.as_ref().canonicalize().with_context(|| {
            format!(
                "Failed to resolve settings file {}",
                settings_path.as_ref().display()
            )
        })?;
        let root = settings_path
            .parent()
            .ok_or_else(|| anyhow!("settings file has no parent directory"))?
            .to_path_buf();
        let settings_source = fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read settings {}", settings_path.display()))?;
        let mut settings: Settings = toml::from_str(&settings_source)
            .with_context(|| format!("Invalid settings {}", settings_path.display()))?;
        for path in [
            &mut settings.runtime.pipeline,
            &mut settings.runtime.default_scene,
            &mut settings.editor_scene,
        ] {
            if path.as_os_str().is_empty() {
                bail!("settings paths must not be empty");
            }
            if !path.is_absolute() {
                *path = root.join(&*path);
            }
        }
        let assets = AssetDatabase::scan(root.join("assets"))?;
        if !settings.runtime.pipeline.is_file() {
            bail!(
                "configured pipeline does not exist: {}",
                settings.runtime.pipeline.display()
            );
        }
        if !settings.runtime.default_scene.is_file() {
            bail!(
                "configured default scene does not exist: {}",
                settings.runtime.default_scene.display()
            );
        }
        if !settings.editor_scene.is_file() {
            bail!(
                "configured editor scene does not exist: {}",
                settings.editor_scene.display()
            );
        }
        Ok(Self {
            root,
            assets,
            settings,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn assets(&self) -> &AssetDatabase {
        &self.assets
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn runtime_settings(&self) -> RuntimeSettings {
        self.settings.runtime.clone()
    }

    pub fn prepare_development(&self) -> Result<RuntimeSettings> {
        let pipeline = self
            .assets
            .prepare_development_pipeline(&self.settings.runtime.pipeline)?;
        Ok(RuntimeSettings {
            pipeline,
            default_scene: self.settings.runtime.default_scene.clone(),
        })
    }

    pub fn package(&self, output: &Path) -> Result<PathBuf> {
        fs::create_dir_all(output)
            .with_context(|| format!("Failed to create release directory {}", output.display()))?;
        let packaged_pipeline = self
            .assets
            .package(&self.settings.runtime.pipeline, output)?;
        let scene_relative = PathBuf::from("scenes").join(
            self.settings
                .runtime
                .default_scene
                .file_name()
                .ok_or_else(|| anyhow!("default scene path has no filename"))?,
        );
        let scene_output = output.join(&scene_relative);
        fs::create_dir_all(
            scene_output
                .parent()
                .ok_or_else(|| anyhow!("packaged scene path has no parent"))?,
        )?;
        self.assets
            .process(
                AssetType::Scene,
                &self.settings.runtime.default_scene,
                &scene_output,
            )
            .with_context(|| {
                format!(
                    "Failed to package scene {}",
                    self.settings.runtime.default_scene.display()
                )
            })?;
        let runtime_settings = RuntimeSettings {
            pipeline: packaged_pipeline
                .file_name()
                .map(PathBuf::from)
                .ok_or_else(|| anyhow!("packaged pipeline path has no filename"))?,
            default_scene: scene_relative,
        };
        let runtime_settings_path = output.join("runtime_settings.toml");
        fs::write(&runtime_settings_path, toml::to_string(&runtime_settings)?)
            .with_context(|| format!("Failed to write {}", runtime_settings_path.display()))?;
        Ok(runtime_settings_path)
    }

    pub fn resolve_asset(&self, name: &str, asset_type: AssetType) -> Result<&Path> {
        self.assets.resolve(&AssetRef {
            name: name.to_owned(),
            asset_type,
        })
    }

    pub fn development_settings_path(&self) -> Result<PathBuf> {
        let output = std::env::temp_dir()
            .join("rust-vulkan-game-engine")
            .join("development")
            .join(format!("settings-{}-{}", std::process::id(), file_nonce()));
        fs::create_dir_all(&output)?;
        let settings = self.prepare_development()?;
        let settings_path = output.join("runtime_settings.toml");
        fs::write(&settings_path, toml::to_string(&settings)?)
            .with_context(|| format!("Failed to write {}", settings_path.display()))?;
        Ok(settings_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "editor-project-{}-{}",
            std::process::id(),
            file_nonce()
        ))
    }

    #[test]
    fn deserializes_flattened_runtime_fields_and_serializes_runtime_subset() {
        let source = r#"
pipeline = "assets/pipeline.pipe"
default_scene = "assets/scenes/game.scene"
editor_scene = "assets/scenes/editor.scene"
"#;
        let settings: Settings = toml::from_str(source).unwrap();
        assert_eq!(
            settings.runtime.pipeline,
            PathBuf::from("assets/pipeline.pipe")
        );
        assert_eq!(
            settings.runtime.default_scene,
            PathBuf::from("assets/scenes/game.scene")
        );
        let runtime_toml = toml::to_string(&settings.runtime).unwrap();
        assert!(!runtime_toml.contains("editor_scene"));
        assert!(runtime_toml.contains("default_scene"));
    }

    #[test]
    fn runtime_settings_resolve_paths_relative_to_the_settings_file() {
        let directory = temp_dir();
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("runtime_settings.toml");
        fs::write(
            &path,
            "pipeline = \"pipeline.pipe\"\ndefault_scene = \"scene.scene\"\n",
        )
        .unwrap();
        let settings = RuntimeSettings::load(&path).unwrap();
        assert_eq!(settings.pipeline, directory.join("pipeline.pipe"));
        assert_eq!(settings.default_scene, directory.join("scene.scene"));
        fs::remove_dir_all(directory).unwrap();
    }
}
