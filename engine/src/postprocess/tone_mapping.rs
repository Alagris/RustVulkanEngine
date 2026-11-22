use super::{PostProcessPass, fullscreen_fragment_shader, post_process};
use anyhow::{Result, bail};
use serde::Deserialize;

#[post_process("tone_mapping")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToneMappingPass {
    #[serde(default = "default_exposure")]
    exposure: f32,
}

fn default_exposure() -> f32 {
    1.0
}

impl PostProcessPass for ToneMappingPass {
    fn name(&self) -> &str {
        "tone_mapping"
    }

    fn fragment_shader_source(&self) -> String {
        fullscreen_fragment_shader(&format!(
            r#"
    float4 color = Source.Sample(SourceSampler, input.uv);
    float3 mapped = 1.0 - exp(-color.rgb * {:.8});
    return float4(mapped, color.a);
"#,
            self.exposure
        ))
    }

    fn validate(&self) -> Result<()> {
        if !self.exposure.is_finite() || self.exposure <= 0.0 {
            bail!("tone mapping exposure must be finite and greater than zero");
        }
        Ok(())
    }
}
