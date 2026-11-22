use super::{PostProcessPass, fullscreen_fragment_shader, post_process};
use anyhow::{Result, bail};
use serde::Deserialize;

#[post_process("blur")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlurPass {
    #[serde(default = "default_radius")]
    radius: f32,
}

fn default_radius() -> f32 {
    2.0
}

impl PostProcessPass for BlurPass {
    fn name(&self) -> &str {
        "blur"
    }

    fn fragment_shader_source(&self) -> String {
        blur_fragment_shader(self.radius, true)
    }

    fn validate(&self) -> Result<()> {
        if !self.radius.is_finite() || self.radius <= 0.0 {
            bail!("blur radius must be finite and greater than zero");
        }
        Ok(())
    }

    fn into_stages(self: Box<Self>) -> Vec<Box<dyn PostProcessPass>> {
        vec![
            Box::new(BlurStage {
                radius: self.radius,
                horizontal: true,
            }),
            Box::new(BlurStage {
                radius: self.radius,
                horizontal: false,
            }),
        ]
    }
}

struct BlurStage {
    radius: f32,
    horizontal: bool,
}

impl PostProcessPass for BlurStage {
    fn name(&self) -> &str {
        if self.horizontal {
            "blur_horizontal"
        } else {
            "blur_vertical"
        }
    }

    fn fragment_shader_source(&self) -> String {
        blur_fragment_shader(self.radius, self.horizontal)
    }
}

fn blur_fragment_shader(radius: f32, horizontal: bool) -> String {
    let offset = if horizontal {
        format!("float2({radius:.8} / width, 0.0)")
    } else {
        format!("float2(0.0, {radius:.8} / height)")
    };
    fullscreen_fragment_shader(&format!(
        r#"
    uint width, height;
    Source.GetDimensions(width, height);
    float2 offset = {offset};
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
    ))
}
