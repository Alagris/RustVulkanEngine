use crate::render_pipeline::{ShaderInputAttribute, ShaderInputFormat, ShaderInputLayout};
use anyhow::{Context, Result, anyhow, bail};

lalrpop_util::lalrpop_mod!(hlsl_layout);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlslLayoutDocument {
    pub structs: Vec<HlslStruct>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlslStruct {
    pub name: String,
    pub fields: Vec<HlslField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlslField {
    pub name: String,
    pub hlsl_type: String,
    pub semantic: String,
}

#[derive(Clone, Debug)]
pub(crate) enum HlslLayoutToken {
    Struct,
    Identifier(String),
    Type(String),
    Semantic(String),
    Text(String),
    LBrace,
    RBrace,
    Colon,
    Semicolon,
    Equals,
}

#[derive(Clone, Debug)]
pub(crate) struct HlslLayoutLexError {
    offset: usize,
    message: String,
}

impl std::fmt::Display for HlslLayoutLexError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for HlslLayoutLexError {}

struct HlslLayoutLexer<'input> {
    source: &'input str,
    offset: usize,
    after_colon: bool,
}

impl<'input> HlslLayoutLexer<'input> {
    fn new(source: &'input str) -> Self {
        Self {
            source,
            offset: 0,
            after_colon: false,
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

    fn skip_comment(&mut self) -> std::result::Result<bool, HlslLayoutLexError> {
        if self.source[self.offset..].starts_with("//") {
            self.take_while(|character| character != '\n');
            Ok(true)
        } else if self.source[self.offset..].starts_with("/*") {
            let start = self.offset;
            self.offset += 2;
            while self.offset < self.source.len() && !self.source[self.offset..].starts_with("*/") {
                let character = self.source[self.offset..].chars().next().unwrap();
                self.offset += character.len_utf8();
            }
            if self.source[self.offset..].starts_with("*/") {
                self.offset += 2;
                Ok(true)
            } else {
                Err(HlslLayoutLexError {
                    offset: start,
                    message: "unterminated block comment".to_owned(),
                })
            }
        } else {
            Ok(false)
        }
    }

    fn skip_string(&mut self) -> std::result::Result<bool, HlslLayoutLexError> {
        if !self.source[self.offset..].starts_with('"') {
            return Ok(false);
        }
        let start = self.offset;
        self.offset += 1;
        let mut escaped = false;
        while let Some(character) = self.source[self.offset..].chars().next() {
            self.offset += character.len_utf8();
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                return Ok(true);
            }
        }
        Err(HlslLayoutLexError {
            offset: start,
            message: "unterminated string literal".to_owned(),
        })
    }
}

impl Iterator for HlslLayoutLexer<'_> {
    type Item = std::result::Result<(usize, HlslLayoutToken, usize), HlslLayoutLexError>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.offset < self.source.len() {
            let start = self.offset;
            let rest = &self.source[self.offset..];
            let character = rest.chars().next().unwrap();

            if character.is_whitespace() {
                self.take_while(char::is_whitespace);
                continue;
            }
            match self.skip_comment() {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => return Some(Err(error)),
            }
            match self.skip_string() {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => return Some(Err(error)),
            }

            let token = if character.is_ascii_alphabetic() || character == '_' {
                let word = self
                    .take_while(|character| character.is_ascii_alphanumeric() || character == '_');
                if word == "struct" {
                    HlslLayoutToken::Struct
                } else if is_hlsl_input_type(word) {
                    HlslLayoutToken::Type(word.to_owned())
                } else if self.after_colon {
                    self.after_colon = false;
                    HlslLayoutToken::Semantic(word.to_owned())
                } else {
                    HlslLayoutToken::Identifier(word.to_owned())
                }
            } else {
                self.offset += character.len_utf8();
                match character {
                    '{' => HlslLayoutToken::LBrace,
                    '}' => HlslLayoutToken::RBrace,
                    ':' => {
                        self.after_colon = true;
                        HlslLayoutToken::Colon
                    }
                    ';' => {
                        self.after_colon = false;
                        HlslLayoutToken::Semicolon
                    }
                    '=' => HlslLayoutToken::Equals,
                    _ => HlslLayoutToken::Text(character.to_string()),
                }
            };
            return Some(Ok((start, token, self.offset)));
        }
        None
    }
}

