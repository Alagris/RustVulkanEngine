use crate::data::data_asset::DataRef;
use crate::ecs::Component;
use crate::ecs::Scene;
use crate::ecs::component::ComponentWeak;
use crate::input::input_asset::Input;
use crate::input::input_callback::{ErasedInputCallback, InputCallback, erase_callback};
use crate::input::input_code;
use crate::input::input_type::{InputType, source_type};
use crate::input::key_binding::KeyBinding;
use crate::input::keymap::KeyMap;
use crate::input::modifier::{InputEvent, InputModifier, InputState, InputValue, Modifier};
use crate::input::mouse::{MouseControl, MouseGrabMode};
use anyhow::{Result, anyhow, bail};
use glam::Vec2;
use std::any::TypeId;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

#[derive(Clone, Debug)]
pub enum RawInput {
    Button { key_code: u32, pressed: bool },
    CursorMoved(Vec2),
    MouseMotion(Vec2),
    Scroll(f32),
    FocusLost,
}

pub struct InputSystem {
    maps: Vec<CompiledKeyMap>,
    map_indices: HashMap<String, usize>,
    active_map: Option<usize>,
    pressed_buttons: HashSet<u32>,
    cursor_position: Option<Vec2>,
    mouse: Option<MouseControl>,
}

impl Default for InputSystem {
    fn default() -> Self {
        Self {
            maps: Vec::new(),
            map_indices: HashMap::new(),
            active_map: None,
            pressed_buttons: HashSet::new(),
            cursor_position: None,
            mouse: None,
        }
    }
}

impl InputSystem {
    pub(crate) fn attach_window(&mut self, window: Rc<winit::window::Window>) -> Result<()> {
        self.mouse = Some(MouseControl::new(window)?);
        self.cursor_position = None;
        Ok(())
    }

    pub fn set_mouse_grab(&mut self, mode: MouseGrabMode) -> Result<()> {
        self.mouse
            .as_mut()
            .ok_or_else(|| anyhow!("mouse control requires a window"))?
            .set_mode(mode)?;
        self.cursor_position = None;
        Ok(())
    }

    pub fn mouse_grab(&self) -> Result<MouseGrabMode> {
        Ok(self
            .mouse
            .as_ref()
            .ok_or_else(|| anyhow!("mouse control requires a window"))?
            .mode())
    }

    pub fn set_mouse_visible(&mut self, visible: bool) -> Result<()> {
        self.mouse
            .as_mut()
            .ok_or_else(|| anyhow!("mouse control requires a window"))?
            .set_visible(visible);
        Ok(())
    }

    pub(crate) fn set_mouse_focus(&mut self, focused: bool) -> Result<()> {
        self.cursor_position = None;
        if let Some(mouse) = self.mouse.as_mut() {
            mouse.set_focus(focused)?;
        }
        Ok(())
    }

    pub(crate) fn uses_raw_mouse_motion(&self) -> bool {
        self.mouse
            .as_ref()
            .is_some_and(MouseControl::uses_raw_motion)
    }

