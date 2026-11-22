use crate::data::data_asset::{
    DataAsset, DataAssetCell, DataAssetDeserializeContext, DataAssetTarget,
};
use crate::input::input_type::InputType;
use anyhow::{Result, anyhow, bail};
use glam::{Vec2, Vec3};
use serde_yaml::Value;
use std::sync::Arc;

pub trait Modifier: DataAsset {
    fn input_type(&self) -> InputType;
    fn output_type(&self) -> InputType;
    fn create_instance(&self) -> Result<Box<dyn InputModifier>>;
}

pub trait InputModifier {
    fn process(&mut self, input: &mut InputEvent);

    fn update(&mut self, _delta_time: f32) -> Vec<InputEvent> {
        Vec::new()
    }

    fn reset(&mut self) {}
}

pub type ModifierCast = for<'a> fn(&'a dyn DataAsset) -> Option<&'a dyn Modifier>;

pub struct ModifierRegistration {
    pub type_name: &'static str,
    pub cast: ModifierCast,
}

pub use anyhow as anyhow_crate;
pub use inventory;
inventory::collect!(ModifierRegistration);

fn registration(type_name: &str) -> Option<&'static ModifierRegistration> {
    inventory::iter::<ModifierRegistration>
        .into_iter()
        .find(|registration| registration.type_name == type_name)
}

impl DataAssetTarget for dyn Modifier {
    fn type_name() -> Option<&'static str> {
        None
    }

    fn cast<'a>(asset: &'a dyn DataAsset, cell: &'a DataAssetCell) -> Option<&'a Self> {
        (registration(cell.registration.type_name)?.cast)(asset)
    }

    fn resolve_link(
        context: &DataAssetDeserializeContext,
        name: &str,
        type_name: Option<&str>,
    ) -> Result<(Arc<DataAssetCell>, String, String)> {
        let type_name = match type_name {
            Some(type_name) => {
                if registration(type_name).is_none() {
                    bail!("data asset type '{type_name}' is not an input modifier");
                }
                type_name
            }
            None => {
                let mut candidates = inventory::iter::<ModifierRegistration>
                    .into_iter()
                    .filter(|registration| context.has_asset(name, registration.type_name));
                let registration = candidates
                    .next()
                    .ok_or_else(|| anyhow!("no modifier data asset named '{name}' exists"))?;
                if candidates.next().is_some() {
                    bail!("modifier data asset name '{name}' is ambiguous; specify its type");
                }
                registration.type_name
            }
        };
        context.resolve_asset_link(name, type_name)
    }

    fn deserialize_nested(
        context: &DataAssetDeserializeContext,
        type_name: Option<&str>,
        value: Value,
    ) -> Result<Arc<DataAssetCell>> {
        let type_name = type_name
            .ok_or_else(|| anyhow!("nested modifier data assets must specify a 'type'"))?;
        let registration = registration(type_name)
            .ok_or_else(|| anyhow!("data asset type '{type_name}' is not an input modifier"))?;
        let cell = context.create_nested_asset(type_name, &value)?;
        let asset = cell
            .asset()
            .ok_or_else(|| anyhow!("nested modifier asset '{type_name}' was not initialized"))?;
        if (registration.cast)(asset).is_none() {
            bail!("data asset factory for '{type_name}' returned a non-modifier");
        }
        Ok(cell)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputValue {
    Bool(bool),
    Float(f32),
    Vector(Vec3),
    Vector2(Vec2),
}

impl InputValue {
    pub fn input_type(self) -> InputType {
        match self {
            Self::Bool(_) => InputType::Bool,
            Self::Float(_) => InputType::Float,
            Self::Vector(_) => InputType::Vector,
            Self::Vector2(_) => InputType::Vector2,
        }
    }

    pub fn neutral(input_type: InputType) -> Self {
        match input_type {
            InputType::Bool => Self::Bool(false),
            InputType::Float => Self::Float(0.0),
            InputType::Vector => Self::Vector(Vec3::ZERO),
            InputType::Vector2 => Self::Vector2(Vec2::ZERO),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputState {
    Started,
    Ongoing,
    Triggered,
    Canceled,
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputEvent {
    pub state: InputState,
    pub value: InputValue,
}

impl InputEvent {
    pub fn new(state: InputState, value: InputValue) -> Self {
        Self { state, value }
    }
}
