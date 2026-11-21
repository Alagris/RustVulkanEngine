use anyhow::{Context, Result, anyhow, bail};
use shaderc::{CompileOptions, Compiler, EnvVersion, ShaderKind, SourceLanguage, TargetEnv};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

lalrpop_util::lalrpop_mod!(shader_grammar);

#[derive(Clone, Debug)]
pub struct RenderPipeline {
    pub name: String,
    pub passes: Vec<RenderPass>,
    pub swapchain: SwapchainSettings,
    pub render_buffers: RenderBufferSettings,
    pub(crate) output_dir: PathBuf,
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

#[derive(Clone, Debug)]
pub(crate) enum Token {
    Shader,
    Pass,
    Face,
    Queue,
    Hlsl,
    LBrace,
    RBrace,
    Identifier(String),
    Quoted(String),
    Text(String),
}

#[derive(Clone, Debug)]
pub(crate) struct LexError {
    pub(crate) offset: usize,
    pub(crate) message: String,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for LexError {}

impl std::fmt::Display for Token {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

struct ShaderLexer<'input> {
    source: &'input str,
    offset: usize,
    in_hlsl: bool,
    hlsl_depth: usize,
    after_hlsl_keyword: bool,
}

impl<'input> ShaderLexer<'input> {
    fn new(source: &'input str) -> Self {
        Self {
            source,
            offset: 0,
            in_hlsl: false,
            hlsl_depth: 0,
            after_hlsl_keyword: false,
        }
    }

    fn take_while(&mut self, predicate: impl Fn(char) -> bool) -> &'input str {
        let start = self.offset;
        while let Some(character) = self.source[self.offset..].chars().next() {
            if !predicate(character) {
                break;
            }
            self.offset += character.len_utf8();
        }
        &self.source[start..self.offset]
    }

    fn quoted(&mut self) -> std::result::Result<Token, LexError> {
        let start = self.offset;
        self.offset += 1;
        let mut escaped = false;
        while let Some(character) = self.source[self.offset..].chars().next() {
            self.offset += character.len_utf8();
            if escaped {
                if !matches!(character, '"' | '\\' | 'n' | 'r' | 't') {
                    return Err(LexError {
                        offset: self.offset - character.len_utf8() - 1,
                        message: format!("unsupported escape '\\{}'", character),
                    });
                }
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                return Ok(Token::Quoted(self.source[start..self.offset].to_owned()));
            }
        }
        Err(LexError {
            offset: start,
            message: "unterminated quoted string".to_owned(),
        })
    }

    fn comment(&mut self) -> &'input str {
        let start = self.offset;
        if self.source[self.offset..].starts_with("//") {
            self.offset += 2;
            self.take_while(|character| character != '\n');
        } else {
            self.offset += 2;
            while self.offset < self.source.len() && !self.source[self.offset..].starts_with("*/") {
                let character = self.source[self.offset..].chars().next().unwrap();
                self.offset += character.len_utf8();
            }
            if self.source[self.offset..].starts_with("*/") {
                self.offset += 2;
            }
        }
        &self.source[start..self.offset]
    }
}