    pub fn from_data_asset(input: &DataRef<Input>) -> Result<Self> {
        let compiled_maps = input
            .with(|input| {
                input
                    .keymaps
                    .iter()
                    .map(|(name, keymap)| {
                        let keymap = keymap
                            .with(|keymap| compile_keymap(name, keymap))
                            .ok_or_else(|| {
                                anyhow!("input key map '{name}' is no longer alive")
                            })??;
                        Ok((name.clone(), keymap))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .ok_or_else(|| anyhow!("input data asset is no longer alive"))??;
        if compiled_maps.is_empty() {
            bail!("input data asset must contain at least one key map");
        }
        let map_indices = compiled_maps
            .iter()
            .enumerate()
            .map(|(index, (name, _))| (name.clone(), index))
            .collect();
        let maps = compiled_maps
            .into_iter()
            .map(|(_, map)| map)
            .collect::<Vec<_>>();
        Ok(Self {
            maps,
            map_indices,
            active_map: Some(0),
            ..Self::default()
        })
    }

    pub fn active_map(&self) -> Option<&str> {
        self.active_map
            .and_then(|index| self.maps.get(index))
            .map(|map| map.name.as_str())
    }

    pub fn keymap_names(&self) -> Vec<&str> {
        self.maps.iter().map(|map| map.name.as_str()).collect()
    }

    pub fn binding_type(&self, map: &str, binding: &str) -> Result<InputType> {
        let index = *self
            .map_indices
            .get(map)
            .ok_or_else(|| anyhow!("input map '{map}' does not exist"))?;
        self.maps[index]
            .bindings
            .get(binding)
            .map(|binding| binding.output_type)
            .ok_or_else(|| anyhow!("input binding '{binding}' does not exist in map '{map}'"))
    }

    pub(crate) fn set_active_map(&mut self, name: &str) -> Result<Vec<InputNotification>> {
        let next = *self
            .map_indices
            .get(name)
            .ok_or_else(|| anyhow!("input map '{name}' does not exist"))?;
        if self.active_map == Some(next) {
            return Ok(Vec::new());
        }
        let mut notifications = Vec::new();
        if let Some(previous) = self.active_map {
            let map = &mut self.maps[previous];
            // Own the name before calling a method that mutably borrows the map.
            let previous_name = map.name.clone();
            map.cancel_all(&previous_name, &mut notifications);
        }
        self.active_map = Some(next);
        // Routing needs &mut self, so finish reading the pressed set first.
        let mut held = self.pressed_buttons.iter().cloned().collect::<Vec<_>>();
        held.sort_unstable();
        for source in held {
            let event = if input_code::is_mouse_code(source) {
                InputEvent::new(InputState::Ongoing, InputValue::Bool(true))
            } else {
                InputEvent::new(InputState::Started, InputValue::Bool(true))
            };
            self.route(source, event, &mut notifications)?;
        }
        Ok(notifications)
    }

    pub(crate) fn subscribe<T: Component + 'static>(
        &mut self,
        map: &str,
        binding: &str,
        component: &ComponentWeak<T>,
        callback: InputCallback<T>,
    ) -> Result<()> {
        let index = *self
            .map_indices
            .get(map)
            .ok_or_else(|| anyhow!("input map '{map}' does not exist"))?;
        let keymap = &mut self.maps[index];
        let binding = keymap
            .bindings
            .get_mut(binding)
            .ok_or_else(|| anyhow!("input binding '{binding}' does not exist in map '{map}'"))?;
        let component = component.downgrade();
        let component_type = TypeId::of::<T>();
        let callback_id = callback as usize;
        if !binding.subscriptions.iter().any(|existing| {
            existing.component_type == component_type
                && existing.callback_id == callback_id
                && Weak::ptr_eq(&existing.component, &component)
        }) {
            binding.subscriptions.push(InputSubscription {
                component,
                component_type,
                callback_id,
                callback: erase_callback(callback),
            });
        }
        Ok(())
    }

    pub(crate) fn unsubscribe_component<T: Component + 'static>(
        &mut self,
        component: &ComponentWeak<T>,
    ) -> usize {
        self.unsubscribe_matching(component, None)
    }

    pub(crate) fn unsubscribe_callback<T: Component + 'static>(
        &mut self,
        component: &ComponentWeak<T>,
        callback: InputCallback<T>,
    ) -> usize {
        self.unsubscribe_matching(component, Some(callback as usize))
    }

    fn unsubscribe_matching<T: Component + 'static>(
        &mut self,
        component: &ComponentWeak<T>,
        callback_id: Option<usize>,
    ) -> usize {
        let component = component.downgrade();
        let component_type = TypeId::of::<T>();
        let mut removed = 0;
        for map in &mut self.maps {
            for binding in map.bindings.values_mut() {
                binding.subscriptions.retain(|subscription| {
                    let matches_component = subscription.component_type == component_type
                        && Weak::ptr_eq(&subscription.component, &component);
                    let matches_callback = callback_id
                        .map_or(true, |callback_id| subscription.callback_id == callback_id);
                    let retain = !(matches_component && matches_callback);
                    if !retain {
                        removed += 1;
                    }
                    retain
                });
            }
        }
        removed
    }

    pub(crate) fn consume(&mut self, input: RawInput) -> Result<Vec<InputNotification>> {
        let mut notifications = Vec::new();
        match input {
            RawInput::Button { key_code, pressed } => {
                if pressed {
                    if !self.pressed_buttons.insert(key_code) {
                        return Ok(notifications);
                    }
                    let state = if input_code::is_mouse_code(key_code) {
                        InputState::Ongoing
                    } else {
                        InputState::Started
                    };
                    self.route(
                        key_code,
                        InputEvent::new(state, InputValue::Bool(true)),
                        &mut notifications,
                    )?;
                } else {
                    if !self.pressed_buttons.remove(&key_code) {
                        return Ok(notifications);
                    }
                    self.route(
                        key_code,
                        InputEvent::new(
                            if input_code::is_mouse_code(key_code) {
                                InputState::Ongoing
                            } else {
                                InputState::Ended
                            },
                            InputValue::Bool(false),
                        ),
                        &mut notifications,
                    )?;
                }
            }
            RawInput::CursorMoved(position) => {
                if !position.is_finite() {
                    bail!("cursor position must be finite");
                }
                let previous = self.cursor_position.replace(position);
                let Some(previous) = previous else {
                    return Ok(notifications);
                };
                let delta = position - previous;
                self.route_mouse_motion(delta, &mut notifications)?;
            }
            RawInput::MouseMotion(delta) => {
                self.route_mouse_motion(delta, &mut notifications)?;
            }
            RawInput::Scroll(amount) => {
                if !amount.is_finite() {
                    bail!("scroll amount must be finite");
                }
                if amount == 0.0 {
                    return Ok(notifications);
                }
                let value = InputValue::Float(amount.signum());
                self.route(
                    input_code::SCROLL,
                    InputEvent::new(InputState::Ongoing, value),
                    &mut notifications,
                )?;
            }
            RawInput::FocusLost => {
                // Drain before routing; route mutably borrows the whole input system.
                let mut pressed = self.pressed_buttons.drain().collect::<Vec<_>>();
                pressed.sort_unstable();
                for source in pressed {
                    self.route(
                        source,
                        InputEvent::new(
                            if input_code::is_mouse_code(source) {
                                InputState::Ongoing
                            } else {
                                InputState::Canceled
                            },
                            InputValue::Bool(false),
                        ),
                        &mut notifications,
                    )?;
                }
                self.cursor_position = None;
            }
        }
        Ok(notifications)
    }

