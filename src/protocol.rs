use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_MESSAGE_BYTES: usize = 4096;
pub const MAX_TEXT_CHARS: usize = 128;
pub const MAX_POINTER_DELTA: i32 = 2048;
pub const MAX_SCROLL_DELTA: i32 = 120;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClientEnvelope {
    pub v: u8,
    pub seq: u64,
    pub event: ClientEvent,
}

impl ClientEnvelope {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.v != PROTOCOL_VERSION {
            return Err(ProtocolError::Version(self.v));
        }
        match &self.event {
            ClientEvent::PointerMove { dx, dy } => {
                validate_range(*dx, MAX_POINTER_DELTA, "pointer dx")?;
                validate_range(*dy, MAX_POINTER_DELTA, "pointer dy")?;
            }
            ClientEvent::Scroll { dx, dy } => {
                validate_range(*dx, MAX_SCROLL_DELTA, "scroll dx")?;
                validate_range(*dy, MAX_SCROLL_DELTA, "scroll dy")?;
            }
            ClientEvent::TextInput { text } => {
                if text.chars().count() > MAX_TEXT_CHARS {
                    return Err(ProtocolError::TextTooLong);
                }
                if text.chars().any(|character| {
                    !(character == '\n'
                        || character == '\t'
                        || character == ' '
                        || character.is_ascii_graphic())
                }) {
                    return Err(ProtocolError::UnsupportedText);
                }
            }
            ClientEvent::PointerButton { .. }
            | ClientEvent::Key { .. }
            | ClientEvent::ReleaseAll
            | ClientEvent::Heartbeat
            | ClientEvent::ClientStateReset => {}
        }
        Ok(())
    }
}

fn validate_range(value: i32, maximum: i32, field: &'static str) -> Result<(), ProtocolError> {
    if (-maximum..=maximum).contains(&value) {
        Ok(())
    } else {
        Err(ProtocolError::OutOfRange(field))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientEvent {
    PointerMove {
        dx: i32,
        dy: i32,
    },
    PointerButton {
        button: MouseButton,
        state: KeyState,
    },
    Scroll {
        dx: i32,
        dy: i32,
    },
    Key {
        code: KeyCode,
        state: KeyState,
    },
    TextInput {
        text: String,
    },
    ReleaseAll,
    Heartbeat,
    ClientStateReset,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum KeyCode {
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Space,
    Minus,
    Equal,
    LeftBracket,
    RightBracket,
    Backslash,
    Semicolon,
    Apostrophe,
    Grave,
    Comma,
    Period,
    Slash,
    LeftCtrl,
    RightCtrl,
    LeftAlt,
    RightAlt,
    LeftShift,
    RightShift,
    LeftMeta,
    RightMeta,
    Escape,
    Tab,
    Enter,
    Backspace,
    Delete,
    Insert,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    CapsLock,
    PrintScreen,
    ScrollLock,
    Pause,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    VolumeMute,
    VolumeDown,
    VolumeUp,
    MediaPrevious,
    MediaPlayPause,
    MediaNext,
}

impl KeyCode {
    #[must_use]
    pub fn is_modifier(self) -> bool {
        matches!(
            self,
            Self::LeftCtrl
                | Self::RightCtrl
                | Self::LeftAlt
                | Self::RightAlt
                | Self::LeftShift
                | Self::RightShift
                | Self::LeftMeta
                | Self::RightMeta
        )
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Ready {
        v: u8,
        backend: String,
        input_available: bool,
        pointer_sensitivity: f64,
        heartbeat_timeout_ms: u64,
    },
    Pong {
        v: u8,
        seq: u64,
    },
    Error {
        v: u8,
        code: &'static str,
        message: String,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("unsupported protocol version {0}")]
    Version(u8),
    #[error("{0} is outside the permitted range")]
    OutOfRange(&'static str),
    #[error("text input is too long")]
    TextTooLong,
    #[error("text input contains characters unsupported by the US-ASCII input path")]
    UnsupportedText,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_pointer_event() {
        let json = r#"{"v":1,"seq":9,"event":{"type":"pointer_move","dx":4,"dy":-7}}"#;
        let message: ClientEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(message.event, ClientEvent::PointerMove { dx: 4, dy: -7 });
        assert!(message.validate().is_ok());
        assert_eq!(
            serde_json::from_str::<ClientEnvelope>(&serde_json::to_string(&message).unwrap())
                .unwrap(),
            message
        );
    }

    #[test]
    fn rejects_unknown_fields_and_events() {
        let extra = r#"{"v":1,"seq":1,"extra":true,"event":{"type":"heartbeat"}}"#;
        let unknown = r#"{"v":1,"seq":1,"event":{"type":"run_shell"}}"#;
        assert!(serde_json::from_str::<ClientEnvelope>(extra).is_err());
        assert!(serde_json::from_str::<ClientEnvelope>(unknown).is_err());
    }

    #[test]
    fn validates_bounds_and_text() {
        let movement = ClientEnvelope {
            v: 1,
            seq: 1,
            event: ClientEvent::PointerMove { dx: 4096, dy: 0 },
        };
        assert_eq!(
            movement.validate(),
            Err(ProtocolError::OutOfRange("pointer dx"))
        );

        let unicode = ClientEnvelope {
            v: 1,
            seq: 2,
            event: ClientEvent::TextInput { text: "é".into() },
        };
        assert_eq!(unicode.validate(), Err(ProtocolError::UnsupportedText));
    }
}
