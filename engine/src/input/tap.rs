use crate::input::input_type::InputType;
use crate::input::modifier::{InputEvent, InputModifier, InputState};
use anyhow::Result;
use component_macros::{DataAsset, Modifier};
use serde::{Deserialize, Serialize};

#[derive(DataAsset, Modifier, Debug, Deserialize, Serialize)]
#[modifier(
    compile_with = compile_tap,
    input_type = InputType::Bool,
    output_type = InputType::Bool
)]
pub struct Tap {}

fn compile_tap(_asset: &Tap) -> Result<Box<dyn InputModifier>> {
    Ok(Box::new(TapState { active: false }))
}

struct TapState {
    active: bool,
}

impl InputModifier for TapState {
    fn process(&mut self, input: &mut InputEvent) {
        match input.state {
            InputState::Started => {
                self.active = true;
                input.state = InputState::Triggered;
            }
            InputState::Ongoing | InputState::Triggered => {}
            InputState::Ended | InputState::Canceled if self.active => {
                self.active = false;
            }
            InputState::Ended | InputState::Canceled => {}
        }
    }

    fn reset(&mut self) {
        self.active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::modifier::InputValue;

    fn event(state: InputState, value: bool) -> InputEvent {
        InputEvent::new(state, InputValue::Bool(value))
    }

    #[test]
    fn press_triggers_once_then_reports_ongoing_until_release() {
        let mut tap = TapState { active: false };
        let mut input = event(InputState::Started, true);
        tap.process(&mut input);
        assert_eq!(input.state, InputState::Triggered);

        for _ in 0..2 {
            let mut input = event(InputState::Ongoing, true);
            tap.process(&mut input);
            assert_eq!(input.state, InputState::Ongoing);
        }

        let mut input = event(InputState::Ended, false);
        tap.process(&mut input);
        assert_eq!(input.state, InputState::Ended);
    }

    #[test]
    fn cancellation_clears_the_active_tap_without_ending() {
        let mut tap = TapState { active: false };
        let mut input = event(InputState::Started, true);
        tap.process(&mut input);

        let mut input = event(InputState::Canceled, false);
        tap.process(&mut input);
        assert_eq!(input.state, InputState::Canceled);

        let mut input = event(InputState::Ended, false);
        tap.process(&mut input);
        assert_eq!(input.state, InputState::Ended);
    }

    #[test]
    fn ongoing_input_without_a_start_does_not_trigger() {
        let mut tap = TapState { active: false };
        let mut input = event(InputState::Ongoing, true);
        tap.process(&mut input);
        assert_eq!(input.state, InputState::Ongoing);
        let mut input = event(InputState::Ongoing, false);
        tap.process(&mut input);
        assert_eq!(input.state, InputState::Ongoing);
    }
}