    fn route_mouse_motion(
        &mut self,
        delta: Vec2,
        notifications: &mut Vec<InputNotification>,
    ) -> Result<()> {
        if !delta.is_finite() {
            bail!("mouse motion must be finite");
        }
        if delta != Vec2::ZERO {
            self.route(
                input_code::MOUSE_MOVE,
                InputEvent::new(InputState::Ongoing, InputValue::Vector2(delta)),
                notifications,
            )?;
        }
        Ok(())
    }

    pub(crate) fn update(&mut self, delta_time: f32) -> Result<Vec<InputNotification>> {
        if !delta_time.is_finite() || delta_time < 0.0 {
            bail!("input delta time must be finite and non-negative");
        }
        let mut notifications = Vec::new();
        // Snapshot keys before routing, which needs a mutable borrow of self.
        let mut pressed = self.pressed_buttons.iter().cloned().collect::<Vec<_>>();
        pressed.sort_unstable();
        for key_code in pressed {
            self.route(
                key_code,
                InputEvent::new(InputState::Ongoing, InputValue::Bool(true)),
                &mut notifications,
            )?;
        }
        if let Some(active_map) = self.active_map {
            let map = &mut self.maps[active_map];
            // Keep the map name owned while update takes a mutable borrow of the map.
            let map_name = map.name.clone();
            map.update(delta_time, &map_name, &mut notifications)?;
        }
        self.prune_listeners();
        Ok(notifications)
    }

    /// Dispatch only after the caller releases its mutable input-system borrow.
    /// Callbacks can re-enter Scene APIs that borrow or mutate the input system.
    pub(crate) fn dispatch(scene: &Scene, notifications: Vec<InputNotification>) -> Result<()> {
        for notification in notifications {
            let Some(component) = notification.component.upgrade() else {
                continue;
            };
            // Report an existing RefCell borrow instead of panicking during callback re-entry.
            let mut component = component.try_borrow_mut().map_err(|_| {
                anyhow!(
                    "input callback for key code {} targets a component that is already mutably borrowed",
                    notification.key_code
                )
            })?;
            (notification.callback)(
                &mut *component,
                scene,
                &notification.map,
                notification.key_code,
                notification.event.state,
                notification.event.value,
            );
        }
        Ok(())
    }

    fn route(
        &mut self,
        key_code: u32,
        event: InputEvent,
        notifications: &mut Vec<InputNotification>,
    ) -> Result<()> {
        let Some(active_map) = self.active_map else {
            return Ok(());
        };
        let map = &mut self.maps[active_map];
        // route mutably borrows the map, so pass it an owned name.
        let map_name = map.name.clone();
        map.route(key_code, event, &map_name, notifications)
    }

    fn prune_listeners(&mut self) {
        for map in &mut self.maps {
            for binding in map.bindings.values_mut() {
                binding
                    .subscriptions
                    .retain(|subscription| subscription.component.strong_count() > 0);
            }
        }
    }
}

/// Owns callback data returned to Scene for dispatch after its `RefMut<InputSystem>` ends.
pub(crate) struct InputNotification {
    map: String,
    key_code: u32,
    event: InputEvent,
    component: Weak<RefCell<dyn Component>>,
    callback: Rc<ErasedInputCallback>,
}

#[derive(Clone)]
struct InputSubscription {
    component: Weak<RefCell<dyn Component>>,
    component_type: TypeId,
    callback_id: usize,
    callback: Rc<ErasedInputCallback>,
}

struct CompiledKeyMap {
    name: String,
    bindings: HashMap<String, CompiledBinding>,
    by_key_code: HashMap<u32, Vec<String>>,
}

impl CompiledKeyMap {
    fn route(
        &mut self,
        key_code: u32,
        event: InputEvent,
        map_name: &str,
        notifications: &mut Vec<InputNotification>,
    ) -> Result<()> {
        let Some(bindings) = self.by_key_code.get(&key_code) else {
            return Ok(());
        };
        // Clone the index so bindings can be mutably accessed while it is traversed.
        for name in bindings.clone() {
            let binding = self
                .bindings
                .get_mut(&name)
                .ok_or_else(|| anyhow!("compiled input binding '{name}' is missing"))?;
            let mut event = event;
            if binding.process(&mut event)? {
                binding.append_notifications(map_name, event, notifications);
            }
        }
        Ok(())
    }

    fn update(
        &mut self,
        delta_time: f32,
        map_name: &str,
        notifications: &mut Vec<InputNotification>,
    ) -> Result<()> {
        for binding in self.bindings.values_mut() {
            for event in binding.update(delta_time)? {
                binding.append_notifications(map_name, event, notifications);
            }
        }
        Ok(())
    }

    fn cancel_all(&mut self, map_name: &str, notifications: &mut Vec<InputNotification>) {
        for binding in self.bindings.values_mut() {
            let was_active = binding.active;
            binding.reset();
            if was_active && !input_code::is_mouse_code(binding.key_code) {
                binding.append_notifications(
                    map_name,
                    InputEvent::new(
                        InputState::Canceled,
                        InputValue::neutral(binding.output_type),
                    ),
                    notifications,
                );
            }
        }
    }
}

