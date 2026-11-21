use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

lalrpop_util::lalrpop_mod!(pub shader_grammar);

#[derive(Clone, Debug)]
pub struct RenderPipeline {
    pub name: String,
    pub passes: Vec<RenderPass>,
    output_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct RenderPass {
    pub name: String,
    pub shader: PathBuf,
    pub shader_name: String,
    pub color_target: String,
    pub clear_color: Option<[f32; 4]>,
    pub metadata: ShaderPassMetadata,
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
}

#[derive(Deserialize)]
struct PipelineFile {
    pipeline: PipelineSettings,
    passes: Vec<PassSettings>,
}

#[derive(Deserialize)]
struct PipelineSettings {
    name: String,
}

#[derive(Deserialize)]
struct PassSettings {
    name: String,
    shader: PathBuf,
    shader_pass: String,
    color_target: String,
    clear_color: Option<[f32; 4]>,
}

#[derive(Debug)]
pub(super) struct ShaderDefinition {
    name: String,
    passes: Vec<ShaderPassMetadata>,
}

#[derive(Clone, Debug)]
pub(crate) enum Token {
    Shader,
    Pass,
    Face,
    Queue,
    Hlsl,
    None,
    Front,
    Back,
    Opaque,
    Transparent,
    LBrace,
    RBrace,
    Equals,
    Semicolon,
    Identifier(String),
    Quoted(String),
    Text(String),
}

#[derive(Clone, Debug)]
pub(crate) struct LexError {
    offset: usize,
    message: String,
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
                    "none" => Token::None,
                    "front" => Token::Front,
                    "back" => Token::Back,
                    "opaque" => Token::Opaque,
                    "transparent" => Token::Transparent,
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
                    '=' => Token::Equals,
                    ';' => Token::Semicolon,
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
                    Token::None => Token::Text("none".to_owned()),
                    Token::Front => Token::Text("front".to_owned()),
                    Token::Back => Token::Text("back".to_owned()),
                    Token::Opaque => Token::Text("opaque".to_owned()),
                    Token::Transparent => Token::Text("transparent".to_owned()),
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
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("Failed to read render pipeline {}", path.display()))?;
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        Self::from_toml(&source, base_dir)
            .with_context(|| format!("Invalid render pipeline {}", path.display()))
    }

    fn from_toml(source: &str, base_dir: &Path) -> Result<Self> {
        let config: PipelineFile = toml::from_str(source).context("Failed to parse TOML")?;
        if config.pipeline.name.trim().is_empty() {
            bail!("pipeline.name must not be empty");
        }
        if config.passes.is_empty() {
            bail!("pipeline must contain at least one [[passes]] entry");
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

            let shader_path = base_dir.join(&settings.shader);
            let shader_source = fs::read_to_string(&shader_path)
                .with_context(|| format!("Failed to read shader {}", shader_path.display()))?;
            let shader = parse_shader_definition(&shader_source)
                .with_context(|| format!("Invalid shader {}", shader_path.display()))?;
            let metadata = shader
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
                color_target: settings.color_target,
                clear_color: settings.clear_color,
                metadata,
            });
        }

        Ok(Self {
            name: config.pipeline.name,
            passes,
            output_dir: base_dir.join("compiled_shaders"),
        })
    }

    pub fn compile_shaders(&self) -> Result<()> {
        fs::create_dir_all(&self.output_dir).with_context(|| {
            format!(
                "Failed to create shader output directory {}",
                self.output_dir.display()
            )
        })?;
        let dxc = std::env::var_os("DXC").unwrap_or_else(|| "dxc".into());

        for (index, pass) in self.passes.iter().enumerate() {
            let source_path = self.output_dir.join(format!("pass_{index}.hlsl"));
            fs::write(&source_path, &pass.metadata.hlsl)
                .with_context(|| format!("Failed to write {}", source_path.display()))?;

            for (stage, profile, entry) in
                [("vert", "vs_6_0", "VSMain"), ("frag", "ps_6_0", "PSMain")]
            {
                let output = self.output_dir.join(format!("pass_{index}.{stage}.spv"));
                let result = Command::new(&dxc)
                    .arg("-spirv")
                    .arg("-T")
                    .arg(profile)
                    .arg("-E")
                    .arg(entry)
                    .arg("-fspv-target-env=vulkan1.0")
                    .arg("-Fo")
                    .arg(&output)
                    .arg(&source_path)
                    .output()
                    .with_context(|| {
                        format!(
                            "Failed to run DXC ('{}'); install DirectX Shader Compiler or set DXC",
                            Path::new(&dxc).display()
                        )
                    })?;
                if !result.status.success() {
                    bail!(
                        "DXC failed compiling pass '{}' {} shader:\n{}",
                        pass.name,
                        stage,
                        String::from_utf8_lossy(&result.stderr)
                    );
                }
                log::info!(
                    "Compiled {} shader for pass '{}' to {}.",
                    stage,
                    pass.name,
                    output.display()
                );
            }
        }
        Ok(())
    }
}

fn parse_shader_definition(source: &str) -> Result<ShaderDefinition> {
    let shader = shader_grammar::ShaderParser::new()
        .parse(ShaderLexer::new(source))
        .map_err(|error| anyhow!("{error}"))?;
    let mut names = HashSet::new();
    for pass in &shader.passes {
        if !names.insert(&pass.name) {
            bail!("shader pass names must be unique: '{}'", pass.name);
        }
    }
    Ok(shader)
}

fn unquote(value: String) -> String {
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

    fn sample_pipeline() -> RenderPipeline {
        RenderPipeline::load(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join("pipeline.toml"),
        )
        .unwrap()
    }

    #[test]
    fn loads_shader_and_multiple_passes() {
        let pipeline = sample_pipeline();
        assert_eq!(pipeline.name, "Forward");
        assert_eq!(pipeline.passes.len(), 2);
        assert_eq!(pipeline.passes[0].shader_name, "Unlit");
        assert_eq!(pipeline.passes[0].metadata.name, "Forward");
        assert_eq!(pipeline.passes[0].metadata.cull, CullMode::Back);
        assert_eq!(pipeline.passes[0].metadata.queue, RenderQueue::Opaque);
        assert!(pipeline.passes[0].metadata.hlsl.contains("VSMain"));
        assert_eq!(pipeline.passes[1].metadata.name, "ShadowCaster");
        assert_eq!(pipeline.passes[1].metadata.cull, CullMode::Front);
    }

    #[test]
    fn parses_nested_hlsl_braces_and_braces_in_strings_and_comments() {
        let source = r#"
shader "Test" {
    pass "Nested" {
        face = back;
        queue = opaque;
        hlsl {
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
        face = back;
        face = front;
        queue = opaque;
        hlsl { float4 VSMain() { return 0; } }
    }
}
"#;
        assert!(parse_shader_definition(source).is_err());
    }

    #[test]
    fn rejects_missing_pass_in_pipeline() {
        let base_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let source = r#"
[pipeline]
name = "Invalid"

[[passes]]
name = "Missing"
shader = "shaders/unlit.shader"
shader_pass = "NotDeclared"
color_target = "swapchain"
"#;
        assert!(
            RenderPipeline::from_toml(source, &base_dir)
                .unwrap_err()
                .to_string()
                .contains("not declared")
        );
    }
}
