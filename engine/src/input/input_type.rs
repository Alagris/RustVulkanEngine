use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum InputType {
    Bool,
    Float,
    Vector,
    Vector2,
}

pub(crate) fn source_type(key_code: u32) -> InputType {
    match key_code {
        crate::input::input_code::MOUSE_MOVE => InputType::Vector2,
        crate::input::input_code::SCROLL => InputType::Float,
        _ => InputType::Bool,
    }
}
