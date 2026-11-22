use anyhow::{Context, Result, anyhow};
use shaderc::{CompileOptions, Compiler, EnvVersion, ShaderKind, SourceLanguage, TargetEnv};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct RenderPipeline {
    pub name: String,
    pub passes: Vec<RenderPass>,
    pub post_processing: Vec<PostProcessPass>,
    pub swapchain: SwapchainSettings,
    pub render_buffers: RenderBufferSettings,
    pub(crate) output_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PostProcessPass {
    BlurHorizontal {
        radius: f32,
    },
    BlurVertical {
        radius: f32,
    },
    ToneMapping {
        exposure: f32,
    },
    Fxaa {
        edge_threshold: f32,
        min_edge_threshold: f32,
    },
}

impl PostProcessPass {
    fn fragment_source(&self) -> String {
        let effect = match self {
            Self::BlurHorizontal { radius } => {
                format!("float2({radius:.8} / width, 0.0)")
            }
            Self::BlurVertical { radius } => {
                format!("float2(0.0, {radius:.8} / height)")
            }
            Self::ToneMapping { exposure } => format!(
                "float3 mapped = 1.0 - exp(-color.rgb * {:.8});\n    return float4(mapped, color.a);",
                exposure
            ),
            Self::Fxaa {
                edge_threshold,
                min_edge_threshold,
            } => format!(
                r#"
    uint width, height;
    Source.GetDimensions(width, height);
    float2 inverse_size = 1.0 / float2(width, height);
    float3 northwest = Source.Sample(SourceSampler, input.uv + inverse_size * float2(-1.0, -1.0)).rgb;
    float3 northeast = Source.Sample(SourceSampler, input.uv + inverse_size * float2(1.0, -1.0)).rgb;
    float3 southwest = Source.Sample(SourceSampler, input.uv + inverse_size * float2(-1.0, 1.0)).rgb;
    float3 southeast = Source.Sample(SourceSampler, input.uv + inverse_size * float2(1.0, 1.0)).rgb;
    float3 center = Source.Sample(SourceSampler, input.uv).rgb;
    float luma_nw = dot(northwest, float3(0.299, 0.587, 0.114));
    float luma_ne = dot(northeast, float3(0.299, 0.587, 0.114));
    float luma_sw = dot(southwest, float3(0.299, 0.587, 0.114));
    float luma_se = dot(southeast, float3(0.299, 0.587, 0.114));
    float luma_center = dot(center, float3(0.299, 0.587, 0.114));
    float luma_min = min(luma_center, min(min(luma_nw, luma_ne), min(luma_sw, luma_se)));
    float luma_max = max(luma_center, max(max(luma_nw, luma_ne), max(luma_sw, luma_se)));
    float threshold = max({edge_threshold:.8}, luma_max * {min_edge_threshold:.8});
    if (luma_max - luma_min < threshold)
    {{
        return float4(center, 1.0);
    }}
    float2 direction;
    direction.x = -((luma_nw + luma_ne) - (luma_sw + luma_se));
    direction.y = (luma_nw + luma_sw) - (luma_ne + luma_se);
    float reduce = max((luma_nw + luma_ne + luma_sw + luma_se) * 0.03125, 0.0078125);
    float reciprocal = 1.0 / (min(abs(direction.x), abs(direction.y)) + reduce);
    direction = clamp(direction * reciprocal, float2(-8.0, -8.0), float2(8.0, 8.0)) * inverse_size;
    float3 filtered_a = 0.5 * (
        Source.Sample(SourceSampler, input.uv + direction * (1.0 / 3.0 - 0.5)).rgb +
        Source.Sample(SourceSampler, input.uv + direction * (2.0 / 3.0 - 0.5)).rgb);
    float3 filtered_b = filtered_a * 0.5 + 0.25 * (
        Source.Sample(SourceSampler, input.uv + direction * -0.5).rgb +
        Source.Sample(SourceSampler, input.uv + direction * 0.5).rgb);
    float luma_b = dot(filtered_b, float3(0.299, 0.587, 0.114));
    float3 result = (luma_b < luma_min || luma_b > luma_max) ? filtered_a : filtered_b;
    return float4(result, 1.0);
"#
            ),
        };
        let body = match self {
            Self::BlurHorizontal { .. } | Self::BlurVertical { .. } => format!(
                r#"
    uint width, height;
    Source.GetDimensions(width, height);
    float2 offset = {effect};
    float4 color = Source.Sample(SourceSampler, input.uv) * 0.227027;
    color += Source.Sample(SourceSampler, input.uv + offset * 1.0) * 0.1945946;
    color += Source.Sample(SourceSampler, input.uv - offset * 1.0) * 0.1945946;
    color += Source.Sample(SourceSampler, input.uv + offset * 2.0) * 0.1216216;
    color += Source.Sample(SourceSampler, input.uv - offset * 2.0) * 0.1216216;
    color += Source.Sample(SourceSampler, input.uv + offset * 3.0) * 0.054054;
    color += Source.Sample(SourceSampler, input.uv - offset * 3.0) * 0.054054;
    color += Source.Sample(SourceSampler, input.uv + offset * 4.0) * 0.016216;
    color += Source.Sample(SourceSampler, input.uv - offset * 4.0) * 0.016216;
    return color;
"#
            ),
            Self::ToneMapping { .. } => format!(
                r#"
    float4 color = Source.Sample(SourceSampler, input.uv);
    {effect}
"#
            ),
            Self::Fxaa { .. } => effect,
        };
        format!(
            r#"
[[vk::binding(0, 0)]] Texture2D<float4> Source;
[[vk::binding(1, 0)]] SamplerState SourceSampler;

struct VertexOutput {{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
}};

VertexOutput VSMain(uint vertex_id : SV_VertexID)
{{
    VertexOutput output;
    float2 position = vertex_id == 0 ? float2(-1.0, -1.0) :
        (vertex_id == 1 ? float2(3.0, -1.0) : float2(-1.0, 3.0));
    output.position = float4(position, 0.0, 1.0);
    output.uv = position * 0.5 + 0.5;
    return output;
}}

float4 PSMain(VertexOutput input) : SV_Target0
{{
    {body}
}}
"#
        )
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
            let source = pass.fragment_source();
            for (stage, shader_kind, entry) in [
                ("vert", ShaderKind::Vertex, "VSMain"),
                ("frag", ShaderKind::Fragment, "PSMain"),
            ] {
                let output = self.output_dir.join(format!("post_{index}.{stage}.spv"));
                let mut options = CompileOptions::new()
                    .context("Failed to initialize embedded shaderc compile options")?;
                options.set_source_language(SourceLanguage::HLSL);
                options.set_target_env(TargetEnv::Vulkan, EnvVersion::Vulkan1_0 as u32);
                options.set_hlsl_io_mapping(true);
                options.set_auto_map_locations(true);
                options.set_auto_bind_uniforms(true);
                let source = if stage == "vert" {
                    source
                        .split("float4 PSMain")
                        .next()
                        .expect("generated postprocess shader has a fragment entry point")
                } else {
                    &source
                };
                let artifact = compiler
                    .compile_into_spirv(source, shader_kind, "postprocess.hlsl", entry, Some(&options))
                    .map_err(|error| {
                        anyhow!(
                            "Embedded HLSL compilation failed for postprocess pass {index} ({stage}): {error}"
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
                    "Compiled {} shader for postprocess pass {index} with embedded shaderc to {}.",
                    stage,
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
        assert_eq!(
            pipeline.post_processing[0],
            PostProcessPass::BlurHorizontal { radius: 2.5 }
        );
        assert_eq!(
            pipeline.post_processing[1],
            PostProcessPass::BlurVertical { radius: 2.5 }
        );
        assert_eq!(
            pipeline.post_processing[2],
            PostProcessPass::ToneMapping { exposure: 1.25 }
        );
        assert_eq!(
            pipeline.post_processing[3],
            PostProcessPass::Fxaa {
                edge_threshold: 0.125,
                min_edge_threshold: 0.0312
            }
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
            RenderPipeline::from_yaml(&invalid, &base_dir)
                .unwrap_err()
                .to_string()
                .contains("blur radius")
        );
        fs::remove_dir_all(base_dir).unwrap();
    }
}
