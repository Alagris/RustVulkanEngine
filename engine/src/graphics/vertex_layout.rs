use crate::render_pipeline::{ShaderInputAttribute, ShaderInputLayout};
use anyhow::{Result, anyhow, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VertexLayout {
    pub stride: u32,
    pub attributes: Vec<ShaderInputAttribute>,
}

impl VertexLayout {
    pub fn new(stride: u32, attributes: Vec<ShaderInputAttribute>) -> Result<Self> {
        validate_input_layout(stride, &attributes)?;
        Ok(Self { stride, attributes })
    }

    pub(crate) fn validate_shader_inputs(&self, expected: &ShaderInputLayout) -> Result<()> {
        validate_input_layout(self.stride, &self.attributes)?;
        for input in &expected.attributes {
            let actual = self
                .attributes
                .iter()
                .find(|attribute| attribute.location == input.location)
                .ok_or_else(|| {
                    anyhow!(
                        "vertex layout is missing shader location {}",
                        input.location
                    )
                })?;
            if actual.format != input.format || actual.offset != input.offset {
                bail!(
                    "vertex layout location {} expects {:?} at byte {}, got {:?} at byte {}",
                    input.location,
                    input.format,
                    input.offset,
                    actual.format,
                    actual.offset
                );
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_input_layout(
    stride: u32,
    attributes: &[ShaderInputAttribute],
) -> Result<()> {
    if stride == 0 || attributes.is_empty() {
        bail!("buffer layouts require a positive stride and at least one attribute");
    }
    let mut locations = std::collections::HashSet::new();
    for attribute in attributes {
        if !locations.insert(attribute.location) {
            bail!(
                "buffer layout repeats shader location {}",
                attribute.location
            );
        }
        let end = attribute
            .offset
            .checked_add(attribute.format.byte_size())
            .ok_or_else(|| anyhow!("buffer attribute range overflow"))?;
        if end > stride {
            bail!(
                "buffer attribute at location {} extends beyond its stride",
                attribute.location
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_pipeline::ShaderInputFormat;

    fn attribute(location: u32, format: ShaderInputFormat, offset: u32) -> ShaderInputAttribute {
        ShaderInputAttribute {
            location,
            format,
            offset,
        }
    }

    #[test]
    fn accepts_attributes_that_exactly_fill_the_stride() {
        let layout = VertexLayout::new(
            28,
            vec![
                attribute(0, ShaderInputFormat::Float32x3, 0),
                attribute(1, ShaderInputFormat::Float32x4, 12),
            ],
        )
        .unwrap();
        assert_eq!(layout.stride, 28);
        assert_eq!(layout.attributes.len(), 2);
    }

    #[test]
    fn rejects_zero_stride_and_empty_attributes() {
        let error = VertexLayout::new(0, vec![attribute(0, ShaderInputFormat::Float32, 0)])
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("positive stride and at least one attribute")
        );
        let error = VertexLayout::new(16, Vec::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("positive stride and at least one attribute")
        );
    }

    #[test]
    fn rejects_duplicate_locations_overflow_and_out_of_stride_attributes() {
        let error = VertexLayout::new(
            32,
            vec![
                attribute(0, ShaderInputFormat::Float32x3, 0),
                attribute(0, ShaderInputFormat::Float32x3, 12),
            ],
        )
        .unwrap_err();
        assert!(error.to_string().contains("repeats shader location 0"));

        let error = VertexLayout::new(
            32,
            vec![attribute(0, ShaderInputFormat::Float32x4, u32::MAX - 4)],
        )
        .unwrap_err();
        assert!(error.to_string().contains("buffer attribute range overflow"));

        let error = VertexLayout::new(8, vec![attribute(0, ShaderInputFormat::Float32x3, 0)])
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("extends beyond its stride")
        );
    }

    #[test]
    fn shader_input_validation_requires_matching_location_format_and_offset() {
        let layout = VertexLayout::new(
            24,
            vec![
                attribute(0, ShaderInputFormat::Float32x3, 0),
                attribute(1, ShaderInputFormat::Float32x3, 12),
            ],
        )
        .unwrap();

        let matching = ShaderInputLayout {
            stride: 24,
            attributes: vec![attribute(1, ShaderInputFormat::Float32x3, 12)],
        };
        assert!(layout.validate_shader_inputs(&matching).is_ok());

        let missing = ShaderInputLayout {
            stride: 24,
            attributes: vec![attribute(2, ShaderInputFormat::Float32x3, 0)],
        };
        let error = layout.validate_shader_inputs(&missing).unwrap_err();
        assert!(error.to_string().contains("missing shader location 2"));

        let mismatched = ShaderInputLayout {
            stride: 24,
            attributes: vec![attribute(1, ShaderInputFormat::Float32x4, 12)],
        };
        let error = layout.validate_shader_inputs(&mismatched).unwrap_err();
        assert!(error.to_string().contains("location 1 expects"));
    }
}