fn is_hlsl_input_type(word: &str) -> bool {
    matches!(
        word,
        "float"
            | "float2"
            | "float3"
            | "float4"
            | "int"
            | "int2"
            | "int3"
            | "int4"
            | "uint"
            | "uint2"
            | "uint3"
            | "uint4"
    )
}

pub fn parse(source: &str) -> Result<HlslLayoutDocument> {
    hlsl_layout::HlslLayoutsParser::new()
        .parse(HlslLayoutLexer::new(source))
        .map_err(|error| anyhow!("{error:?}"))
        .context("Failed to parse HLSL input layouts")
}

pub fn parse_input_layouts(source: &str) -> Result<(ShaderInputLayout, ShaderInputLayout)> {
    let document = parse(source)?;
    let vertex = document
        .structs
        .iter()
        .find(|structure| structure.name == "VertexInput")
        .ok_or_else(|| anyhow!("HLSL must declare struct VertexInput"))?;
    let vertex = build_layout(vertex, false)?;
    let instance = document
        .structs
        .iter()
        .find(|structure| structure.name == "InstanceInput")
        .map(|structure| build_layout(structure, true))
        .transpose()?
        .unwrap_or_default();
    Ok((vertex, instance))
}

fn build_layout(structure: &HlslStruct, instance: bool) -> Result<ShaderInputLayout> {
    if structure.fields.is_empty() {
        bail!(
            "struct {} must contain at least one input field",
            structure.name
        );
    }
    let mut attributes = Vec::new();
    let mut offset = 0_u32;
    let mut stride_alignment = 1_u32;
    for field in &structure.fields {
        let format = parse_shader_input_format(&field.hlsl_type)
            .ok_or_else(|| anyhow!("unsupported shader input type '{}'", field.hlsl_type))?;
        let location = input_location(&field.semantic, instance)
            .ok_or_else(|| anyhow!("unsupported HLSL input semantic '{}'", field.semantic))?;
        if attributes
            .iter()
            .any(|attribute: &ShaderInputAttribute| attribute.location == location)
        {
            bail!("duplicate shader input location {location}");
        }
        let alignment = format.alignment();
        offset = align_up(offset, alignment)?;
        stride_alignment = stride_alignment.max(alignment);
        attributes.push(ShaderInputAttribute {
            location,
            format,
            offset,
        });
        offset = offset
            .checked_add(format.byte_size())
            .ok_or_else(|| anyhow!("shader input layout size overflow"))?;
    }
    attributes.sort_by_key(|attribute| attribute.location);
    Ok(ShaderInputLayout {
        stride: align_up(offset, stride_alignment)?,
        attributes,
    })
}

fn parse_shader_input_format(hlsl_type: &str) -> Option<ShaderInputFormat> {
    Some(match hlsl_type {
        "float" => ShaderInputFormat::Float32,
        "float2" => ShaderInputFormat::Float32x2,
        "float3" => ShaderInputFormat::Float32x3,
        "float4" => ShaderInputFormat::Float32x4,
        "int" => ShaderInputFormat::Sint32,
        "int2" => ShaderInputFormat::Sint32x2,
        "int3" => ShaderInputFormat::Sint32x3,
        "int4" => ShaderInputFormat::Sint32x4,
        "uint" => ShaderInputFormat::Uint32,
        "uint2" => ShaderInputFormat::Uint32x2,
        "uint3" => ShaderInputFormat::Uint32x3,
        "uint4" => ShaderInputFormat::Uint32x4,
        _ => return None,
    })
}

fn input_location(semantic: &str, instance: bool) -> Option<u32> {
    let semantic = semantic.to_ascii_uppercase();
    if instance {
        return semantic
            .strip_prefix("INSTANCE")
            .and_then(|index| index.parse::<u32>().ok())
            .and_then(|index| index.checked_add(2));
    }
    if semantic == "POSITION" {
        return Some(0);
    }
    semantic_index(&semantic, "COLOR", 1)
        .or_else(|| semantic_index(&semantic, "TEXCOORD", 2))
        .or_else(|| semantic_index(&semantic, "NORMAL", 8))
}

