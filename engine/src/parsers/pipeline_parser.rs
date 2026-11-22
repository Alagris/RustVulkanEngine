use crate::parsers::hlsl_preprocessor;
use crate::parsers::material_parser;
use crate::postprocess::{PostProcessPass, PostProcessRegistry};
use crate::render_pipeline::{
    BufferSettings, ColorFormat, DepthFormat, PresentMode, RenderBufferSettings, RenderPass,
    RenderPipeline, ShaderPassMetadata, SwapchainSettings,
};
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct PipelineFile {
    pipeline: PipelineSettings,
    #[serde(default)]
    swapchain: SwapchainFile,
    #[serde(default)]
    render_buffers: RenderBuffersFile,
    #[serde(default)]
    postprocessing: Vec<serde_yaml::Value>,
    passes: Vec<PassSettings>,
}

#[derive(Deserialize)]
#[serde(default)]
struct SwapchainFile {
    image_count: u32,
    present_mode: String,
    color_format: String,
}

impl Default for SwapchainFile {
    fn default() -> Self {
        Self {
            image_count: 3,
            present_mode: "fifo".to_owned(),
            color_format: "bgra8_srgb".to_owned(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct RenderBuffersFile {
    depth: DepthBufferFile,
    shadow: ShadowBufferFile,
}

#[derive(Deserialize)]
#[serde(default)]
struct DepthBufferFile {
    width: u32,
    height: u32,
    format: String,
}

impl Default for DepthBufferFile {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            format: "d32_float".to_owned(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct ShadowBufferFile {
    width: u32,
    height: u32,
    format: String,
    shadow_cascades: u32,
}

impl Default for ShadowBufferFile {
    fn default() -> Self {
        Self {
            width: 2048,
            height: 2048,
            format: "d32_float".to_owned(),
            shadow_cascades: 4,
        }
    }
}

impl Default for RenderBuffersFile {
    fn default() -> Self {
        Self {
            depth: DepthBufferFile::default(),
            shadow: ShadowBufferFile::default(),
        }
    }
}

#[derive(Deserialize)]
struct PipelineSettings {
    name: String,
}

#[derive(Deserialize)]
struct PassSettings {
    name: String,
    material: PathBuf,
    shader_pass: String,
    color_target: String,
    clear_color: Option<[f32; 4]>,
}

impl RenderPipeline {
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with_registry(path, &PostProcessRegistry::default())
    }

    pub fn load_with_registry(path: &Path, registry: &PostProcessRegistry) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("Failed to read render pipeline {}", path.display()))?;
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        Self::from_yaml_with_registry(&source, base_dir, registry)
            .with_context(|| format!("Invalid render pipeline {}", path.display()))
    }

    #[cfg(test)]
    pub(crate) fn from_yaml(source: &str, base_dir: &Path) -> Result<Self> {
        Self::from_yaml_with_registry(source, base_dir, &PostProcessRegistry::default())
    }

    pub(crate) fn from_yaml_with_registry(
        source: &str,
        base_dir: &Path,
        postprocess_registry: &PostProcessRegistry,
    ) -> Result<Self> {
        let config: PipelineFile =
            serde_yaml::from_str(source).context("Failed to parse pipeline YAML")?;
        postprocess_registry.registered_types()?;
        if config.pipeline.name.trim().is_empty() {
            bail!("pipeline.name must not be empty");
        }
        if config.passes.is_empty() {
            bail!("pipeline must contain at least one [[passes]] entry");
        }
        if config.swapchain.image_count == 0 {
            bail!("swapchain.image_count must be greater than zero");
        }
        if config.render_buffers.shadow.shadow_cascades == 0
            || config.render_buffers.shadow.shadow_cascades > 4
        {
            bail!("render_buffers.shadow.shadow_cascades must be between 1 and 4");
        }
        if config.render_buffers.shadow.width == 0 || config.render_buffers.shadow.height == 0 {
            bail!("shadow render-buffer dimensions must be greater than zero");
        }
        if (config.render_buffers.depth.width == 0) != (config.render_buffers.depth.height == 0) {
            bail!("render_buffers.depth width and height must both be zero or both be positive");
        }
        let present_mode = match config.swapchain.present_mode.as_str() {
            "fifo" => PresentMode::Fifo,
            "mailbox" => PresentMode::Mailbox,
            mode => bail!("unsupported swapchain present_mode '{mode}'"),
        };
        let color_format = match config.swapchain.color_format.as_str() {
            "bgra8_srgb" => ColorFormat::Bgra8Srgb,
            "rgba8_srgb" => ColorFormat::Rgba8Srgb,
            "bgra8_unorm" => ColorFormat::Bgra8Unorm,
            format => bail!("unsupported swapchain color_format '{format}'"),
        };
        let parse_depth_format = |format: &str| match format {
            "d32_float" => Ok(DepthFormat::D32Float),
            "d32_float_s8" => Ok(DepthFormat::D32FloatS8),
            "d24_unorm_s8" => Ok(DepthFormat::D24UnormS8),
            _ => bail!("unsupported depth format '{format}'"),
        };
        let swapchain = SwapchainSettings {
            image_count: config.swapchain.image_count,
            present_mode,
            color_format,
        };
        let render_buffers = RenderBufferSettings {
            depth: BufferSettings {
                width: config.render_buffers.depth.width,
                height: config.render_buffers.depth.height,
                format: parse_depth_format(&config.render_buffers.depth.format)?,
            },
            shadow: BufferSettings {
                width: config.render_buffers.shadow.width,
                height: config.render_buffers.shadow.height,
                format: parse_depth_format(&config.render_buffers.shadow.format)?,
            },
            shadow_cascades: config.render_buffers.shadow.shadow_cascades,
        };
        let mut post_processing: Vec<std::sync::Arc<dyn PostProcessPass>> = Vec::new();
        for (index, definition) in config.postprocessing.into_iter().enumerate() {
            post_processing.extend(
                postprocess_registry
                    .create_stages(definition)
                    .with_context(|| format!("Invalid postprocessing entry {index}"))?
                    .into_iter()
                    .map(std::sync::Arc::from),
            );
        }

        let mut names = HashSet::new();
        let mut passes = Vec::with_capacity(config.passes.len());
        for settings in config.passes {
            if settings.name.trim().is_empty() || !names.insert(settings.name.clone()) {
                bail!(
                    "render pass names must be non-empty and unique: '{}'",
                    settings.name
                );
            }
            if settings.shader_pass.trim().is_empty() || settings.color_target.trim().is_empty() {
                bail!(
                    "render pass '{}' has an empty shader pass or target",
                    settings.name
                );
            }
            if let Some(color) = settings.clear_color {
                if color
                    .iter()
                    .any(|component| !component.is_finite() || !(0.0..=1.0).contains(component))
                {
                    bail!(
                        "render pass '{}' clear_color values must be finite and between 0 and 1",
                        settings.name
                    );
                }
            }

            let material_path = if settings.material.is_absolute() {
                settings.material
            } else {
                base_dir.join(settings.material)
            };
            let material_file = material_parser::parse(&material_path)?;
            let shader_path = if material_file.shader.is_absolute() {
                material_file.shader
            } else {
                material_path
                    .parent()
                    .unwrap_or(base_dir)
                    .join(material_file.shader)
            };
            let shader_source = fs::read_to_string(&shader_path)
                .with_context(|| format!("Failed to read shader {}", shader_path.display()))?;
            let shader_source = hlsl_preprocessor::expand_source(&shader_source, &shader_path)
                .with_context(|| {
                    format!(
                        "Failed to expand HLSL directives in {}",
                        shader_path.display()
                    )
                })?;
            let shader = material_parser::parse_shader_definition(&shader_source)
                .with_context(|| format!("Invalid shader {}", shader_path.display()))?;
            let metadata: ShaderPassMetadata = shader
                .passes
                .into_iter()
                .find(|shader_pass| shader_pass.name == settings.shader_pass)
                .ok_or_else(|| {
                    anyhow!(
                        "render pass '{}' selects shader pass '{}', but it is not declared in {}",
                        settings.name,
                        settings.shader_pass,
                        shader_path.display()
                    )
                })?;

            passes.push(RenderPass {
                name: settings.name,
                shader: shader_path,
                shader_name: shader.name,
                material: material_path,
                material_name: material_file.name,
                color_target: settings.color_target,
                clear_color: settings.clear_color,
                metadata,
            });
        }

        Ok(Self {
            name: config.pipeline.name,
            passes,
            post_processing,
            swapchain,
            render_buffers,
            output_dir: base_dir.join("compiled_shaders"),
        })
    }
}
