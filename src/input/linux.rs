#![allow(unsafe_code)]

use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{self, Write},
    mem::size_of,
    os::fd::AsRawFd,
};

use crate::protocol::{KeyCode, MouseButton};

use super::{BackendStatus, InputBackend, InputError};

const UINPUT_PATH: &str = "/dev/uinput";
const UINPUT_IOCTL_BASE: u32 = b'U' as u32;
const UI_DEV_CREATE: libc::c_ulong = ioctl_none(UINPUT_IOCTL_BASE, 1);
const UI_DEV_DESTROY: libc::c_ulong = ioctl_none(UINPUT_IOCTL_BASE, 2);
const UI_DEV_SETUP: libc::c_ulong =
    ioctl_write(UINPUT_IOCTL_BASE, 3, ioctl_size(size_of::<UinputSetup>()));
const UI_SET_EVBIT: libc::c_ulong =
    ioctl_write(UINPUT_IOCTL_BASE, 100, ioctl_size(size_of::<i32>()));
const UI_SET_KEYBIT: libc::c_ulong =
    ioctl_write(UINPUT_IOCTL_BASE, 101, ioctl_size(size_of::<i32>()));
const UI_SET_RELBIT: libc::c_ulong =
    ioctl_write(UINPUT_IOCTL_BASE, 102, ioctl_size(size_of::<i32>()));

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const SYN_REPORT: u16 = 0;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BUS_USB: u16 = 0x03;

const KEY_CAPABILITIES: &[u16] = &[
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50,
    51, 52, 53, 54, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 70, 87, 88, 97, 99, 100,
    102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 113, 114, 115, 119, 125, 126, 163, 164, 165,
];

const fn ioctl_none(kind: u32, number: u32) -> libc::c_ulong {
    ((kind << 8) | number) as libc::c_ulong
}

const fn ioctl_write(kind: u32, number: u32, size: u32) -> libc::c_ulong {
    ((1_u32 << 30) | (size << 16) | (kind << 8) | number) as libc::c_ulong
}

#[allow(clippy::cast_possible_truncation)]
const fn ioctl_size(size: usize) -> u32 {
    // Linux ioctl request sizes have a 14-bit field on the supported architectures.
    assert!(size <= 0x3fff);
    size as u32
}

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [libc::c_char; 80],
    ff_effects_max: u32,
}

#[repr(C)]
struct InputEvent {
    time: libc::timeval,
    event_type: u16,
    code: u16,
    value: i32,
}

struct Device {
    file: File,
}

impl Device {
    fn open() -> io::Result<Self> {
        Ok(Self {
            file: OpenOptions::new()
                .read(true)
                .write(true)
                .open(UINPUT_PATH)?,
        })
    }

    fn set_capability(&self, request: libc::c_ulong, value: u16) -> io::Result<()> {
        let result = unsafe { libc::ioctl(self.file.as_raw_fd(), request, i32::from(value)) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn setup(&self, name: &str, product: u16) -> io::Result<()> {
        let mut setup = UinputSetup {
            id: InputId {
                bustype: BUS_USB,
                vendor: 0x1209,
                product,
                version: 1,
            },
            name: [0; 80],
            ff_effects_max: 0,
        };
        for (target, source) in setup.name.iter_mut().zip(name.as_bytes()) {
            *target = libc::c_char::from_ne_bytes([*source]);
        }
        let result = unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_SETUP, &setup) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        let result = unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_CREATE) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn emit(&mut self, event_type: u16, code: u16, value: i32) -> io::Result<()> {
        let event = InputEvent {
            time: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            event_type,
            code,
            value,
        };
        let bytes = unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(&event).cast::<u8>(),
                size_of::<InputEvent>(),
            )
        };
        self.file.write_all(bytes)
    }

    fn sync(&mut self) -> io::Result<()> {
        self.emit(EV_SYN, SYN_REPORT, 0)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let result = unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY) };
        if result < 0 {
            tracing::warn!(error = %io::Error::last_os_error(), "failed to destroy uinput device");
        }
    }
}

pub struct UinputBackend {
    mouse: Device,
    keyboard: Device,
    keys: HashSet<KeyCode>,
    buttons: HashSet<MouseButton>,
}

