use crate::postprocess::PostProcessPass;
use anyhow::{Context, Result, anyhow};
use shaderc::{CompileOptions, Compiler, EnvVersion, ShaderKind, SourceLanguage, TargetEnv};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct RenderPipeline {
    pub name: String,
    pub passes: Vec<RenderPass>,
    pub post_processing: Vec<Arc<dyn PostProcessPass>>,
    pub swapchain: SwapchainSettings,
    pub render_buffers: RenderBufferSettings,
    pub(crate) output_dir: PathBuf,
}

impl std::fmt::Debug for RenderPipeline {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RenderPipeline")
            .field("name", &self.name)
            .field("passes", &self.passes)
            .field(
                "post_processing",
                &self
                    .post_processing
                    .iter()
                    .map(|pass| pass.name())
                    .collect::<Vec<_>>(),
            )
            .field("swapchain", &self.swapchain)
            .field("render_buffers", &self.render_buffers)
            .field("output_dir", &self.output_dir)
            .finish()
    }
}
#[derive(Clone, Debug)]
pub struct RenderPass {
    pub name: String,
    pub shader: PathBuf,
    pub shader_name: String,
    pub material: PathBuf,
    pub material_name: String,
    pub color_target: String,
    pub clear_color: Option<[f32; 4]>,
    pub metadata: ShaderPassMetadata,
}

