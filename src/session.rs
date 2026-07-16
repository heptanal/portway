use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use crate::{
    input::{InputError, SharedBackend},
    protocol::{
        ClientEnvelope, ClientEvent, KeyCode, KeyState, MAX_POINTER_DELTA, MouseButton,
        ProtocolError,
    },
};

pub const HEARTBEAT_TIMEOUT_MS: u64 = 15_000;
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_millis(HEARTBEAT_TIMEOUT_MS);
const RATE_PER_SECOND: f64 = 250.0;
const RATE_BURST: f64 = 500.0;

pub struct SessionState {
    held_keys: HashSet<KeyCode>,
    held_buttons: HashSet<MouseButton>,
    last_sequence: Option<u64>,
    pointer_sensitivity: f64,
}

impl SessionState {
    #[must_use]
    pub fn new(pointer_sensitivity: f64) -> Self {
        Self {
            held_keys: HashSet::new(),
            held_buttons: HashSet::new(),
            last_sequence: None,
            pointer_sensitivity,
        }
    }

    pub async fn process(
        &mut self,
        envelope: ClientEnvelope,
        backend: &SharedBackend,
    ) -> Result<SessionOutcome, SessionError> {
        envelope.validate()?;
        if self
            .last_sequence
            .is_some_and(|previous| envelope.seq <= previous)
        {
            return Err(SessionError::Sequence);
        }
        self.last_sequence = Some(envelope.seq);
        let mut input = backend.lock().await;

        match envelope.event {
            ClientEvent::PointerMove { dx, dy } => {
                let dx = scale_pointer(dx, self.pointer_sensitivity);
                let dy = scale_pointer(dy, self.pointer_sensitivity);
                input.move_pointer(dx, dy)?;
            }
            ClientEvent::Scroll { dx, dy } => input.scroll(dx, dy)?,
            ClientEvent::PointerButton { button, state } => match state {
                KeyState::Down if !self.held_buttons.contains(&button) => {
                    input.button_down(button)?;
                    self.held_buttons.insert(button);
                }
                KeyState::Up if self.held_buttons.contains(&button) => {
                    input.button_up(button)?;
                    self.held_buttons.remove(&button);
                }
                KeyState::Down | KeyState::Up => {}
            },
            ClientEvent::Key { code, state } => match state {
                KeyState::Down if !self.held_keys.contains(&code) => {
                    input.key_down(code)?;
                    self.held_keys.insert(code);
                }
                KeyState::Up if self.held_keys.contains(&code) => {
                    input.key_up(code)?;
                    self.held_keys.remove(&code);
                }
                KeyState::Down | KeyState::Up => {}
            },
            ClientEvent::TextInput { text } => input.type_text(&text)?,
            ClientEvent::ReleaseAll | ClientEvent::ClientStateReset => {
                input.release_all()?;
                self.held_buttons.clear();
                self.held_keys.clear();
            }
            ClientEvent::Heartbeat => return Ok(SessionOutcome::Heartbeat(envelope.seq)),
        }
        Ok(SessionOutcome::Applied)
    }

    pub async fn cleanup(&mut self, backend: &SharedBackend) -> Result<(), InputError> {
        let mut input = backend.lock().await;
        let mut first_error = None;

        for button in self.held_buttons.drain() {
            if let Err(error) = input.button_up(button) {
                first_error.get_or_insert(error);
            }
        }
        for code in self.held_keys.drain() {
            if let Err(error) = input.key_up(code) {
                first_error.get_or_insert(error);
            }
        }

        first_error.map_or(Ok(()), Err)
    }
}