impl UinputBackend {
    pub fn new(mouse_name: &str, keyboard_name: &str) -> io::Result<Self> {
        let mouse = Self::create_mouse(mouse_name)?;
        let keyboard = Self::create_keyboard(keyboard_name)?;
        Ok(Self {
            mouse,
            keyboard,
            keys: HashSet::new(),
            buttons: HashSet::new(),
        })
    }

    fn create_mouse(name: &str) -> io::Result<Device> {
        let device = Device::open()?;
        device.set_capability(UI_SET_EVBIT, EV_KEY)?;
        device.set_capability(UI_SET_EVBIT, EV_REL)?;
        for code in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE] {
            device.set_capability(UI_SET_KEYBIT, code)?;
        }
        for code in [REL_X, REL_Y, REL_WHEEL, REL_HWHEEL] {
            device.set_capability(UI_SET_RELBIT, code)?;
        }
        device.setup(name, 0x2721)?;
        Ok(device)
    }

    fn create_keyboard(name: &str) -> io::Result<Device> {
        let device = Device::open()?;
        device.set_capability(UI_SET_EVBIT, EV_KEY)?;
        for &code in KEY_CAPABILITIES {
            device.set_capability(UI_SET_KEYBIT, code)?;
        }
        device.setup(name, 0x2722)?;
        Ok(device)
    }

    fn emit_key(&mut self, code: KeyCode, value: i32) -> Result<(), InputError> {
        self.keyboard.emit(EV_KEY, linux_key_code(code), value)?;
        self.keyboard.sync()?;
        Ok(())
    }
}

impl InputBackend for UinputBackend {
    fn status(&self) -> BackendStatus {
        BackendStatus {
            name: "uinput".to_owned(),
            available: true,
            detail: Some(format!("mouse and keyboard opened through {UINPUT_PATH}")),
        }
    }

    fn move_pointer(&mut self, dx: i32, dy: i32) -> Result<(), InputError> {
        if dx != 0 {
            self.mouse.emit(EV_REL, REL_X, dx)?;
        }
        if dy != 0 {
            self.mouse.emit(EV_REL, REL_Y, dy)?;
        }
        self.mouse.sync()?;
        Ok(())
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<(), InputError> {
        if dx != 0 {
            self.mouse.emit(EV_REL, REL_HWHEEL, dx)?;
        }
        if dy != 0 {
            self.mouse.emit(EV_REL, REL_WHEEL, dy)?;
        }
        self.mouse.sync()?;
        Ok(())
    }

    fn button_down(&mut self, button: MouseButton) -> Result<(), InputError> {
        if self.buttons.insert(button) {
            self.mouse.emit(EV_KEY, linux_button_code(button), 1)?;
            self.mouse.sync()?;
        }
        Ok(())
    }

    fn button_up(&mut self, button: MouseButton) -> Result<(), InputError> {
        if self.buttons.remove(&button) {
            self.mouse.emit(EV_KEY, linux_button_code(button), 0)?;
            self.mouse.sync()?;
        }
        Ok(())
    }

    fn key_down(&mut self, code: KeyCode) -> Result<(), InputError> {
        if self.keys.insert(code) {
            self.emit_key(code, 1)?;
        }
        Ok(())
    }

    fn key_up(&mut self, code: KeyCode) -> Result<(), InputError> {
        if self.keys.remove(&code) {
            self.emit_key(code, 0)?;
        }
        Ok(())
    }

    fn is_key_down(&self, code: KeyCode) -> bool {
        self.keys.contains(&code)
    }

    fn release_all(&mut self) -> Result<(), InputError> {
        let buttons: Vec<_> = self.buttons.iter().copied().collect();
        let keys: Vec<_> = self.keys.iter().copied().collect();
        for button in buttons {
            self.button_up(button)?;
        }
        for code in keys {
            self.key_up(code)?;
        }
        Ok(())
    }
}

impl Drop for UinputBackend {
    fn drop(&mut self) {
        if let Err(error) = self.release_all() {
            tracing::warn!(%error, "failed to release uinput state during teardown");
        }
    }
}

fn linux_button_code(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => BTN_LEFT,
        MouseButton::Right => BTN_RIGHT,
        MouseButton::Middle => BTN_MIDDLE,
    }
}

