use super::game_object_serde::GameObjectDefinition;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SceneDefinition {
    pub(crate) root: GameObjectDefinition,
    #[serde(default, rename = "deltaTime")]
    pub(crate) delta_time: f32,
    #[serde(default, rename = "timeSinceStart")]
    pub(crate) time_since_start: f32,
}

#[derive(Deserialize)]
pub(crate) struct ComponentDefinition {
    #[serde(rename = "type")]
    pub(crate) type_name: String,
    #[serde(default = "empty_component_data")]
    pub(crate) data: serde_yaml::Value,
}

fn empty_component_data() -> serde_yaml::Value {
    serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
}
