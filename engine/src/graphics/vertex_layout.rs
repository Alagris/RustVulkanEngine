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
