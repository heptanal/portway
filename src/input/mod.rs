mod keymap;
#[cfg(target_os = "linux")]
mod linux;

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use anyhow::Result;
#[cfg(not(target_os = "linux"))]
use anyhow::anyhow;
use serde::Serialize;
use thiserror::Error;

use crate::{
    config::{BackendKind, Config},
    protocol::{KeyCode, MouseButton},
};

pub use keymap::{KeyStroke, map_text};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecordedEvent {
    PointerMove { dx: i32, dy: i32 },
    Scroll { dx: i32, dy: i32 },
    ButtonDown { button: MouseButton },
    ButtonUp { button: MouseButton },
    KeyDown { code: KeyCode },
    KeyUp { code: KeyCode },
    ReleaseAll,
}

#[derive(Debug, Clone)]
pub struct BackendStatus {
    pub name: String,
    pub available: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Error)]
pub enum InputError {
    #[error("input backend is unavailable: {0}")]
    Unavailable(String),
    #[error("unsupported text character {0:?}")]
    UnsupportedCharacter(char),
    #[error("input device operation failed: {0}")]
    Device(#[from] std::io::Error),
}

pub trait InputBackend: Send {
    fn status(&self) -> BackendStatus;
    fn move_pointer(&mut self, dx: i32, dy: i32) -> Result<(), InputError>;
    fn scroll(&mut self, dx: i32, dy: i32) -> Result<(), InputError>;
    fn button_down(&mut self, button: MouseButton) -> Result<(), InputError>;
    fn button_up(&mut self, button: MouseButton) -> Result<(), InputError>;
    fn key_down(&mut self, code: KeyCode) -> Result<(), InputError>;
    fn key_up(&mut self, code: KeyCode) -> Result<(), InputError>;
    fn is_key_down(&self, code: KeyCode) -> bool;
    fn release_all(&mut self) -> Result<(), InputError>;

    fn type_text(&mut self, text: &str) -> Result<(), InputError> {
        let strokes = map_text(text)?;
        for stroke in strokes {
            let shift_was_down = self.is_key_down(KeyCode::LeftShift);
            if stroke.shift && !shift_was_down {
                self.key_down(KeyCode::LeftShift)?;
            } else if !stroke.shift && shift_was_down {
                self.key_up(KeyCode::LeftShift)?;
            }
            self.key_down(stroke.code)?;
            self.key_up(stroke.code)?;
            if stroke.shift && !shift_was_down {
                self.key_up(KeyCode::LeftShift)?;
            } else if !stroke.shift && shift_was_down {
                self.key_down(KeyCode::LeftShift)?;
            }
        }
        Ok(())
    }
}

pub type SharedBackend = Arc<tokio::sync::Mutex<Box<dyn InputBackend>>>;

#[derive(Clone)]
pub struct RecordingHandle(Arc<Mutex<Vec<RecordedEvent>>>);

impl RecordingHandle {
    #[must_use]
    pub fn events(&self) -> Vec<RecordedEvent> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

pub struct RecordingBackend {
    events: RecordingHandle,
    keys: HashSet<KeyCode>,
    buttons: HashSet<MouseButton>,
}

impl RecordingBackend {
    #[must_use]
    pub fn new() -> (Self, RecordingHandle) {
        let events = RecordingHandle(Arc::new(Mutex::new(Vec::new())));
        (
            Self {
                events: events.clone(),
                keys: HashSet::new(),
                buttons: HashSet::new(),
            },
            events,
        )
    }