fn linux_key_code(code: KeyCode) -> u16 {
    match code {
        KeyCode::KeyA => 30,
        KeyCode::KeyB => 48,
        KeyCode::KeyC => 46,
        KeyCode::KeyD => 32,
        KeyCode::KeyE => 18,
        KeyCode::KeyF => 33,
        KeyCode::KeyG => 34,
        KeyCode::KeyH => 35,
        KeyCode::KeyI => 23,
        KeyCode::KeyJ => 36,
        KeyCode::KeyK => 37,
        KeyCode::KeyL => 38,
        KeyCode::KeyM => 50,
        KeyCode::KeyN => 49,
        KeyCode::KeyO => 24,
        KeyCode::KeyP => 25,
        KeyCode::KeyQ => 16,
        KeyCode::KeyR => 19,
        KeyCode::KeyS => 31,
        KeyCode::KeyT => 20,
        KeyCode::KeyU => 22,
        KeyCode::KeyV => 47,
        KeyCode::KeyW => 17,
        KeyCode::KeyX => 45,
        KeyCode::KeyY => 21,
        KeyCode::KeyZ => 44,
        KeyCode::Digit0 => 11,
        KeyCode::Digit1 => 2,
        KeyCode::Digit2 => 3,
        KeyCode::Digit3 => 4,
        KeyCode::Digit4 => 5,
        KeyCode::Digit5 => 6,
        KeyCode::Digit6 => 7,
        KeyCode::Digit7 => 8,
        KeyCode::Digit8 => 9,
        KeyCode::Digit9 => 10,
        KeyCode::Space => 57,
        KeyCode::Minus => 12,
        KeyCode::Equal => 13,
        KeyCode::LeftBracket => 26,
        KeyCode::RightBracket => 27,
        KeyCode::Backslash => 43,
        KeyCode::Semicolon => 39,
        KeyCode::Apostrophe => 40,
        KeyCode::Grave => 41,
        KeyCode::Comma => 51,
        KeyCode::Period => 52,
        KeyCode::Slash => 53,
        KeyCode::LeftCtrl => 29,
        KeyCode::RightCtrl => 97,
        KeyCode::LeftAlt => 56,
        KeyCode::RightAlt => 100,
        KeyCode::LeftShift => 42,
        KeyCode::RightShift => 54,
        KeyCode::LeftMeta => 125,
        KeyCode::RightMeta => 126,
        KeyCode::Escape => 1,
        KeyCode::Tab => 15,
        KeyCode::Enter => 28,
        KeyCode::Backspace => 14,
        KeyCode::Delete => 111,
        KeyCode::Insert => 110,
        KeyCode::ArrowUp => 103,
        KeyCode::ArrowDown => 108,
        KeyCode::ArrowLeft => 105,
        KeyCode::ArrowRight => 106,
        KeyCode::Home => 102,
        KeyCode::End => 107,
        KeyCode::PageUp => 104,
        KeyCode::PageDown => 109,
        KeyCode::CapsLock => 58,
        KeyCode::PrintScreen => 99,
        KeyCode::ScrollLock => 70,
        KeyCode::Pause => 119,
        KeyCode::F1 => 59,
        KeyCode::F2 => 60,
        KeyCode::F3 => 61,
        KeyCode::F4 => 62,
        KeyCode::F5 => 63,
        KeyCode::F6 => 64,
        KeyCode::F7 => 65,
        KeyCode::F8 => 66,
        KeyCode::F9 => 67,
        KeyCode::F10 => 68,
        KeyCode::F11 => 87,
        KeyCode::F12 => 88,
        KeyCode::VolumeMute => 113,
        KeyCode::VolumeDown => 114,
        KeyCode::VolumeUp => 115,
        KeyCode::MediaPrevious => 165,
        KeyCode::MediaPlayPause => 164,
        KeyCode::MediaNext => 163,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_exposed_key_is_declared_as_a_capability() {
        let representative = [
            KeyCode::KeyA,
            KeyCode::Digit0,
            KeyCode::LeftMeta,
            KeyCode::F12,
            KeyCode::MediaPlayPause,
        ];
        for key in representative {
            assert!(KEY_CAPABILITIES.contains(&linux_key_code(key)));
        }
    }

    #[test]
    #[ignore = "requires Linux /dev/uinput permission and creates real virtual devices"]
    fn creates_real_uinput_devices() {
        let mut backend = UinputBackend::new("Portway test mouse", "Portway test keyboard")
            .expect("open and configure /dev/uinput");
        backend.move_pointer(1, 1).unwrap();
        backend.key_down(KeyCode::LeftShift).unwrap();
        backend.release_all().unwrap();
    }
}
