use crate::input::input_type::InputType;
use crate::input::modifier::{InputEvent, InputModifier, InputValue};
use anyhow::Result;
use component_macros::{DataAsset, Modifier};
use serde::{Deserialize, Serialize};

#[derive(DataAsset, Modifier, Debug, Deserialize, Serialize)]
#[modifier(
    compile_with = compile_swizzle,
    input_type = InputType::Vector2,
    output_type = InputType::Vector2
)]
pub struct Swizzle {}

fn compile_swizzle(_asset: &Swizzle) -> Result<Box<dyn InputModifier>> {
    Ok(Box::new(SwizzleState))
}

struct SwizzleState;

impl InputModifier for SwizzleState {
    fn process(&mut self, input: &mut InputEvent) {
        if let InputValue::Vector2(value) = input.value {
            input.value = InputValue::Vector2(glam::Vec2::new(value.y, value.x));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::modifier::InputState;

    #[test]
    fn swaps_vector2_axes_without_changing_the_input_state() {
        let mut swizzle = SwizzleState;
        let input = InputEvent::new(
            crate::input::modifier::InputState::Ongoing,
            InputValue::Vector2(glam::Vec2::new(2.0, 5.0)),
        );

        let mut input = input;
        swizzle.process(&mut input);
        assert_eq!(
            input,
            InputEvent::new(
                InputState::Ongoing,
                InputValue::Vector2(glam::Vec2::new(5.0, 2.0))
            )
        );
    }
}