#[derive(Clone, Debug)]
pub struct SwapchainSettings {
    pub image_count: u32,
    pub present_mode: PresentMode,
    pub color_format: ColorFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentMode {
    Fifo,
    Mailbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorFormat {
    Bgra8Srgb,
    Rgba8Srgb,
    Bgra8Unorm,
}

#[derive(Clone, Debug)]
pub struct RenderBufferSettings {
    pub depth: BufferSettings,
    pub shadow: BufferSettings,
    pub shadow_cascades: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthFormat {
    D32Float,
    D32FloatS8,
    D24UnormS8,
}

#[derive(Clone, Copy, Debug)]
pub struct BufferSettings {
    pub width: u32,
    pub height: u32,
    pub format: DepthFormat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CullMode {
    None,
    Front,
    Back,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderQueue {
    Opaque,
    Transparent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderPassMetadata {
    pub name: String,
    pub queue: RenderQueue,
    pub cull: CullMode,
    pub hlsl: String,
    pub vertex_inputs: ShaderInputLayout,
    pub instance_inputs: ShaderInputLayout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderInputFormat {
    Float32,
    Float32x2,
    Float32x3,
    Float32x4,
    Sint32,
    Sint32x2,
    Sint32x3,
    Sint32x4,
    Uint32,
    Uint32x2,
    Uint32x3,
    Uint32x4,
}

impl ShaderInputFormat {
    pub fn byte_size(self) -> u32 {
        match self {
            Self::Float32 | Self::Sint32 | Self::Uint32 => 4,
            Self::Float32x2 | Self::Sint32x2 | Self::Uint32x2 => 8,
            Self::Float32x3 | Self::Sint32x3 | Self::Uint32x3 => 12,
            Self::Float32x4 | Self::Sint32x4 | Self::Uint32x4 => 16,
        }
    }

    pub(crate) fn alignment(self) -> u32 {
        match self {
            Self::Float32 | Self::Sint32 | Self::Uint32 => 4,
            Self::Float32x2 | Self::Sint32x2 | Self::Uint32x2 => 8,
            Self::Float32x3
            | Self::Float32x4
            | Self::Sint32x3
            | Self::Sint32x4
            | Self::Uint32x3
            | Self::Uint32x4 => 16,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderInputAttribute {
    pub location: u32,
    pub format: ShaderInputFormat,
    pub offset: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShaderInputLayout {
    pub stride: u32,
    pub attributes: Vec<ShaderInputAttribute>,
}

#[derive(Debug)]
pub(crate) struct ShaderDefinition {
    pub(crate) name: String,
    pub(crate) passes: Vec<ShaderPassMetadata>,
}

pub fn shader_asset_name(source: &str) -> Result<String> {
    crate::parsers::material_parser::shader_asset_name(source)
}

pub fn shader_asset_name_from_path(path: &std::path::Path) -> Result<String> {
    preprocess_shader_file(path).and_then(|source| shader_asset_name(&source))
}

pub fn preprocess_shader_file(path: &std::path::Path) -> Result<String> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("Failed to read shader {}", path.display()))?;
    crate::parsers::hlsl_preprocessor::expand_source(&source, path)
        .with_context(|| format!("Failed to expand HLSL directives in {}", path.display()))
}

impl RenderPipeline {
    pub fn compile_shaders(&self) -> Result<()> {
        assert!(
            !self.passes.is_empty(),
            "validated pipeline must contain render passes"
        );
        fs::create_dir_all(&self.output_dir).with_context(|| {
            format!(
                "Failed to create shader output directory {}",
                self.output_dir.display()
            )
        })?;
        let compiler = Compiler::new().context("Failed to initialize embedded shaderc compiler")?;
        for (index, pass) in self.passes.iter().enumerate() {
            assert!(
                !pass.metadata.hlsl.trim().is_empty(),
                "parsed HLSL pass must not be empty"
            );
            let source_path = pass.shader.display().to_string();
            for (stage, shader_kind, entry) in [
                ("vert", ShaderKind::Vertex, "VSMain"),
                ("frag", ShaderKind::Fragment, "PSMain"),
            ] {
                let output = self.output_dir.join(format!("pass_{index}.{stage}.spv"));
                let mut options = CompileOptions::new()
                    .context("Failed to initialize embedded shaderc compile options")?;
                options.set_source_language(SourceLanguage::HLSL);
                options.set_target_env(TargetEnv::Vulkan, EnvVersion::Vulkan1_0 as u32);
                options.set_hlsl_io_mapping(true);
                options.set_auto_map_locations(true);
                options.set_auto_bind_uniforms(true);
                let artifact = compiler
                    .compile_into_spirv(
                        &pass.metadata.hlsl,
                        shader_kind,
                        &source_path,
                        entry,
                        Some(&options),
                    )
                    .map_err(|error| {
                        anyhow!(
                            "Embedded HLSL compilation failed for pass '{}' ({stage}): {error}",
                            pass.name
                        )
                    })?;
                assert!(
                    !artifact.as_binary().is_empty(),
                    "shaderc emitted an empty SPIR-V module"
                );
                assert_eq!(
                    artifact.as_binary().first(),
                    Some(&0x0723_0203),
                    "shaderc output must have the SPIR-V magic number"
                );
                fs::write(&output, artifact.as_binary_u8()).with_context(|| {
                    format!("Failed to write SPIR-V module {}", output.display())
                })?;
                log::info!(
                    "Compiled {} shader for pass '{}' with embedded shaderc to {}.",
                    stage,
                    pass.name,
                    output.display()
                );
            }
        }
        for (index, pass) in self.post_processing.iter().enumerate() {
            for (stage, shader_kind, entry, source) in [
                (
                    "vert",
                    ShaderKind::Vertex,
                    "VSMain",
                    pass.vertex_shader_source(),
                ),
                (
                    "frag",
                    ShaderKind::Fragment,
                    "PSMain",
                    pass.fragment_shader_source(),
                ),
            ] {
                let output = self.output_dir.join(format!("post_{index}.{stage}.spv"));
                let mut options = CompileOptions::new()
                    .context("Failed to initialize embedded shaderc compile options")?;
                options.set_source_language(SourceLanguage::HLSL);
                options.set_target_env(TargetEnv::Vulkan, EnvVersion::Vulkan1_0 as u32);
                options.set_hlsl_io_mapping(true);
                options.set_auto_map_locations(true);
                options.set_auto_bind_uniforms(true);
                let artifact = compiler
                    .compile_into_spirv(
                        &source,
                        shader_kind,
                        "postprocess.hlsl",
                        entry,
                        Some(&options),
                    )
                    .map_err(|error| {
                        anyhow!(
                            "Embedded HLSL compilation failed for postprocess pass '{}' ({stage}): {error}",
                            pass.name()
                        )
                    })?;
                assert!(
                    !artifact.as_binary().is_empty(),
                    "shaderc emitted an empty postprocess SPIR-V module"
                );
                assert_eq!(
                    artifact.as_binary().first(),
                    Some(&0x0723_0203),
                    "shaderc output must have the SPIR-V magic number"
                );
                fs::write(&output, artifact.as_binary_u8()).with_context(|| {
                    format!("Failed to write SPIR-V module {}", output.display())
                })?;
                log::info!(
                    "Compiled {} shader for postprocess pass '{}' with embedded shaderc to {}.",
                    stage,
                    pass.name(),
                    output.display()
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::material_parser::parse_shader_definition;
    use crate::postprocess::PostProcessRegistry;

    #[crate::postprocess::post_process("custom_tint")]
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct CustomPostProcess {
        intensity: f32,
    }

    impl PostProcessPass for CustomPostProcess {
        fn name(&self) -> &str {
            "custom_tint"
        }

        fn fragment_shader_source(&self) -> String {
            crate::postprocess::fullscreen_fragment_shader(&format!(
                "return Source.Sample(SourceSampler, input.uv) * {:.8};",
                self.intensity
            ))
        }
    }

    fn temporary_directory(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("engine-{name}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn parses_nested_hlsl_braces_and_braces_in_strings_and_comments() {
        let source = r#"
shader "Test" {
    pass "Nested" {
        face "back"
        queue "opaque"
        hlsl {
            struct VertexInput { float3 position : POSITION; };
            float4 VSMain() {
                const char* text = "}";
                // }
                return float4(1, 0, 0, 1);
            }
            float4 PSMain() { return float4(1, 1, 1, 1); }
        }
    }
}
"#;
        let shader = parse_shader_definition(source).unwrap();
        assert_eq!(shader.name, "Test");
        assert!(shader.passes[0].hlsl.contains("const char* text = \"}\""));
        assert!(shader.passes[0].hlsl.contains("return float4(1, 0, 0, 1)"));
    }

    #[test]
    fn rejects_duplicate_pass_properties() {
        let source = r#"
shader "Test" {
    pass "Forward" {
        face "back"
        face "front"
        queue "opaque"
        hlsl { struct VertexInput { float3 position : POSITION; }; }
    }
}
"#;
        assert!(parse_shader_definition(source).is_err());
    }

    #[test]
    fn rejects_missing_pass_in_pipeline() {
        let base_dir = temporary_directory("missing-pass");
        fs::write(
            base_dir.join("cube.mat"),
            "material:\n  name: Cube\n  shader: shader.shader\n",
        )
        .unwrap();
        fs::write(
            base_dir.join("shader.shader"),
            r#"
shader "Test" {
    pass "Forward" {
        face "back"
        queue "opaque"
        hlsl { struct VertexInput { float3 position : POSITION; }; }
    }
}
"#,
        )
        .unwrap();
        let source = r#"
pipeline:
  name: Invalid
passes:
  - name: Missing
    material: cube.mat
    shader_pass: NotDeclared
    color_target: swapchain
"#;
        assert!(
            RenderPipeline::from_yaml(source, &base_dir)
                .unwrap_err()
                .to_string()
                .contains("not declared")
        );
        fs::remove_dir_all(base_dir).unwrap();
    }

    #[test]
    fn pipeline_loader_expands_hlsl_includes_before_input_layout_analysis() {
        let base_dir = temporary_directory("hlsl-includes");
        fs::create_dir_all(base_dir.join("shaders").join("common")).unwrap();
        fs::write(
            base_dir.join("material.mat"),
            "material:\n  name: IncludedMaterial\n  shader: shaders/included.shader\n",
        )
        .unwrap();
        fs::write(
            base_dir.join("shaders").join("common").join("vertex.hlsl"),
            "#if defined(USE_POSITION)\nstruct VertexInput { float3 position : POSITION; };\n#else\nstruct VertexInput { float2 position : TEXCOORD0; };\n#endif\n",
        )
        .unwrap();
        fs::write(
            base_dir.join("shaders").join("included.shader"),
            r#"
shader "IncludedShader" {
    pass "Forward" {
        face "back"
        queue "opaque"
        hlsl {
            #define USE_POSITION 1
            #include "common/vertex.hlsl"
            float4 VSMain(VertexInput input) : SV_Position { return float4(input.position, 1.0); }
            float4 PSMain() : SV_Target0 { return float4(1.0, 1.0, 1.0, 1.0); }
        }
    }
}
"#,
        )
        .unwrap();
        let source = r#"
pipeline:
  name: Included
passes:
  - name: Forward
    material: material.mat
    shader_pass: Forward
    color_target: swapchain
"#;

        let pipeline = RenderPipeline::from_yaml(source, &base_dir).unwrap();
        let pass = &pipeline.passes[0];
        assert_eq!(pass.shader_name, "IncludedShader");
        assert_eq!(pass.metadata.vertex_inputs.attributes.len(), 1);
        assert!(pass.metadata.hlsl.contains("struct VertexInput"));
        assert!(!pass.metadata.hlsl.contains("#include"));
        assert!(!pass.metadata.hlsl.contains("float2 position"));
        fs::remove_dir_all(base_dir).unwrap();
    }

    #[test]
    fn compiles_configured_postprocessing_effects_and_validates_parameters() {
        let base_dir = temporary_directory("postprocessing");
        fs::write(
            base_dir.join("material.mat"),
            "material:\n  name: TestMaterial\n  shader: shader.shader\n",
        )
        .unwrap();
        fs::write(
            base_dir.join("shader.shader"),
            r#"
shader "TestShader" {
    pass "Forward" {
        face "back"
        queue "opaque"
        hlsl {
            struct VertexInput { float3 position : POSITION; };
            float4 VSMain(VertexInput input) : SV_Position { return float4(input.position, 1.0); }
            float4 PSMain() : SV_Target0 { return float4(1.0, 1.0, 1.0, 1.0); }
        }
    }
}
"#,
        )
        .unwrap();
        let source = r#"
pipeline:
  name: PostprocessingTest
postprocessing:
  - type: blur
    radius: 2.5
  - type: tone_mapping
    exposure: 1.25
  - type: fxaa
    edge_threshold: 0.125
    min_edge_threshold: 0.0312
passes:
  - name: Forward
    material: material.mat
    shader_pass: Forward
    color_target: swapchain
"#;
        let pipeline = RenderPipeline::from_yaml(source, &base_dir).unwrap();
        assert_eq!(pipeline.post_processing.len(), 4);
        assert_eq!(pipeline.post_processing[0].name(), "blur_horizontal");
        assert_eq!(pipeline.post_processing[1].name(), "blur_vertical");
        assert_eq!(pipeline.post_processing[2].name(), "tone_mapping");
        assert_eq!(pipeline.post_processing[3].name(), "fxaa");
        assert!(
            pipeline.post_processing[0]
                .fragment_shader_source()
                .contains("2.50000000 / width")
        );
        assert!(
            pipeline.post_processing[2]
                .fragment_shader_source()
                .contains("color.rgb * 1.25000000")
        );
        pipeline.compile_shaders().unwrap();
        for index in 0..pipeline.post_processing.len() {
            for stage in ["vert", "frag"] {
                let module =
                    fs::read(base_dir.join(format!("compiled_shaders/post_{index}.{stage}.spv")))
                        .unwrap();
                assert!(module.len() > 4);
                assert_eq!(
                    u32::from_le_bytes(module[..4].try_into().unwrap()),
                    0x0723_0203
                );
            }
        }
        let invalid = source.replace("radius: 2.5", "radius: 0.0");
        assert!(
            format!(
                "{:#}",
                RenderPipeline::from_yaml(&invalid, &base_dir).unwrap_err()
            )
            .contains("blur radius")
        );
        fs::remove_dir_all(base_dir).unwrap();
    }

    #[test]
    fn custom_postprocess_implementations_can_be_registered() {
        let registry = PostProcessRegistry::default();
        assert!(
            registry
                .registered_types()
                .unwrap()
                .contains(&"custom_tint")
        );
        let base_dir = temporary_directory("custom-postprocess");
        fs::write(
            base_dir.join("material.mat"),
            "material:\n  name: TestMaterial\n  shader: shader.shader\n",
        )
        .unwrap();
        fs::write(
            base_dir.join("shader.shader"),
            r#"
shader "TestShader" {
    pass "Forward" {
        face "back"
        queue "opaque"
        hlsl {
            struct VertexInput { float3 position : POSITION; };
            float4 VSMain(VertexInput input) : SV_Position { return float4(input.position, 1.0); }
            float4 PSMain() : SV_Target0 { return float4(1.0, 1.0, 1.0, 1.0); }
        }
    }
}
"#,
        )
        .unwrap();
        let source = r#"
pipeline:
  name: CustomPostprocessTest
postprocessing:
  - type: custom_tint
    intensity: 0.75
passes:
  - name: Forward
    material: material.mat
    shader_pass: Forward
    color_target: swapchain
"#;
        let pipeline =
            RenderPipeline::from_yaml_with_registry(source, &base_dir, &registry).unwrap();
        assert_eq!(pipeline.post_processing.len(), 1);
        assert_eq!(pipeline.post_processing[0].name(), "custom_tint");
        assert!(
            pipeline.post_processing[0]
                .fragment_shader_source()
                .contains("input.uv) * 0.75000000")
        );
        pipeline.compile_shaders().unwrap();
        let module = fs::read(base_dir.join("compiled_shaders/post_0.frag.spv")).unwrap();
        assert!(module.len() > 4);
        assert_eq!(
            u32::from_le_bytes(module[..4].try_into().unwrap()),
            0x0723_0203
        );
        fs::remove_dir_all(base_dir).unwrap();
    }
}
