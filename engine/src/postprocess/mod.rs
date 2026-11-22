use anyhow::{Context, Result, anyhow, bail};
use serde_yaml::Value;

mod blur;
mod fxaa;
mod tone_mapping;

pub use blur::BlurPass;
pub use component_macros::post_process;
pub use fxaa::FxaaPass;
pub use inventory;
pub use serde_yaml;
pub use tone_mapping::ToneMappingPass;

pub const POST_PROCESS_FRAGMENT_HEADER: &str = r#"
[[vk::binding(0, 0)]] Texture2D<float4> Source;
[[vk::binding(1, 0)]] SamplerState SourceSampler;

struct VertexOutput {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};
"#;

pub const FULLSCREEN_VERTEX_SHADER: &str = r#"
struct VertexOutput {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

VertexOutput VSMain(uint vertex_id : SV_VertexID)
{
    VertexOutput output;
    float2 position = vertex_id == 0 ? float2(-1.0, -1.0) :
        (vertex_id == 1 ? float2(3.0, -1.0) : float2(-1.0, 3.0));
    output.position = float4(position, 0.0, 1.0);
    output.uv = position * 0.5 + 0.5;
    return output;
}
"#;

/// Wraps a fragment body in the standard input-texture and fullscreen-pass declarations.
pub fn fullscreen_fragment_shader(body: &str) -> String {
    format!(
        "{POST_PROCESS_FRAGMENT_HEADER}\nfloat4 PSMain(VertexOutput input) : SV_Target0\n{{\n{body}\n}}\n"
    )
}

/// Defines one fullscreen effect stage. Shader entry points must be `VSMain` and `PSMain`.
///
/// The default vertex shader emits a fullscreen triangle. Fragment shaders can use `Source`,
/// `SourceSampler`, and `VertexOutput input`, or provide a custom vertex shader.
pub trait PostProcessPass: Send + Sync + 'static {
    fn name(&self) -> &str;

    fn vertex_shader_source(&self) -> String {
        FULLSCREEN_VERTEX_SHADER.to_owned()
    }

    fn fragment_shader_source(&self) -> String;

    fn validate(&self) -> Result<()> {
        Ok(())
    }

    fn into_stages(self: Box<Self>) -> Vec<Box<dyn PostProcessPass>>
    where
        Self: Sized,
    {
        vec![self]
    }
}

pub type PostProcessFactory = fn(&Value) -> Result<Vec<Box<dyn PostProcessPass>>>;

pub struct PostProcessRegistration {
    pub type_name: &'static str,
    pub factory: PostProcessFactory,
}

inventory::collect!(PostProcessRegistration);

#[derive(Clone, Copy, Debug, Default)]
pub struct PostProcessRegistry;

impl PostProcessRegistry {
    pub fn new() -> Self {
        Self
    }

    pub fn registered_types(&self) -> Result<Vec<&'static str>> {
        let mut registrations = inventory::iter::<PostProcessRegistration>
            .into_iter()
            .collect::<Vec<_>>();
        registrations.sort_by_key(|registration| registration.type_name);
        for pair in registrations.windows(2) {
            if pair[0].type_name == pair[1].type_name {
                bail!(
                    "postprocess type '{}' was registered more than once",
                    pair[0].type_name
                );
            }
        }
        Ok(registrations
            .into_iter()
            .map(|registration| registration.type_name)
            .collect())
    }

    pub(crate) fn create_stages(&self, mut config: Value) -> Result<Vec<Box<dyn PostProcessPass>>> {
        let mapping = config
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("postprocessing entries must be mappings"))?;
        let type_key = Value::String("type".to_owned());
        let type_name = mapping
            .remove(&type_key)
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| anyhow!("postprocessing entries must specify a string 'type'"))?;
        let mut registrations = inventory::iter::<PostProcessRegistration>
            .into_iter()
            .filter(|registration| registration.type_name == type_name);
        let registration = registrations
            .next()
            .ok_or_else(|| anyhow!("unknown postprocess type '{type_name}'"))?;
        if registrations.next().is_some() {
            return Err(anyhow!(
                "postprocess type '{type_name}' was registered more than once"
            ));
        }
        let stages = (registration.factory)(&config)
            .with_context(|| format!("Failed to create postprocess '{type_name}'"))?;
        if stages.is_empty() {
            bail!("postprocess '{type_name}' did not produce any render stages");
        }
        for stage in &stages {
            stage
                .validate()
                .with_context(|| format!("Invalid stage '{}' from '{type_name}'", stage.name()))?;
        }
        Ok(stages)
    }
}
