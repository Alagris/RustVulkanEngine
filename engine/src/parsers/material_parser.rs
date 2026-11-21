use crate::render_pipeline::{CullMode, LexError, RenderQueue};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialAsset {
    pub name: String,
    pub shader: PathBuf,
}

#[derive(Deserialize)]
struct MaterialFile {
    material: MaterialSettings,
}

#[derive(Deserialize)]
struct MaterialSettings {
    name: String,
    shader: PathBuf,
}

pub fn parse(path: &Path) -> Result<MaterialAsset> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("Failed to read material {}", path.display()))?;
    let file: MaterialFile = serde_yaml::from_str(&source)
        .with_context(|| format!("Invalid material {}", path.display()))?;
    if file.material.name.trim().is_empty() {
        bail!("material name must not be empty in {}", path.display());
    }

    if file.material.shader.as_os_str().is_empty() {
        bail!(
            "material shader path must not be empty in {}",
            path.display()
        );
    }
    Ok(MaterialAsset {
        name: file.material.name,
        shader: file.material.shader,
    })
}

pub(crate) fn parse_cull_mode(value: String) -> std::result::Result<CullMode, LexError> {
    match unquote_material_string(value).as_str() {
        "none" => Ok(CullMode::None),
        "front" => Ok(CullMode::Front),
        "back" => Ok(CullMode::Back),
        other => Err(LexError {
            offset: 0,
            message: format!("unsupported face culling mode '{other}'"),
        }),
    }
}

pub(crate) fn parse_render_queue(value: String) -> std::result::Result<RenderQueue, LexError> {
    match unquote_material_string(value).as_str() {
        "opaque" => Ok(RenderQueue::Opaque),
        "transparent" => Ok(RenderQueue::Transparent),
        other => Err(LexError {
            offset: 0,
            message: format!("unsupported render queue '{other}'"),
        }),
    }
}

fn unquote_material_string(value: String) -> String {
    assert!(
        value.len() >= 2,
        "quoted shader metadata must include delimiters"
    );
    let mut chars = value[1..value.len() - 1].chars();
    let mut decoded = String::new();
    while let Some(character) = chars.next() {
        if character == '\\' {
            match chars.next() {
                Some('"') => decoded.push('"'),
                Some('\\') => decoded.push('\\'),
                Some('n') => decoded.push('\n'),
                Some('r') => decoded.push('\r'),
                Some('t') => decoded.push('\t'),
                Some(other) => decoded.push(other),
                None => break,
            }
        } else {
            decoded.push(character);
        }
    }
    decoded
}
