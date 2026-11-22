use crate::input::input_type::InputType;
use crate::input::modifier::{InputEvent, InputModifier, InputState, InputValue};
use anyhow::{Result, bail};
use component_macros::{DataAsset, Modifier};
use serde::{Deserialize, Serialize};

#[derive(DataAsset, Modifier, Debug, Deserialize, Serialize)]
#[modifier(
    compile_with = compile_hold,
    input_type = InputType::Bool,
    output_type = InputType::Bool
)]
pub struct Hold {
    pub duration: f32,
}

fn compile_hold(asset: &Hold) -> Result<Box<dyn InputModifier>> {
    if !asset.duration.is_finite() || asset.duration <= 0.0 {
        bail!("Hold duration must be finite and greater than zero");
    }
    Ok(Box::new(HoldState {
        duration: asset.duration,
        elapsed: 0.0,
        pressed: false,
        triggered: false,
    }))
}

struct HoldState {
    duration: f32,
    elapsed: f32,
    pressed: bool,
    triggered: bool,
}

impl InputModifier for HoldState {
    fn process(&mut self, input: &mut InputEvent) {
        match input.state {
            InputState::Started => {
                self.pressed = true;
                self.triggered = false;
                self.elapsed = 0.0;
            }
            InputState::Ongoing => {}
            InputState::Ended if self.pressed => {
                self.pressed = false;
                input.state = if self.triggered {
                    InputState::Ended
                } else {
                    InputState::Canceled
                };
                input.value = InputValue::Bool(false);
                self.elapsed = 0.0;
                self.triggered = false;
            }
            InputState::Canceled => {
                self.reset();
                input.value = InputValue::Bool(false);
            }
            InputState::Triggered | InputState::Ended => {}
        }
    }

    fn update(&mut self, delta_time: f32) -> Vec<InputEvent> {
        if !self.pressed || self.triggered {
            return Vec::new();
        }
        self.elapsed += delta_time;
        if self.elapsed >= self.duration {
            self.triggered = true;
            return vec![InputEvent::new(
                InputState::Triggered,
                InputValue::Bool(true),
            )];
        }
        Vec::new()
    }

    fn reset(&mut self) {
        self.elapsed = 0.0;
        self.pressed = false;
        self.triggered = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(state: InputState, value: bool) -> InputEvent {
        InputEvent::new(state, InputValue::Bool(value))
    }

    fn hold(duration: f32) -> HoldState {
        HoldState {
            duration,
            elapsed: 0.0,
            pressed: false,
            triggered: false,
        }
    }

    #[test]
    fn triggers_once_after_duration_and_ends_on_release() {
        let mut hold = hold(0.5);
        let mut input = event(InputState::Started, true);
        hold.process(&mut input);
        assert_eq!(input.state, InputState::Started);
        assert!(hold.update(0.49).is_empty());
        let mut input = event(InputState::Ongoing, true);
        hold.process(&mut input);
        assert_eq!(input.state, InputState::Ongoing);
        assert_eq!(hold.update(0.01), [event(InputState::Triggered, true)]);
        assert!(hold.update(0.1).is_empty());
        let mut input = event(InputState::Ongoing, true);
        hold.process(&mut input);
        assert_eq!(input.state, InputState::Ongoing);
        let mut input = event(InputState::Ended, false);
        hold.process(&mut input);
        assert_eq!(input.state, InputState::Ended);
    }

    #[test]
    fn release_before_duration_cancels_without_ending() {
        let mut hold = hold(0.5);
        let mut input = event(InputState::Started, true);
        hold.process(&mut input);
        assert!(hold.update(0.2).is_empty());

        let mut input = event(InputState::Ended, false);
        hold.process(&mut input);
        assert_eq!(input.state, InputState::Canceled);
        assert!(hold.update(0.5).is_empty());
    }

    #[test]
    fn external_cancellation_resets_the_hold_timer() {
        let mut hold = hold(0.5);
        let mut input = event(InputState::Started, true);
        hold.process(&mut input);
        hold.update(0.2);
        let mut input = event(InputState::Canceled, false);
        hold.process(&mut input);
        assert_eq!(input.state, InputState::Canceled);
        assert!(hold.update(1.0).is_empty());
        let mut input = event(InputState::Ended, false);
        hold.process(&mut input);
        assert_eq!(input.state, InputState::Ended);
    }

    #[test]
    fn rejects_non_positive_or_non_finite_durations() {
        for duration in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(compile_hold(&Hold { duration }).is_err());
        }
    }
}