struct CompiledBinding {
    key_code: u32,
    output_type: InputType,
    modifiers: Vec<Box<dyn InputModifier>>,
    modifier_output_types: Vec<InputType>,
    subscriptions: Vec<InputSubscription>,
    active: bool,
    suppress_until_release: bool,
}

impl CompiledBinding {
    fn process(&mut self, event: &mut InputEvent) -> Result<bool> {
        if self.suppress_until_release {
            match event.state {
                InputState::Started => self.suppress_until_release = false,
                InputState::Ended | InputState::Canceled => {
                    self.suppress_until_release = false;
                    return Ok(false);
                }
                InputState::Ongoing | InputState::Triggered => return Ok(false),
            }
        }
        let source_was_released = matches!(event.state, InputState::Ended | InputState::Canceled);
        let is_mouse_input = input_code::is_mouse_code(self.key_code);
        if is_mouse_input {
            event.state = InputState::Ongoing;
        }
        for (index, modifier) in self.modifiers.iter_mut().enumerate() {
            modifier.process(event);
            validate_output_types(
                std::slice::from_ref(event),
                self.modifier_output_types[index],
                self.key_code,
            )?;
        }
        let should_notify = self.track_event(*event);
        if !is_mouse_input && !source_was_released && event.state == InputState::Canceled {
            self.suppress_until_release = true;
        }
        Ok(should_notify)
    }