fn semantic_index(semantic: &str, prefix: &str, base: u32) -> Option<u32> {
    let suffix = semantic.strip_prefix(prefix)?;
    let index = if suffix.is_empty() {
        0
    } else {
        suffix.parse::<u32>().ok()?
    };
    base.checked_add(index)
}

fn align_up(value: u32, alignment: u32) -> Result<u32> {
    assert!(alignment.is_power_of_two());
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or_else(|| anyhow!("shader input alignment overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_layout(source: &str) -> (ShaderInputLayout, ShaderInputLayout) {
        parse_input_layouts(source).expect("input layout should parse")
    }

    #[test]
    fn accepts_empty_hlsl_document_but_requires_vertex_input_for_layout() {
        assert!(parse("").unwrap().structs.is_empty());
        assert!(
            parse_input_layouts("")
                .unwrap_err()
                .to_string()
                .contains("VertexInput")
        );
    }

    #[test]
    fn derives_vertex_offsets_and_aligned_stride() {
        let (vertex, instance) = parse_layout(
            "struct VertexInput { float3 position : POSITION; float4 color : COLOR0; };",
        );
        assert_eq!(
            vertex,
            ShaderInputLayout {
                stride: 32,
                attributes: vec![
                    ShaderInputAttribute {
                        location: 0,
                        format: ShaderInputFormat::Float32x3,
                        offset: 0,
                    },
                    ShaderInputAttribute {
                        location: 1,
                        format: ShaderInputFormat::Float32x4,
                        offset: 16,
                    },
                ],
            }
        );
        assert_eq!(instance, ShaderInputLayout::default());
    }

    #[test]
    fn supports_float_integer_and_unsigned_scalar_and_vector_inputs() {
        let types = [
            ("float", ShaderInputFormat::Float32),
            ("float2", ShaderInputFormat::Float32x2),
            ("float3", ShaderInputFormat::Float32x3),
            ("float4", ShaderInputFormat::Float32x4),
            ("int", ShaderInputFormat::Sint32),
            ("int2", ShaderInputFormat::Sint32x2),
            ("int3", ShaderInputFormat::Sint32x3),
            ("int4", ShaderInputFormat::Sint32x4),
            ("uint", ShaderInputFormat::Uint32),
            ("uint2", ShaderInputFormat::Uint32x2),
            ("uint3", ShaderInputFormat::Uint32x3),
            ("uint4", ShaderInputFormat::Uint32x4),
        ];

        for (hlsl_type, expected_format) in types {
            let source = format!("struct VertexInput {{ {hlsl_type} position : POSITION; }};");
            let document = parse(&source).unwrap();
            let layout = build_layout(&document.structs[0], false).unwrap();
            assert_eq!(layout.attributes[0].format, expected_format, "{hlsl_type}");
            assert_eq!(
                layout.stride,
                expected_format
                    .byte_size()
                    .next_multiple_of(expected_format.alignment()),
                "{hlsl_type}"
            );
        }
    }

    #[test]
    fn parses_multiple_structs_and_selects_named_input_structs() {
        let (vertex, instance) = parse_layout(
            r#"
                struct Irrelevant { float4 value : COLOR0; };
                struct VertexInput { float3 position : POSITION; };
                struct InstanceInput { float4 row : INSTANCE0; };
            "#,
        );
        assert_eq!(vertex.attributes[0].location, 0);
        assert_eq!(instance.attributes[0].location, 2);
    }

    #[test]
    fn ignores_braces_and_struct_keywords_inside_comments_and_strings() {
        let source = r#"
            // struct VertexInput { float4 bad : COLOR0; };
            const char* text = "struct VertexInput { }";
            /* struct InstanceInput { float4 bad : INSTANCE0; }; */
            struct VertexInput { float3 position : POSITION; };
        "#;
        let (vertex, instance) = parse_layout(source);
        assert_eq!(vertex.attributes.len(), 1);
        assert!(instance.attributes.is_empty());
    }

    #[test]
    fn ignores_bound_resources_with_scalar_and_template_element_types() {
        let source = r#"
            [[vk::binding(0, 0)]]
            RWStructuredBuffer<float> outputBuffer : register(u0);
            [[vk::binding(1, 0)]]
            Buffer<uint> inputBuffer : register(t0);
            float exposure : register(b1);
            struct VertexInput { float3 position : POSITION; };
            struct InstanceInput { float4 world0 : INSTANCE0; };
        "#;
        let (vertex, instance) = parse_layout(source);
        assert_eq!(vertex.attributes.len(), 1);
        assert_eq!(instance.attributes.len(), 1);
    }

    #[test]
    fn ignores_non_struct_compute_declarations_containing_braces() {
        let source = r#"
            groupshared float cache[64];
            [numthreads(8, 1, 1)]
            void CSMain(uint3 dispatchId : SV_DispatchThreadID) {
                cache[dispatchId.x] = 0.0;
            }
            struct VertexInput { float3 position : POSITION; };
        "#;
        let (vertex, instance) = parse_layout(source);
        assert_eq!(vertex.attributes.len(), 1);
        assert!(instance.attributes.is_empty());
    }

    #[test]
    fn rejects_empty_vertex_input() {
        let error = parse_input_layouts("struct VertexInput { };").unwrap_err();
        assert!(error.to_string().contains("at least one input field"));
    }

    #[test]
    fn rejects_unsupported_vertex_type() {
        let error = parse_input_layouts("struct VertexInput { double3 position : POSITION; };")
            .unwrap_err();
        assert!(error.to_string().contains("Failed to parse"));
    }

    #[test]
    fn rejects_unknown_semantic() {
        let error = parse_input_layouts("struct VertexInput { float3 position : BLENDINDICES0; };")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported HLSL input semantic")
        );
    }

    #[test]
    fn rejects_invalid_semantic_suffix() {
        let error =
            parse_input_layouts("struct VertexInput { float2 uv : TEXCOORDoops; };").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported HLSL input semantic")
        );
    }

    #[test]
    fn rejects_duplicate_vertex_locations() {
        let error = parse_input_layouts(
            "struct VertexInput { float4 first : COLOR; float4 second : COLOR0; };",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("duplicate shader input location")
        );
    }

    #[test]
    fn rejects_duplicate_instance_locations() {
        let error = parse_input_layouts(
            "struct VertexInput { float3 position : POSITION; }; \
             struct InstanceInput { float4 first : INSTANCE0; float4 second : INSTANCE0; };",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("duplicate shader input location")
        );
    }

    #[test]
    fn rejects_unsupported_instance_semantic() {
        let error = parse_input_layouts(
            "struct VertexInput { float3 position : POSITION; }; \
             struct InstanceInput { float4 transform : TEXCOORD0; };",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported HLSL input semantic")
        );
    }

    #[test]
    fn rejects_malformed_struct_fields() {
        assert!(parse("struct VertexInput { float3 position : POSITION };").is_err());
        assert!(parse("struct VertexInput { float3 : POSITION; };").is_err());
        assert!(parse("struct VertexInput { float3 position POSITION; };").is_err());
    }

    #[test]
    fn rejects_unterminated_comments_and_strings() {
        assert!(
            parse_input_layouts(
                "struct VertexInput { float3 position : POSITION; }; /* unfinished"
            )
            .is_err()
        );
        assert!(
            parse_input_layouts("struct VertexInput { float3 position : POSITION; }; \"unfinished")
                .is_err()
        );
    }

    #[test]
    fn maps_vertex_semantics_to_distinct_locations() {
        let (vertex, _) = parse_layout(
            "struct VertexInput { \
                float3 position : POSITION; \
                float4 color : COLOR1; \
                float2 uv : TEXCOORD3; \
                float3 normal : NORMAL2; \
            };",
        );
        assert_eq!(
            vertex
                .attributes
                .iter()
                .map(|attribute| attribute.location)
                .collect::<Vec<_>>(),
            vec![0, 2, 5, 10]
        );
    }
}
