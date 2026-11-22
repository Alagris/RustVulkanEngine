use crate::input::input_type::InputType;
use crate::input::modifier::{InputEvent, InputModifier, InputState, InputValue};
use anyhow::{Result, bail};
use component_macros::{DataAsset, Modifier};
use serde::{Deserialize, Serialize};

#[derive(DataAsset, Modifier, Debug, Deserialize, Serialize)]
#[modifier(
    compile_with = compile_double_tap,
    input_type = InputType::Bool,
    output_type = InputType::Bool
)]
pub struct DoubleTap {
    pub max_interval: f32,
}

fn compile_double_tap(asset: &DoubleTap) -> Result<Box<dyn InputModifier>> {
    if !asset.max_interval.is_finite() || asset.max_interval <= 0.0 {
        bail!("DoubleTap max_interval must be finite and greater than zero");
    }
    Ok(Box::new(DoubleTapState {
        max_interval: asset.max_interval,
        elapsed: 0.0,
        waiting: false,
        pressed: false,
        active: false,
    }))
}

struct DoubleTapState {
    max_interval: f32,
    elapsed: f32,
    waiting: bool,
    pressed: bool,
    active: bool,
}

impl InputModifier for DoubleTapState {
    fn process(&mut self, input: &mut InputEvent) {
        match input.state {
            InputState::Started if self.waiting && !self.pressed => {
                self.waiting = false;
                self.pressed = true;
                self.active = true;
                input.state = InputState::Triggered;
                input.value = InputValue::Bool(true);
            }
            InputState::Started => {
                self.waiting = true;
                self.elapsed = 0.0;
                self.pressed = true;
                self.active = false;
                input.value = InputValue::Bool(true);
            }
            InputState::Ongoing => {}
            InputState::Ended if self.pressed => {
                self.pressed = false;
                if self.active {
                    self.active = false;
                    input.value = InputValue::Bool(false);
                } else {
                    input.state = InputState::Canceled;
                    input.value = InputValue::Bool(false);
                }
            }
            InputState::Canceled => {
                self.reset();
                input.value = InputValue::Bool(false);
            }
            InputState::Triggered | InputState::Ended => {}
        }
    }

    fn update(&mut self, delta_time: f32) -> Vec<InputEvent> {
        if !self.waiting {
            return Vec::new();
        }
        self.elapsed += delta_time;
        if self.elapsed >= self.max_interval {
            self.reset();
            return vec![InputEvent::new(
                InputState::Canceled,
                InputValue::Bool(false),
            )];
        }
        Vec::new()
    }

    fn reset(&mut self) {
        self.elapsed = 0.0;
        self.waiting = false;
        self.pressed = false;
        self.active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(state: InputState, value: bool) -> InputEvent {
        InputEvent::new(state, InputValue::Bool(value))
    }

    fn double_tap(max_interval: f32) -> DoubleTapState {
        DoubleTapState {
            max_interval,
            elapsed: 0.0,
            waiting: false,
            pressed: false,
            active: false,
        }
    }

    #[test]
    fn triggers_on_the_second_press_within_the_interval() {
        let mut double_tap = double_tap(0.4);
        let mut input = event(InputState::Started, true);
        double_tap.process(&mut input);
        assert_eq!(input, event(InputState::Started, true));

        let mut input = event(InputState::Ongoing, true);
        double_tap.process(&mut input);
        assert_eq!(input, event(InputState::Ongoing, true));

        let mut input = event(InputState::Ended, false);
        double_tap.process(&mut input);
        assert_eq!(input, event(InputState::Canceled, false));
        assert!(double_tap.update(0.2).is_empty());
        let mut input = event(InputState::Started, true);
        double_tap.process(&mut input);
        assert_eq!(input, event(InputState::Triggered, true));

        let mut input = event(InputState::Ongoing, true);
        double_tap.process(&mut input);
        assert_eq!(input, event(InputState::Ongoing, true));

        let mut input = event(InputState::Ended, false);
        double_tap.process(&mut input);
        assert_eq!(input, event(InputState::Ended, false));
    }

    #[test]
    fn timeout_cancels_the_pending_tap_without_ending() {
        let mut double_tap = double_tap(0.4);
        let mut input = event(InputState::Started, true);
        double_tap.process(&mut input);
        let mut input = event(InputState::Ended, false);
        double_tap.process(&mut input);
        assert_eq!(input.state, InputState::Canceled);

        assert_eq!(double_tap.update(0.4), [event(InputState::Canceled, false)]);
        assert!(double_tap.update(0.1).is_empty());
        let mut input = event(InputState::Started, true);
        double_tap.process(&mut input);
        assert_eq!(input.state, InputState::Started);
    }

    #[test]
    fn cancellation_discards_pending_double_tap_state() {
        let mut double_tap = double_tap(0.4);
        let mut input = event(InputState::Started, true);
        double_tap.process(&mut input);

        let mut input = event(InputState::Canceled, false);
        double_tap.process(&mut input);
        assert_eq!(input.state, InputState::Canceled);
        assert!(double_tap.update(0.4).is_empty());
        let mut input = event(InputState::Started, true);
        double_tap.process(&mut input);
        assert_eq!(input.state, InputState::Started);
    }

    #[test]
    fn rejects_non_positive_or_non_finite_intervals() {
        for max_interval in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(compile_double_tap(&DoubleTap { max_interval }).is_err());
        }
    }
}
