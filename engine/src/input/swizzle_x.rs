use crate::input::input_type::InputType;
use crate::input::modifier::{InputEvent, InputModifier, InputValue};
use anyhow::Result;
use component_macros::{DataAsset, Modifier};
use serde::{Deserialize, Serialize};

#[derive(DataAsset, Modifier, Debug, Deserialize, Serialize)]
#[modifier(
    compile_with = compile_swizzle_x,
    input_type = InputType::Float,
    output_type = InputType::Vector2
)]
pub struct SwizzleX {}

fn compile_swizzle_x(_asset: &SwizzleX) -> Result<Box<dyn InputModifier>> {
    Ok(Box::new(SwizzleXState))
}

struct SwizzleXState;

impl InputModifier for SwizzleXState {
    fn process(&mut self, input: &mut InputEvent) {
        if let InputValue::Float(value) = input.value {
            input.value = InputValue::Vector2(glam::Vec2::new(value, 0.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::modifier::InputState;

    #[test]
    fn converts_float_to_x_axis_and_preserves_input_state() {
        let mut swizzle_x = SwizzleXState;
        let input = InputEvent::new(InputState::Ongoing, InputValue::Float(-1.0));

        let mut input = input;
        swizzle_x.process(&mut input);
        assert_eq!(
            input,
            InputEvent::new(
                InputState::Ongoing,
                InputValue::Vector2(glam::Vec2::new(-1.0, 0.0))
            )
        );
    }
}