    fn update(&mut self, delta_time: f32) -> Result<Vec<InputEvent>> {
        if input_code::is_mouse_code(self.key_code) {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        for index in 0..self.modifiers.len() {
            // Finish this indexed mutable borrow before iterating the downstream modifiers.
            let generated = self.modifiers[index].update(delta_time);
            validate_output_types(&generated, self.modifier_output_types[index], self.key_code)?;
            for event in generated {
                let mut event = event;
                for (offset, modifier) in self.modifiers.iter_mut().enumerate().skip(index + 1) {
                    modifier.process(&mut event);
                    validate_output_types(
                        std::slice::from_ref(&event),
                        self.modifier_output_types[offset],
                        self.key_code,
                    )?;
                }
                events.push(event);
            }
        }
        self.track_events(&mut events);
        if events
            .iter()
            .any(|event| event.state == InputState::Canceled)
        {
            self.suppress_until_release = true;
        }
        Ok(events)
    }

    fn track_events(&mut self, events: &mut Vec<InputEvent>) {
        let mut tracked = Vec::with_capacity(events.len());
        for event in events.drain(..) {
            if self.track_event(event) {
                tracked.push(event);
            }
        }
        *events = tracked;
    }

    fn track_event(&mut self, event: InputEvent) -> bool {
        let should_deliver = event.state != InputState::Ended || self.active;
        match event.state {
            InputState::Started | InputState::Triggered => self.active = true,
            InputState::Ended | InputState::Canceled => self.active = false,
            InputState::Ongoing => {}
        }
        should_deliver
    }

    fn reset(&mut self) {
        self.active = false;
        self.suppress_until_release = false;
        for modifier in &mut self.modifiers {
            modifier.reset();
        }
    }

    fn append_notifications(
        &mut self,
        map: &str,
        event: InputEvent,
        notifications: &mut Vec<InputNotification>,
    ) {
        self.subscriptions.retain(|subscription| {
            if subscription.component.strong_count() > 0 {
                // Snapshot callbacks so queued notifications don't borrow the subscription list.
                notifications.push(InputNotification {
                    map: map.to_owned(),
                    key_code: self.key_code,
                    event,
                    component: subscription.component.clone(),
                    callback: Rc::clone(&subscription.callback),
                });
                true
            } else {
                false
            }
        });
    }
}

fn compile_keymap(name: &str, keymap: &KeyMap) -> Result<CompiledKeyMap> {
    let mut bindings = HashMap::new();
    let mut by_key_code: HashMap<u32, Vec<String>> = HashMap::new();
    for (name, binding) in &keymap.bindings {
        let compiled = binding
            .with(compile_binding)
            .ok_or_else(|| anyhow!("input binding '{name}' is no longer alive"))??;
        by_key_code
            .entry(compiled.key_code)
            .or_default()
            .push(name.clone());
        bindings.insert(name.clone(), compiled);
    }
    for names in by_key_code.values_mut() {
        names.sort_unstable();
    }
    Ok(CompiledKeyMap {
        name: name.to_owned(),
        bindings,
        by_key_code,
    })
}

fn compile_binding(binding: &KeyBinding) -> Result<CompiledBinding> {
    let mut input_type = source_type(binding.key);
    let mut modifiers = Vec::with_capacity(binding.modifiers.len());
    let mut modifier_output_types = Vec::with_capacity(binding.modifiers.len());
    for modifier in &binding.modifiers {
        let (modifier_input, modifier_output, instance) = modifier
            .with(|modifier: &dyn Modifier| {
                (
                    modifier.input_type(),
                    modifier.output_type(),
                    modifier.create_instance(),
                )
            })
            .ok_or_else(|| anyhow!("modifier for input code {} is no longer alive", binding.key))?;
        if modifier_input != input_type {
            bail!(
                "modifier expects {:?}, but input code {} provides {:?}",
                modifier_input,
                binding.key,
                input_type
            );
        }
        input_type = modifier_output;
        modifier_output_types.push(modifier_output);
        modifiers.push(instance?);
    }
    if binding.input_type != input_type {
        bail!(
            "input binding for code {} declares output type {:?}, but its source and modifiers produce {:?}",
            binding.key,
            binding.input_type,
            input_type
        );
    }
    Ok(CompiledBinding {
        key_code: binding.key,
        output_type: input_type,
        modifiers,
        modifier_output_types,
        subscriptions: Vec::new(),
        active: false,
        suppress_until_release: false,
    })
}

fn validate_output_types(events: &[InputEvent], expected: InputType, key_code: u32) -> Result<()> {
    if let Some(event) = events
        .iter()
        .find(|event| event.value.input_type() != expected)
    {
        bail!(
            "modifier chain for input code {key_code} produced {:?}, expected {:?}",
            event.value.input_type(),
            expected
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::data_asset_database::DataAssetDatabase;
    use crate::ecs::component::component_ref;
    use crate::ecs::{ComponentRegistry, ComponentWeak, Scene};
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::rc::Rc;

    #[crate::ecs::component]
    #[derive(crate::ecs::Component)]
    struct InputTestComponent {
        callback_count: Rc<Cell<usize>>,
    }

    impl crate::ecs::Component for InputTestComponent {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    fn no_op_callback(
        _component: &mut InputTestComponent,
        _scene: &Scene,
        _map: &str,
        _key_code: u32,
        _state: InputState,
        _value: InputValue,
    ) {
    }

    fn count_callback(
        component: &mut InputTestComponent,
        _scene: &Scene,
        _map: &str,
        _key_code: u32,
        _state: InputState,
        _value: InputValue,
    ) {
        component
            .callback_count
            .set(component.callback_count.get().saturating_add(1));
    }

    struct TriggerEveryOngoing;

    impl InputModifier for TriggerEveryOngoing {
        fn process(&mut self, input: &mut InputEvent) {
            if input.state == InputState::Ongoing {
                input.state = InputState::Triggered;
            }
        }
    }

    struct CancelFirstOngoing {
        canceled: bool,
    }

    impl InputModifier for CancelFirstOngoing {
        fn process(&mut self, input: &mut InputEvent) {
            if input.state == InputState::Ongoing && !self.canceled {
                self.canceled = true;
                input.state = InputState::Canceled;
                input.value = InputValue::Bool(false);
            }
        }
    }

    struct RewriteMouseState {
        observed_states: Rc<RefCell<Vec<InputState>>>,
    }

    impl InputModifier for RewriteMouseState {
        fn process(&mut self, input: &mut InputEvent) {
            self.observed_states.borrow_mut().push(input.state);
            input.state = InputState::Canceled;
        }
    }

    struct TriggerOnUpdate;

    impl InputModifier for TriggerOnUpdate {
        fn process(&mut self, _input: &mut InputEvent) {}

        fn update(&mut self, _delta_time: f32) -> Vec<InputEvent> {
            vec![InputEvent::new(
                InputState::Triggered,
                InputValue::Bool(true),
            )]
        }
    }

    fn test_binding(key_code: u32, modifier: Box<dyn InputModifier>) -> CompiledBinding {
        CompiledBinding {
            key_code,
            output_type: InputType::Bool,
            modifiers: vec![modifier],
            modifier_output_types: vec![InputType::Bool],
            subscriptions: Vec::new(),
            active: false,
            suppress_until_release: false,
        }
    }

    fn test_component() -> (
        Rc<RefCell<dyn Component>>,
        ComponentWeak<InputTestComponent>,
        Rc<Cell<usize>>,
    ) {
        let callback_count = Rc::new(Cell::new(0));
        let component = component_ref(Box::new(InputTestComponent {
            callback_count: Rc::clone(&callback_count),
            instance: crate::ecs::ComponentInstance::default(),
        }));
        let weak = component
            .borrow()
            .weak_component::<InputTestComponent>()
            .unwrap();
        (component, weak, callback_count)
    }

    fn subscribe_noop(
        input: &mut InputSystem,
        map: &str,
        binding: &str,
    ) -> Rc<RefCell<dyn Component>> {
        let (component, weak, _) = test_component();
        input
            .subscribe(map, binding, &weak, no_op_callback)
            .unwrap();
        component
    }

    fn load_input(yaml: &str) -> Result<(DataAssetDatabase, InputSystem)> {
        let database = DataAssetDatabase::from_documents(vec![(
            "Controls".to_owned(),
            serde_yaml::from_str(yaml)?,
            PathBuf::from("controls.data"),
        )])?;
        let asset = database
            .get::<Input>("Controls")
            .ok_or_else(|| anyhow!("input asset was not loaded"))?;
        let input = InputSystem::from_data_asset(&asset)?;
        Ok((database, input))
    }

    fn single_binding_input(
        map: &str,
        binding: &str,
        key_code: u32,
        input_type: &str,
        modifier: Option<(&str, &str)>,
    ) -> String {
        let modifier = modifier.map_or_else(
            || " []".to_owned(),
            |(type_name, data)| {
                format!("\n                - type: {type_name}\n                  data: {data}")
            },
        );
        format!(
            "type: Input\nname: Controls\ndata:\n  keymaps:\n    {map}:\n      type: KeyMap\n      data:\n        bindings:\n          {binding}:\n            type: KeyBinding\n            data:\n              key: {key_code}\n              input_type: {input_type}\n              modifiers:{modifier}\n"
        )
    }

    fn notifications(input: Vec<InputNotification>) -> Vec<(InputState, InputValue)> {
        input
            .into_iter()
            .map(|notification| (notification.event.state, notification.event.value))
            .collect()
    }

    fn states(input: Vec<InputNotification>) -> Vec<InputState> {
        input
            .into_iter()
            .map(|notification| notification.event.state)
            .collect()
    }

    #[test]
    fn raw_mouse_motion_is_routed_and_validated() {
        let yaml =
            single_binding_input("Gameplay", "look", input_code::MOUSE_MOVE, "Vector2", None);
        let (_database, mut input) = load_input(&yaml).unwrap();
        let _component = subscribe_noop(&mut input, "Gameplay", "look");
        assert_eq!(
            notifications(
                input
                    .consume(RawInput::MouseMotion(Vec2::new(4.0, -3.0)))
                    .unwrap()
            ),
            [(
                InputState::Ongoing,
                InputValue::Vector2(Vec2::new(4.0, -3.0))
            )]
        );
        assert!(
            input
                .consume(RawInput::MouseMotion(Vec2::ZERO))
                .unwrap()
                .is_empty()
        );
        assert!(
            input
                .consume(RawInput::MouseMotion(Vec2::new(f32::NAN, 1.0)))
                .err()
                .unwrap()
                .to_string()
                .contains("mouse motion must be finite")
        );
    }

    #[test]
    fn mouse_control_requires_a_window() {
        let mut input = InputSystem::default();
        assert!(
            input
                .set_mouse_grab(MouseGrabMode::Confined)
                .unwrap_err()
                .to_string()
                .contains("requires a window")
        );
        assert!(
            input
                .set_mouse_visible(false)
                .unwrap_err()
                .to_string()
                .contains("requires a window")
        );
        assert!(
            input
                .mouse_grab()
                .unwrap_err()
                .to_string()
                .contains("requires a window")
        );
    }

    #[test]
    fn keyboard_button_without_modifiers_has_started_ongoing_and_ended_states() {
        let yaml = single_binding_input("Gameplay", "jump", input_code::SPACE, "Bool", None);
        let (_database, mut input) = load_input(&yaml).unwrap();
        let _component = subscribe_noop(&mut input, "Gameplay", "jump");

        assert_eq!(
            states(
                input
                    .consume(RawInput::Button {
                        key_code: input_code::SPACE,
                        pressed: true,
                    })
                    .unwrap()
            ),
            [InputState::Started]
        );
        assert_eq!(states(input.update(0.016).unwrap()), [InputState::Ongoing]);
        assert_eq!(states(input.update(0.016).unwrap()), [InputState::Ongoing]);
        assert_eq!(
            states(
                input
                    .consume(RawInput::Button {
                        key_code: input_code::SPACE,
                        pressed: false,
                    })
                    .unwrap()
            ),
            [InputState::Ended]
        );
        assert!(input.update(0.016).unwrap().is_empty());
    }

    #[test]
    fn duplicate_button_events_do_not_restart_or_end_a_press() {
        let yaml = single_binding_input("Gameplay", "jump", input_code::SPACE, "Bool", None);
        let (_database, mut input) = load_input(&yaml).unwrap();
        let _component = subscribe_noop(&mut input, "Gameplay", "jump");
        let press = RawInput::Button {
            key_code: input_code::SPACE,
            pressed: true,
        };
        let release = RawInput::Button {
            key_code: input_code::SPACE,
            pressed: false,
        };

        assert_eq!(
            states(input.consume(press.clone()).unwrap()),
            [InputState::Started]
        );
        assert!(input.consume(press).unwrap().is_empty());
        assert_eq!(
            states(input.consume(release.clone()).unwrap()),
            [InputState::Ended]
        );
        assert!(input.consume(release).unwrap().is_empty());
    }

    #[test]
    fn focus_loss_cancels_keyboard_input_without_a_later_ended_state() {
        let yaml = single_binding_input("Gameplay", "jump", input_code::SPACE, "Bool", None);
        let (_database, mut input) = load_input(&yaml).unwrap();
        let _component = subscribe_noop(&mut input, "Gameplay", "jump");
        input
            .consume(RawInput::Button {
                key_code: input_code::SPACE,
                pressed: true,
            })
            .unwrap();
        assert_eq!(
            states(input.consume(RawInput::FocusLost).unwrap()),
            [InputState::Canceled]
        );
        assert!(input.update(0.016).unwrap().is_empty());
        assert!(
            input
                .consume(RawInput::Button {
                    key_code: input_code::SPACE,
                    pressed: false,
                })
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn ongoing_modifiers_can_trigger_again_on_each_frame() {
        let mut binding = test_binding(input_code::SPACE, Box::new(TriggerEveryOngoing));
        let mut event = InputEvent::new(InputState::Started, InputValue::Bool(true));
        assert!(binding.process(&mut event).unwrap());
        assert_eq!(event.state, InputState::Started);
        for _ in 0..2 {
            let mut event = InputEvent::new(InputState::Ongoing, InputValue::Bool(true));
            assert!(binding.process(&mut event).unwrap());
            assert_eq!(event.state, InputState::Triggered);
        }
        let mut event = InputEvent::new(InputState::Ended, InputValue::Bool(false));
        assert!(binding.process(&mut event).unwrap());
        assert_eq!(event.state, InputState::Ended);
    }

    #[test]
    fn modifier_cancellation_suppresses_later_ongoing_and_ended_states() {
        let mut binding = test_binding(
            input_code::SPACE,
            Box::new(CancelFirstOngoing { canceled: false }),
        );
        let mut event = InputEvent::new(InputState::Started, InputValue::Bool(true));
        assert!(binding.process(&mut event).unwrap());
        let mut event = InputEvent::new(InputState::Ongoing, InputValue::Bool(true));
        assert!(binding.process(&mut event).unwrap());
        assert_eq!(event.state, InputState::Canceled);
        let mut event = InputEvent::new(InputState::Ongoing, InputValue::Bool(true));
        assert!(!binding.process(&mut event).unwrap());
        let mut event = InputEvent::new(InputState::Ended, InputValue::Bool(false));
        assert!(!binding.process(&mut event).unwrap());
    }

    #[test]
    fn mouse_events_start_ongoing_and_modifiers_may_change_the_state() {
        let observed_states = Rc::new(RefCell::new(Vec::new()));
        let mut binding = test_binding(
            input_code::MOUSE_LEFT,
            Box::new(RewriteMouseState {
                observed_states: Rc::clone(&observed_states),
            }),
        );
        for state in [InputState::Started, InputState::Ended] {
            let mut event = InputEvent::new(state, InputValue::Bool(true));
            assert!(binding.process(&mut event).unwrap());
            assert_eq!(event.state, InputState::Canceled);
        }
        assert_eq!(
            *observed_states.borrow(),
            [InputState::Ongoing, InputState::Ongoing]
        );
    }

    #[test]
    fn mouse_modifier_updates_are_not_polled_without_raw_mouse_events() {
        let mut binding = test_binding(input_code::MOUSE_MOVE, Box::new(TriggerOnUpdate));
        assert!(binding.update(0.016).unwrap().is_empty());
    }

    #[test]
    fn mouse_buttons_are_ongoing_on_press_each_frame_and_release() {
        let yaml = single_binding_input("Gameplay", "fire", input_code::MOUSE_LEFT, "Bool", None);
        let (_database, mut input) = load_input(&yaml).unwrap();
        let _component = subscribe_noop(&mut input, "Gameplay", "fire");

        assert_eq!(
            notifications(
                input
                    .consume(RawInput::Button {
                        key_code: input_code::MOUSE_LEFT,
                        pressed: true,
                    })
                    .unwrap()
            ),
            [(InputState::Ongoing, InputValue::Bool(true))]
        );
        assert_eq!(
            notifications(input.update(0.016).unwrap()),
            [(InputState::Ongoing, InputValue::Bool(true))]
        );
        assert_eq!(
            notifications(
                input
                    .consume(RawInput::Button {
                        key_code: input_code::MOUSE_LEFT,
                        pressed: false,
                    })
                    .unwrap()
            ),
            [(InputState::Ongoing, InputValue::Bool(false))]
        );
    }

    #[test]
    fn mouse_button_focus_loss_is_ongoing_not_canceled() {
        let yaml = single_binding_input("Gameplay", "fire", input_code::MOUSE_LEFT, "Bool", None);
        let (_database, mut input) = load_input(&yaml).unwrap();
        let _component = subscribe_noop(&mut input, "Gameplay", "fire");
        input
            .consume(RawInput::Button {
                key_code: input_code::MOUSE_LEFT,
                pressed: true,
            })
            .unwrap();

        assert_eq!(
            notifications(input.consume(RawInput::FocusLost).unwrap()),
            [(InputState::Ongoing, InputValue::Bool(false))]
        );
        assert!(input.update(0.016).unwrap().is_empty());
    }

    #[test]
    fn scroll_emits_ongoing_signed_float_values() {
        let yaml = single_binding_input("Gameplay", "zoom", input_code::SCROLL, "Float", None);
        let (_database, mut input) = load_input(&yaml).unwrap();
        let _component = subscribe_noop(&mut input, "Gameplay", "zoom");

        assert_eq!(
            notifications(input.consume(RawInput::Scroll(-2.0)).unwrap()),
            [(InputState::Ongoing, InputValue::Float(-1.0))]
        );
        assert_eq!(
            notifications(input.consume(RawInput::Scroll(3.0)).unwrap()),
            [(InputState::Ongoing, InputValue::Float(1.0))]
        );
    }

    #[test]
    fn mouse_movement_does_not_end_or_cancel_when_it_stops_or_loses_focus() {
        let yaml =
            single_binding_input("Gameplay", "look", input_code::MOUSE_MOVE, "Vector2", None);
        let (_database, mut input) = load_input(&yaml).unwrap();
        let _component = subscribe_noop(&mut input, "Gameplay", "look");
        input
            .consume(RawInput::CursorMoved(Vec2::new(1.0, 2.0)))
            .unwrap();
        assert_eq!(
            notifications(
                input
                    .consume(RawInput::CursorMoved(Vec2::new(3.0, 7.0)))
                    .unwrap()
            ),
            [(
                InputState::Ongoing,
                InputValue::Vector2(Vec2::new(2.0, 5.0))
            )]
        );
        assert!(input.update(0.016).unwrap().is_empty());
        assert!(input.consume(RawInput::FocusLost).unwrap().is_empty());
    }

    #[test]
    fn active_map_switch_uses_index_and_reactivates_held_inputs() {
        let yaml = format!(
            "type: Input\nname: Controls\ndata:\n  keymaps:\n    Gameplay:\n      type: KeyMap\n      data:\n        bindings:\n          jump:\n            type: KeyBinding\n            data: {{ key: {}, input_type: Bool }}\n    Menu:\n      type: KeyMap\n      data:\n        bindings:\n          select:\n            type: KeyBinding\n            data: {{ key: {}, input_type: Bool }}\n",
            input_code::SPACE,
            input_code::SPACE
        );
        let (_database, mut input) = load_input(&yaml).unwrap();
        let _gameplay_component = subscribe_noop(&mut input, "Gameplay", "jump");
        let _menu_component = subscribe_noop(&mut input, "Menu", "select");
        assert_eq!(input.active_map(), Some("Gameplay"));
        input
            .consume(RawInput::Button {
                key_code: input_code::SPACE,
                pressed: true,
            })
            .unwrap();

        let switched = input.set_active_map("Menu").unwrap();
        assert_eq!(input.active_map(), Some("Menu"));
        assert_eq!(
            switched
                .iter()
                .map(|notification| (notification.map.as_str(), notification.event.state))
                .collect::<Vec<_>>(),
            [
                ("Gameplay", InputState::Canceled),
                ("Menu", InputState::Started),
            ]
        );
    }

    #[test]
    fn switching_maps_does_not_cancel_mouse_buttons() {
        let yaml = format!(
            "type: Input\nname: Controls\ndata:\n  keymaps:\n    Gameplay:\n      type: KeyMap\n      data:\n        bindings:\n          fire:\n            type: KeyBinding\n            data: {{ key: {}, input_type: Bool }}\n    Menu:\n      type: KeyMap\n      data:\n        bindings:\n          select:\n            type: KeyBinding\n            data: {{ key: {}, input_type: Bool }}\n",
            input_code::MOUSE_LEFT,
            input_code::MOUSE_LEFT
        );
        let (_database, mut input) = load_input(&yaml).unwrap();
        let _gameplay_component = subscribe_noop(&mut input, "Gameplay", "fire");
        let _menu_component = subscribe_noop(&mut input, "Menu", "select");
        input
            .consume(RawInput::Button {
                key_code: input_code::MOUSE_LEFT,
                pressed: true,
            })
            .unwrap();

        assert_eq!(
            notifications(input.set_active_map("Menu").unwrap()),
            [(InputState::Ongoing, InputValue::Bool(true))]
        );
    }

    #[test]
    fn rejects_invalid_modifier_chains_during_asset_deserialization() {
        let yaml = single_binding_input(
            "Gameplay",
            "invalid",
            input_code::SPACE,
            "Vector2",
            Some(("SwizzleX", "{}")),
        );
        let error = match load_input(&yaml) {
            Ok(_) => panic!("invalid modifier chain unexpectedly loaded"),
            Err(error) => format!("{error:#}"),
        };
        assert!(error.contains("expects"), "{error}");
    }

    #[test]
    fn expired_component_subscriptions_are_removed_automatically() {
        let yaml = single_binding_input("Gameplay", "jump", input_code::SPACE, "Bool", None);
        let (_database, mut input) = load_input(&yaml).unwrap();
        let (component, weak, _) = test_component();
        input
            .subscribe("Gameplay", "jump", &weak, no_op_callback)
            .unwrap();
        drop(component);
        assert!(
            input
                .consume(RawInput::Button {
                    key_code: input_code::SPACE,
                    pressed: true,
                })
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unsubscribe_can_remove_one_callback_or_all_for_a_component() {
        let yaml = single_binding_input("Gameplay", "jump", input_code::SPACE, "Bool", None);
        let (_database, mut input) = load_input(&yaml).unwrap();
        let (component, weak, _) = test_component();
        input
            .subscribe("Gameplay", "jump", &weak, no_op_callback)
            .unwrap();
        input
            .subscribe("Gameplay", "jump", &weak, count_callback)
            .unwrap();
        assert_eq!(input.unsubscribe_callback(&weak, no_op_callback), 1);
        assert_eq!(
            input
                .consume(RawInput::Button {
                    key_code: input_code::SPACE,
                    pressed: true,
                })
                .unwrap()
                .len(),
            1
        );
        assert_eq!(input.unsubscribe_component(&weak), 1);
        assert!(component.borrow().as_any().is::<InputTestComponent>());
    }

    #[test]
    fn scene_dispatches_callbacks_directly_to_the_component() {
        let yaml = single_binding_input("Gameplay", "jump", input_code::SPACE, "Bool", None);
        let (_database, input) = load_input(&yaml).unwrap();
        let mut scene =
            Scene::from_yaml("root:\n  name: Root\n", &ComponentRegistry::new()).unwrap();
        scene.configure_input(input).unwrap();
        scene.initialize().unwrap();

        let (component, component_weak, callback_count) = test_component();
        component_weak
            .with(|component| scene.subscribe_input("Gameplay", "jump", component, count_callback))
            .unwrap()
            .unwrap();

        scene
            .consume_input(RawInput::Button {
                key_code: input_code::SPACE,
                pressed: true,
            })
            .unwrap();
        assert_eq!(callback_count.get(), 1);
        drop(component);
    }
}