#[allow(clippy::cast_possible_truncation)]
fn scale_pointer(value: i32, sensitivity: f64) -> i32 {
    ((f64::from(value) * sensitivity).round() as i32)
        .clamp(-MAX_POINTER_DELTA * 5, MAX_POINTER_DELTA * 5)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOutcome {
    Applied,
    Heartbeat(u64),
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("message sequence must increase")]
    Sequence,
    #[error(transparent)]
    Input(#[from] InputError),
}

pub struct RateLimiter {
    tokens: f64,
    last_refill: Instant,
    rate: f64,
    burst: f64,
}

impl RateLimiter {
    #[must_use]
    pub fn standard() -> Self {
        Self::new(RATE_PER_SECOND, RATE_BURST)
    }

    #[must_use]
    pub fn new(rate: f64, burst: f64) -> Self {
        Self {
            tokens: burst,
            last_refill: Instant::now(),
            rate,
            burst,
        }
    }

    pub fn allow_at(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last_refill);
        self.tokens = (self.tokens + elapsed.as_secs_f64() * self.rate).min(self.burst);
        self.last_refill = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        input::{RecordedEvent, RecordingBackend},
        protocol::PROTOCOL_VERSION,
    };

    use super::*;

    fn envelope(seq: u64, event: ClientEvent) -> ClientEnvelope {
        ClientEnvelope {
            v: PROTOCOL_VERSION,
            seq,
            event,
        }
    }

    #[tokio::test]
    async fn applies_motion_scroll_and_ordered_press_release() {
        let (recording, handle) = RecordingBackend::new();
        let backend: SharedBackend = Arc::new(tokio::sync::Mutex::new(Box::new(recording)));
        let mut session = SessionState::new(1.5);

        session
            .process(
                envelope(1, ClientEvent::PointerMove { dx: 4, dy: -2 }),
                &backend,
            )
            .await
            .unwrap();
        session
            .process(envelope(2, ClientEvent::Scroll { dx: 1, dy: -3 }), &backend)
            .await
            .unwrap();
        session
            .process(
                envelope(
                    3,
                    ClientEvent::Key {
                        code: KeyCode::LeftCtrl,
                        state: KeyState::Down,
                    },
                ),
                &backend,
            )
            .await
            .unwrap();
        session
            .process(
                envelope(
                    4,
                    ClientEvent::Key {
                        code: KeyCode::KeyC,
                        state: KeyState::Down,
                    },
                ),
                &backend,
            )
            .await
            .unwrap();
        session
            .process(
                envelope(
                    5,
                    ClientEvent::Key {
                        code: KeyCode::KeyC,
                        state: KeyState::Up,
                    },
                ),
                &backend,
            )
            .await
            .unwrap();

        assert_eq!(
            handle.events(),
            vec![
                RecordedEvent::PointerMove { dx: 6, dy: -3 },
                RecordedEvent::Scroll { dx: 1, dy: -3 },
                RecordedEvent::KeyDown {
                    code: KeyCode::LeftCtrl
                },
                RecordedEvent::KeyDown {
                    code: KeyCode::KeyC
                },
                RecordedEvent::KeyUp {
                    code: KeyCode::KeyC
                },
            ]
        );
    }

    #[tokio::test]
    async fn cleanup_releases_held_state() {
        let (recording, handle) = RecordingBackend::new();
        let backend: SharedBackend = Arc::new(tokio::sync::Mutex::new(Box::new(recording)));
        let mut session = SessionState::new(1.0);
        session
            .process(
                envelope(
                    1,
                    ClientEvent::PointerButton {
                        button: MouseButton::Left,
                        state: KeyState::Down,
                    },
                ),
                &backend,
            )
            .await
            .unwrap();
        session
            .process(
                envelope(
                    2,
                    ClientEvent::Key {
                        code: KeyCode::LeftAlt,
                        state: KeyState::Down,
                    },
                ),
                &backend,
            )
            .await
            .unwrap();

        session.cleanup(&backend).await.unwrap();

        let events = handle.events();
        assert!(events.contains(&RecordedEvent::ButtonUp {
            button: MouseButton::Left
        }));
        assert!(events.contains(&RecordedEvent::KeyUp {
            code: KeyCode::LeftAlt
        }));
    }

    #[tokio::test]
    async fn rejects_replayed_sequence() {
        let backend: SharedBackend = Arc::new(tokio::sync::Mutex::new(Box::new(
            RecordingBackend::default(),
        )));
        let mut session = SessionState::new(1.0);
        session
            .process(envelope(2, ClientEvent::Heartbeat), &backend)
            .await
            .unwrap();
        assert!(matches!(
            session
                .process(envelope(2, ClientEvent::Heartbeat), &backend)
                .await,
            Err(SessionError::Sequence)
        ));
    }

    #[test]
    fn rate_limiter_exhausts_and_refills() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(10.0, 2.0);
        assert!(limiter.allow_at(start));
        assert!(limiter.allow_at(start));
        assert!(!limiter.allow_at(start));
        assert!(limiter.allow_at(start + Duration::from_millis(100)));
    }
}
