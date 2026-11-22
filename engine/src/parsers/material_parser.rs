use crate::parsers::material_lexer::{LexError, ShaderLexer, Token};
use crate::render_pipeline::{
    CullMode, RenderQueue, ShaderDefinition, ShaderInputLayout, ShaderPassMetadata,
};
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

lalrpop_util::lalrpop_mod!(shader_grammar);

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

    fn pass_source(name: &str, face: &str, queue: &str) -> String {
        format!(
            r#"
shader "Test" {{
    pass "{name}" {{
        face "{face}"
        queue "{queue}"
        hlsl {{
            struct VertexInput {{ float3 position : POSITION; }};
            float4 VSMain(VertexInput input) : SV_Position {{ return 0; }}
            float4 PSMain() : SV_Target0 {{ return 1; }}
        }}
    }}
}}
"#
        )
    }

    #[test]
    fn rejects_duplicate_pass_names() {
        let single = pass_source("Forward", "back", "opaque");
        let closing = single.rfind('}').unwrap();
        let mut source = single[..closing].to_owned();
        source.push_str(
            r#"
    pass "Forward" {
        face "back"
        queue "opaque"
        hlsl {
            struct VertexInput { float3 position : POSITION; };
            float4 VSMain(VertexInput input) : SV_Position { return 0; }
            float4 PSMain() : SV_Target0 { return 1; }
        }
    }
}
"#,
        );
        let error = parse_shader_definition(&source).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("shader pass names must be unique: 'Forward'")
        );
    }

    #[test]
    fn rejects_unsupported_cull_modes_and_render_queues() {
        let error = parse_shader_definition(&pass_source("Forward", "sideways", "opaque"))
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("unsupported face culling mode 'sideways'"),
            "unexpected error: {error:#}"
        );

        let error = parse_shader_definition(&pass_source("Forward", "back", "shiny")).unwrap_err();
        assert!(
            format!("{error:#}").contains("unsupported render queue 'shiny'"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn shader_asset_name_returns_declared_name_or_fails() {
        assert_eq!(
            shader_asset_name(&pass_source("Forward", "back", "opaque")).unwrap(),
            "Test"
        );
        assert!(shader_asset_name("float4 VSMain() : SV_Position { return 0; }").is_err());
    }

    #[test]
    fn material_files_require_non_empty_names_and_shader_paths() {
        let directory = std::env::temp_dir().join(format!(
            "engine-material-parser-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        for (file, contents, message) in [
            (
                "empty_name.mat",
                "material:\n  name: \"  \"\n  shader: Lit\n",
                "material name must not be empty",
            ),
            (
                "empty_shader.mat",
                "material:\n  name: Lit\n  shader: \"\"\n",
                "material shader path must not be empty",
            ),
            (
                "no_mapping.mat",
                "just a string\n",
                "Invalid material",
            ),
        ] {
            let path = directory.join(file);
            fs::write(&path, contents).unwrap();
            let error = parse(&path).unwrap_err();
            assert!(
                format!("{error:#}").contains(message),
                "expected '{message}' in '{error:#}'"
            );
        }
        let valid = directory.join("valid.mat");
        fs::write(&valid, "material:\n  name: Lit\n  shader: LitShader\n").unwrap();
        let material = parse(&valid).unwrap();
        assert_eq!(material.name, "Lit");
        assert_eq!(material.shader, PathBuf::from("LitShader"));
        fs::remove_dir_all(directory).unwrap();
    }
}
