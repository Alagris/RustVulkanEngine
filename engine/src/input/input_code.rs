use winit::keyboard::KeyCode;

pub const SPACE: u32 = KeyCode::Space as u32;
pub const MOUSE_MOVE: u32 = 0x1_0000;
pub const SCROLL: u32 = 0x1_0001;
pub const MOUSE_LEFT: u32 = 0x1_0010;
pub const MOUSE_RIGHT: u32 = 0x1_0011;
pub const MOUSE_MIDDLE: u32 = 0x1_0012;
pub const MOUSE_BACK: u32 = 0x1_0013;
pub const MOUSE_FORWARD: u32 = 0x1_0014;
pub const MOUSE_OTHER_BASE: u32 = 0x1_0100;

pub fn is_mouse_code(key_code: u32) -> bool {
    key_code >= MOUSE_MOVE
}

pub fn keyboard(code: KeyCode) -> u32 {
    code as u32
}

pub fn mouse_other(button: u16) -> u32 {
    MOUSE_OTHER_BASE + u32::from(button)
}