impl Iterator for ShaderLexer<'_> {
    type Item = std::result::Result<(usize, Token, usize), LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.offset < self.source.len() {
            let start = self.offset;
            let rest = &self.source[self.offset..];
            let character = rest.chars().next().unwrap();

            if rest.starts_with("//") || rest.starts_with("/*") {
                let text = self.comment().to_owned();
                if self.in_hlsl {
                    return Some(Ok((start, Token::Text(text), self.offset)));
                }
                continue;
            }

            if character.is_whitespace() {
                let text = self.take_while(char::is_whitespace).to_owned();
                if self.in_hlsl {
                    return Some(Ok((start, Token::Text(text), self.offset)));
                }
                continue;
            }

            let token = if character == '"' {
                match self.quoted() {
                    Ok(token) => token,
                    Err(error) => return Some(Err(error)),
                }
            } else if character == '\'' && self.in_hlsl {
                let start = self.offset;
                self.offset += 1;
                let mut escaped = false;
                let mut terminated = false;
                while let Some(character) = self.source[self.offset..].chars().next() {
                    self.offset += character.len_utf8();
                    if escaped {
                        escaped = false;
                    } else if character == '\\' {
                        escaped = true;
                    } else if character == '\'' {
                        terminated = true;
                        break;
                    }
                }
                if !terminated {
                    return Some(Err(LexError {
                        offset: start,
                        message: "unterminated character literal".to_owned(),
                    }));
                }
                Token::Text(self.source[start..self.offset].to_owned())
            } else if character.is_ascii_alphabetic() || character == '_' {
                let word = self
                    .take_while(|character| character.is_ascii_alphanumeric() || character == '_');
                match word {
                    "shader" => Token::Shader,
                    "pass" => Token::Pass,
                    "face" => Token::Face,
                    "queue" => Token::Queue,
                    "hlsl" => Token::Hlsl,
                    _ => Token::Identifier(word.to_owned()),
                }
            } else {
                self.offset += character.len_utf8();
                match character {
                    '{' => {
                        if self.after_hlsl_keyword {
                            self.in_hlsl = true;
                            self.hlsl_depth = 1;
                            self.after_hlsl_keyword = false;
                        } else if self.in_hlsl {
                            self.hlsl_depth += 1;
                        }
                        Token::LBrace
                    }
                    '}' => {
                        if self.in_hlsl {
                            self.hlsl_depth -= 1;
                            if self.hlsl_depth == 0 {
                                self.in_hlsl = false;
                            }
                        }
                        Token::RBrace
                    }
                    _ if self.in_hlsl => Token::Text(character.to_string()),
                    _ => {
                        return Some(Err(LexError {
                            offset: start,
                            message: format!("unexpected character '{}'", character),
                        }));
                    }
                }
            };

            self.after_hlsl_keyword = !self.in_hlsl && matches!(token, Token::Hlsl);
            let token = if self.in_hlsl {
                match token {
                    Token::Identifier(text) => Token::Text(text),
                    Token::Quoted(text) => Token::Text(text),
                    Token::Shader => Token::Text("shader".to_owned()),
                    Token::Pass => Token::Text("pass".to_owned()),
                    Token::Face => Token::Text("face".to_owned()),
                    Token::Queue => Token::Text("queue".to_owned()),
                    Token::Hlsl => Token::Text("hlsl".to_owned()),
                    token => token,
                }
            } else {
                token
            };
            return Some(Ok((start, token, self.offset)));
        }
        None
    }
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
        Ok(())
    }
}

pub(crate) fn parse_shader_definition(source: &str) -> Result<ShaderDefinition> {
    let mut shader = shader_grammar::ShaderParser::new()
        .parse(ShaderLexer::new(source))
        .map_err(|error| anyhow!("{error}"))?;
    let mut names = HashSet::new();
    for pass in &mut shader.passes {
        if !names.insert(&pass.name) {
            bail!("shader pass names must be unique: '{}'", pass.name);
        }
        let (vertex_inputs, instance_inputs) =
            crate::parsers::hlsl_parser::parse_input_layouts(&pass.hlsl)
                .with_context(|| format!("Invalid input layout in shader pass '{}'", pass.name))?;
        pass.vertex_inputs = vertex_inputs;
        pass.instance_inputs = instance_inputs;
    }
    Ok(shader)
}

pub fn shader_asset_name(source: &str) -> Result<String> {
    parse_shader_definition(source).map(|shader| shader.name)
}

pub(super) fn unquote(value: String) -> String {
    let mut chars = value[1..value.len() - 1].chars();
    let mut decoded = String::new();
    while let Some(character) = chars.next() {
        if character == '\\' {
            let escaped = chars.next().expect("lexer validates quoted escapes");
            decoded.push(match escaped {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                _ => unreachable!("lexer validates quoted escapes"),
            });
        } else {
            decoded.push(character);
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parses_multiple_shader_passes_with_quoted_metadata() {
        let source = r#"
shader "Test" {
    pass "Forward" {
        face "back"
        queue "opaque"
        hlsl {
            struct VertexInput { float3 position : POSITION; };
            float4 VSMain(VertexInput input) : SV_Position { return 0; }
            float4 PSMain() : SV_Target0 { return 1; }
        }
    }
    pass "Shadow" {
        face "front"
        queue "transparent"
        hlsl {
            struct VertexInput { float3 position : POSITION; };
            float4 VSMain(VertexInput input) : SV_Position { return 0; }
            float4 PSMain() : SV_Target0 { return 1; }
        }
    }
}
"#;
        let shader = parse_shader_definition(source).unwrap();
        assert_eq!(shader.name, "Test");
        assert_eq!(shader.passes.len(), 2);
        assert_eq!(shader.passes[0].name, "Forward");
        assert_eq!(shader.passes[0].cull, CullMode::Back);
        assert_eq!(shader.passes[0].queue, RenderQueue::Opaque);
        assert_eq!(shader.passes[0].vertex_inputs.stride, 16);
        assert_eq!(shader.passes[1].name, "Shadow");
        assert_eq!(shader.passes[1].cull, CullMode::Front);
        assert_eq!(shader.passes[1].queue, RenderQueue::Transparent);
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
}
