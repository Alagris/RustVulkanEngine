use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub pipeline: PathBuf,
    pub default_scene: PathBuf,
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
        Ok(settings)
    }
}
