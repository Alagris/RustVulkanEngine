use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub pipeline: PathBuf,
    pub default_scene: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_assets: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_input_map: Option<String>,
}

impl RuntimeSettings {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let mut settings: Self = serde_yaml::from_str(&text)
            .with_context(|| format!("Invalid runtime settings {}", path.display()))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        for asset_path in [&mut settings.pipeline, &mut settings.default_scene] {
            if asset_path.as_os_str().is_empty() {
                bail!("runtime settings paths must not be empty");
            }
            if !asset_path.is_absolute() {
                *asset_path = base.join(&*asset_path);
            }
        }
        if let Some(data_assets) = &mut settings.data_assets {
            if data_assets.as_os_str().is_empty() {
                bail!("runtime data asset path must not be empty");
            }
            if !data_assets.is_absolute() {
                *data_assets = base.join(&*data_assets);
            }
        }
        if settings
            .input_asset
            .as_ref()
            .is_some_and(|name| name.trim().is_empty())
        {
            bail!("runtime input asset name must not be empty");
        }
        if settings
            .active_input_map
            .as_ref()
            .is_some_and(|name| name.trim().is_empty())
        {
            bail!("runtime active input map name must not be empty");
        }
        // Postcondition: every configured path has been anchored to the settings file.
        assert!(
            settings.pipeline.is_absolute() || !path.is_absolute(),
            "resolved pipeline path must be absolute when the settings path is absolute"
        );
        assert!(
            settings.default_scene.is_absolute() || !path.is_absolute(),
            "resolved scene path must be absolute when the settings path is absolute"
        );
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_settings(name: &str, contents: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "engine-settings-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("runtime_settings.yaml");
        fs::write(&path, contents).unwrap();
        path
    }

    fn cleanup(path: &Path) {
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn resolves_relative_paths_against_the_settings_directory() {
        let path = temp_settings(
            "relative",
            "pipeline: render/pipeline.pipe\ndefault_scene: scenes/main.scene\ndata_assets: data\n",
        );
        let settings = RuntimeSettings::load(&path).unwrap();
        let base = path.parent().unwrap();
        assert_eq!(settings.pipeline, base.join("render/pipeline.pipe"));
        assert_eq!(settings.default_scene, base.join("scenes/main.scene"));
        assert_eq!(settings.data_assets, Some(base.join("data")));
        assert_eq!(settings.input_asset, None);
        assert_eq!(settings.active_input_map, None);
        cleanup(&path);
    }

    #[test]
    fn preserves_absolute_paths() {
        let absolute = std::env::temp_dir().join("absolute.pipe");
        let path = temp_settings(
            "absolute",
            &format!(
                "pipeline: {:?}\ndefault_scene: scene.scene\n",
                absolute.display().to_string()
            ),
        );
        let settings = RuntimeSettings::load(&path).unwrap();
        assert_eq!(settings.pipeline, absolute);
        cleanup(&path);
    }

    #[test]
    fn rejects_empty_paths_and_blank_names() {
        for (name, contents, message) in [
            (
                "empty-pipeline",
                "pipeline: \"\"\ndefault_scene: scene.scene\n",
                "runtime settings paths must not be empty",
            ),
            (
                "empty-data",
                "pipeline: p.pipe\ndefault_scene: s.scene\ndata_assets: \"\"\n",
                "runtime data asset path must not be empty",
            ),
            (
                "blank-input",
                "pipeline: p.pipe\ndefault_scene: s.scene\ninput_asset: \"  \"\n",
                "runtime input asset name must not be empty",
            ),
            (
                "blank-map",
                "pipeline: p.pipe\ndefault_scene: s.scene\nactive_input_map: \" \"\n",
                "runtime active input map name must not be empty",
            ),
        ] {
            let path = temp_settings(name, contents);
            let error = RuntimeSettings::load(&path).unwrap_err();
            assert!(
                error.to_string().contains(message),
                "expected '{message}' in '{error}'"
            );
            cleanup(&path);
        }
    }

    #[test]
    fn reports_missing_files_and_invalid_yaml_with_path_context() {
        let missing = std::env::temp_dir().join("engine-settings-does-not-exist.yaml");
        let error = RuntimeSettings::load(&missing).unwrap_err();
        assert!(error.to_string().contains("Failed to read"));

        let path = temp_settings("invalid-yaml", "pipeline: [not, a, path\n");
        let error = RuntimeSettings::load(&path).unwrap_err();
        assert!(error.to_string().contains("Invalid runtime settings"));
        cleanup(&path);
    }
}
