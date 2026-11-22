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
        Ok(settings)
    }
}