    fn record(&self, event: RecordedEvent) {
        self.events
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

impl Default for RecordingBackend {
    fn default() -> Self {
        Self::new().0
    }
}

impl InputBackend for RecordingBackend {
    fn status(&self) -> BackendStatus {
        BackendStatus {
            name: "mock".to_owned(),
            available: true,
            detail: Some("events are recorded and no host input is emitted".to_owned()),
        }
    }

    fn move_pointer(&mut self, dx: i32, dy: i32) -> Result<(), InputError> {
        self.record(RecordedEvent::PointerMove { dx, dy });
        Ok(())
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<(), InputError> {
        self.record(RecordedEvent::Scroll { dx, dy });
        Ok(())
    }

    fn button_down(&mut self, button: MouseButton) -> Result<(), InputError> {
        if self.buttons.insert(button) {
            self.record(RecordedEvent::ButtonDown { button });
        }
        Ok(())
    }

    fn button_up(&mut self, button: MouseButton) -> Result<(), InputError> {
        if self.buttons.remove(&button) {
            self.record(RecordedEvent::ButtonUp { button });
        }
        Ok(())
    }

    fn key_down(&mut self, code: KeyCode) -> Result<(), InputError> {
        if self.keys.insert(code) {
            self.record(RecordedEvent::KeyDown { code });
        }
        Ok(())
    }

    fn key_up(&mut self, code: KeyCode) -> Result<(), InputError> {
        if self.keys.remove(&code) {
            self.record(RecordedEvent::KeyUp { code });
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
        self.record(RecordedEvent::ReleaseAll);
        Ok(())
    }
}

pub struct UnavailableBackend {
    detail: String,
}

impl UnavailableBackend {
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    fn error(&self) -> InputError {
        InputError::Unavailable(self.detail.clone())
    }
}

impl InputBackend for UnavailableBackend {
    fn status(&self) -> BackendStatus {
        BackendStatus {
            name: "unavailable".to_owned(),
            available: false,
            detail: Some(self.detail.clone()),
        }
    }

    fn move_pointer(&mut self, _dx: i32, _dy: i32) -> Result<(), InputError> {
        Err(self.error())
    }
    fn scroll(&mut self, _dx: i32, _dy: i32) -> Result<(), InputError> {
        Err(self.error())
    }
    fn button_down(&mut self, _button: MouseButton) -> Result<(), InputError> {
        Err(self.error())
    }
    fn button_up(&mut self, _button: MouseButton) -> Result<(), InputError> {
        Err(self.error())
    }
    fn key_down(&mut self, _code: KeyCode) -> Result<(), InputError> {
        Err(self.error())
    }
    fn key_up(&mut self, _code: KeyCode) -> Result<(), InputError> {
        Err(self.error())
    }
    fn is_key_down(&self, _code: KeyCode) -> bool {
        false
    }
    fn release_all(&mut self) -> Result<(), InputError> {
        Ok(())
    }
}

pub fn create_backend(config: &Config) -> Result<Box<dyn InputBackend>> {
    match config.backend {
        BackendKind::Mock => Ok(Box::new(RecordingBackend::default())),
        BackendKind::Uinput => create_uinput(config),
        BackendKind::Auto => match create_uinput(config) {
            Ok(backend) => Ok(backend),
            Err(error) => Ok(Box::new(UnavailableBackend::new(error.to_string()))),
        },
    }
}

#[cfg(target_os = "linux")]
fn create_uinput(config: &Config) -> Result<Box<dyn InputBackend>> {
    linux::UinputBackend::new(&config.mouse_name, &config.keyboard_name)
        .map(|backend| Box::new(backend) as Box<dyn InputBackend>)
        .map_err(Into::into)
}

#[cfg(not(target_os = "linux"))]
fn create_uinput(_config: &Config) -> Result<Box<dyn InputBackend>> {
    Err(anyhow!("the uinput backend is available only on Linux"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_order_and_release_all() {
        let (mut backend, handle) = RecordingBackend::new();
        backend.key_down(KeyCode::LeftCtrl).unwrap();
        backend.button_down(MouseButton::Left).unwrap();
        backend.release_all().unwrap();
        let events = handle.events();
        assert_eq!(
            events[0],
            RecordedEvent::KeyDown {
                code: KeyCode::LeftCtrl
            }
        );
        assert!(events.contains(&RecordedEvent::ButtonUp {
            button: MouseButton::Left
        }));
        assert!(events.contains(&RecordedEvent::KeyUp {
            code: KeyCode::LeftCtrl
        }));
        assert_eq!(events.last(), Some(&RecordedEvent::ReleaseAll));
    }

    #[test]
    fn text_preserves_sticky_shift() {
        let (mut backend, handle) = RecordingBackend::new();
        backend.key_down(KeyCode::LeftShift).unwrap();
        backend.type_text("aA").unwrap();
        assert!(backend.is_key_down(KeyCode::LeftShift));

        let events = handle.events();
        assert_eq!(
            events.first(),
            Some(&RecordedEvent::KeyDown {
                code: KeyCode::LeftShift
            })
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| **event
                    == RecordedEvent::KeyDown {
                        code: KeyCode::LeftShift
                    })
                .count(),
            2
        );
        assert!(events.contains(&RecordedEvent::KeyUp {
            code: KeyCode::LeftShift
        }));
    }
}
