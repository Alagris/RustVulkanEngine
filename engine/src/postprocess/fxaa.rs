use super::{PostProcessPass, fullscreen_fragment_shader, post_process};
use anyhow::{Result, bail};
use serde::Deserialize;

#[post_process("fxaa")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FxaaPass {
    #[serde(default = "default_edge_threshold")]
    edge_threshold: f32,
    #[serde(default = "default_min_edge_threshold")]
    min_edge_threshold: f32,
}

fn default_edge_threshold() -> f32 {
    0.125
}

fn default_min_edge_threshold() -> f32 {
    0.0312
}

impl PostProcessPass for FxaaPass {
    fn name(&self) -> &str {
        "fxaa"
    }

    fn fragment_shader_source(&self) -> String {
        fullscreen_fragment_shader(&format!(
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
    float threshold = max({:.8}, luma_max * {:.8});
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
"#,
            self.edge_threshold, self.min_edge_threshold
        ))
    }

    fn validate(&self) -> Result<()> {
        if !self.edge_threshold.is_finite() || !(0.0..=1.0).contains(&self.edge_threshold) {
            bail!("FXAA edge_threshold must be finite and between zero and one");
        }
        if !self.min_edge_threshold.is_finite() || !(0.0..=1.0).contains(&self.min_edge_threshold) {
            bail!("FXAA min_edge_threshold must be finite and between zero and one");
        }
        Ok(())
    }
}
