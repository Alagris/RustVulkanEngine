use anyhow::{Context, Result, anyhow};
use std::rc::Rc;
use winit::window::{CursorGrabMode, Window};

/// Desired cursor grab behavior while the game window has focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseGrabMode {
    Free,
    #[default]
    Confined,
    Captured,
}

trait MouseWindow {
    fn set_grab(&self, mode: CursorGrabMode) -> Result<()>;
    fn set_visible(&self, visible: bool);
    fn has_focus(&self) -> bool;
}

impl MouseWindow for Window {
    fn set_grab(&self, mode: CursorGrabMode) -> Result<()> {
        self.set_cursor_grab(mode).map_err(Into::into)
    }

    fn set_visible(&self, visible: bool) {
        self.set_cursor_visible(visible);
    }

    fn has_focus(&self) -> bool {
        Window::has_focus(self)
    }
}

pub(crate) struct MouseControl {
    window: Rc<dyn MouseWindow>,
    mode: MouseGrabMode,
    visible: bool,
    focused: bool,
}

impl MouseControl {
    pub(crate) fn new(window: Rc<Window>) -> Result<Self> {
        Self::with_window(window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct MockWindow {
        focused: Cell<bool>,
        fail_locked: Cell<bool>,
        fail_confined: Cell<bool>,
        grabs: RefCell<Vec<CursorGrabMode>>,
        visibility: RefCell<Vec<bool>>,
    }

    impl MouseWindow for MockWindow {
        fn set_grab(&self, mode: CursorGrabMode) -> Result<()> {
            self.grabs.borrow_mut().push(mode);
            if (mode == CursorGrabMode::Locked && self.fail_locked.get())
                || (mode == CursorGrabMode::Confined && self.fail_confined.get())
            {
                anyhow::bail!("mock {mode:?} failure");
            }
            Ok(())
        }

        fn set_visible(&self, visible: bool) {
            self.visibility.borrow_mut().push(visible);
        }

        fn has_focus(&self) -> bool {
            self.focused.get()
        }
    }

    #[test]
    fn default_grab_and_focus_restore_desired_settings() {
        let window = Rc::new(MockWindow::default());
        window.focused.set(true);
        let mut mouse = MouseControl::with_window(window.clone()).unwrap();
        assert_eq!(*window.grabs.borrow(), [CursorGrabMode::Confined]);
        assert_eq!(*window.visibility.borrow(), [false]);
        assert!(mouse.uses_raw_motion());

        mouse.set_focus(false).unwrap();
        assert_eq!(window.grabs.borrow().last(), Some(&CursorGrabMode::None));
        assert_eq!(window.visibility.borrow().last(), Some(&true));
        assert!(!mouse.uses_raw_motion());
        mouse.set_mode(MouseGrabMode::Free).unwrap();
        mouse.set_visible(true);
        mouse.set_focus(true).unwrap();
        assert_eq!(window.grabs.borrow().last(), Some(&CursorGrabMode::None));
        assert_eq!(window.visibility.borrow().last(), Some(&true));
        assert!(!mouse.uses_raw_motion());
    }

    #[test]
    fn unfocused_window_defers_grab_and_capture_falls_back() {
        let window = Rc::new(MockWindow::default());
        window.fail_locked.set(true);
        let mut mouse = MouseControl::with_window(window.clone()).unwrap();
        mouse.set_mode(MouseGrabMode::Captured).unwrap();
        assert!(window.grabs.borrow().is_empty());
        assert_eq!(*window.visibility.borrow(), [true]);
        mouse.set_focus(true).unwrap();
        assert_eq!(
            *window.grabs.borrow(),
            [CursorGrabMode::Locked, CursorGrabMode::Confined]
        );
        assert_eq!(mouse.mode(), MouseGrabMode::Captured);
        assert!(mouse.uses_raw_motion());
    }

    #[test]
    fn capture_failure_reports_both_errors_and_preserves_mode() {
        let window = Rc::new(MockWindow::default());
        window.focused.set(true);
        let mut mouse = MouseControl::with_window(window.clone()).unwrap();
        window.fail_locked.set(true);
        window.fail_confined.set(true);
        let error = mouse
            .set_mode(MouseGrabMode::Captured)
            .unwrap_err()
            .to_string();
        assert!(error.contains("mock Locked failure"));
        assert!(error.contains("mock Confined failure"));
        assert_eq!(mouse.mode(), MouseGrabMode::Confined);
    }
}

impl MouseControl {
    fn with_window(window: Rc<dyn MouseWindow>) -> Result<Self> {
        let focused = window.has_focus();
        let control = Self {
            window,
            mode: MouseGrabMode::default(),
            visible: false,
            focused,
        };
        if focused {
            control.apply_grab(control.mode)?;
        }
        control.window.set_visible(!focused);
        Ok(control)
    }

    fn apply_grab(&self, mode: MouseGrabMode) -> Result<()> {
        match mode {
            MouseGrabMode::Free => self
                .window
                .set_grab(CursorGrabMode::None)
                .context("failed to release the mouse"),
            MouseGrabMode::Confined => self
                .window
                .set_grab(CursorGrabMode::Confined)
                .context("failed to confine the mouse"),
            MouseGrabMode::Captured => match self.window.set_grab(CursorGrabMode::Locked) {
                Ok(()) => Ok(()),
                Err(locked_error) => {
                    self.window.set_grab(CursorGrabMode::Confined).map_err(|confined_error| {
                        anyhow!("failed to capture the mouse: locking failed: {locked_error:#}; confinement failed: {confined_error:#}")
                    })?;
                    log::info!("Cursor locking is unavailable; using confined raw mouse input");
                    Ok(())
                }
            },
        }
    }

    pub(crate) fn set_mode(&mut self, mode: MouseGrabMode) -> Result<()> {
        if self.focused {
            self.apply_grab(mode)?;
        }
        self.mode = mode;
        Ok(())
    }

    pub(crate) fn mode(&self) -> MouseGrabMode {
        self.mode
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if self.focused {
            self.window.set_visible(visible);
        }
    }

    pub(crate) fn set_focus(&mut self, focused: bool) -> Result<()> {
        if focused == self.focused {
            return Ok(());
        }
        if focused {
            self.apply_grab(self.mode)?;
            self.window.set_visible(self.visible);
        } else {
            let result = self.apply_grab(MouseGrabMode::Free);
            self.window.set_visible(true);
            self.focused = false;
            result?;
        }
        self.focused = focused;
        Ok(())
    }

    pub(crate) fn uses_raw_motion(&self) -> bool {
        self.focused && self.mode != MouseGrabMode::Free
    }
}
