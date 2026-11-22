use crate::ecs::Component;
use crate::ecs::Scene;
use crate::input::modifier::{InputState, InputValue};
use std::rc::Rc;

pub type InputCallback<T> = fn(&mut T, &Scene, &str, u32, InputState, InputValue);

pub(crate) type ErasedInputCallback =
    dyn Fn(&mut dyn Component, &Scene, &str, u32, InputState, InputValue);

pub(crate) fn erase_callback<T: Component + 'static>(
    callback: InputCallback<T>,
) -> Rc<ErasedInputCallback> {
    Rc::new(move |component, scene, map, key_code, state, value| {
        if let Some(component) = component.as_any_mut().downcast_mut::<T>() {
            callback(component, scene, map, key_code, state, value);
        }
    })
}
