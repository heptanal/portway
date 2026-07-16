use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

use crate::config::{AuthMode, Config};

const TOKEN_BYTES: usize = 32;
const PAIRING_NONCE_BYTES: usize = 12;
const SESSION_BYTES: usize = 32;
const MAX_SESSIONS: usize = 32;
const MAX_USED_PAIRING_CODES: usize = 1_024;
const PAIRING_CODE_VERSION: &str = "p1";

#[derive(Clone)]
pub struct Authenticator {
    inner: Arc<AuthInner>,
}

struct AuthInner {
    setup_token: Option<Vec<u8>>,
    state: Mutex<EphemeralState>,
    pairing_code_ttl: Duration,
    session_ttl: Duration,
}

#[derive(Default)]
struct EphemeralState {
    used_pairing_codes: HashMap<String, Instant>,
    sessions: HashMap<String, Instant>,
}

pub struct AuthMaterial {
    pub authenticator: Authenticator,
    pub newly_created: bool,
    pub token_path: PathBuf,
}

impl Authenticator {
    #[must_use]
    pub fn disabled() -> Self {
        Self::new(None, Duration::from_mins(5), Duration::from_hours(12))
    }

    #[must_use]
    pub fn from_token(token: impl Into<Vec<u8>>) -> Self {
        Self::new(
            Some(token.into()),
            Duration::from_mins(5),
            Duration::from_hours(12),
        )
    }

    #[must_use]
    pub fn from_token_with_ttls(
        token: impl Into<Vec<u8>>,
        pairing_code_ttl: Duration,
        session_ttl: Duration,
    ) -> Self {
        Self::new(Some(token.into()), pairing_code_ttl, session_ttl)
    }

    fn new(
        setup_token: Option<Vec<u8>>,
        pairing_code_ttl: Duration,
        session_ttl: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(AuthInner {
                setup_token,
                state: Mutex::new(EphemeralState::default()),
                pairing_code_ttl,
                session_ttl,
            }),
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.inner.setup_token.is_some()
    }

    #[must_use]
    pub fn pairing_code_ttl_seconds(&self) -> u64 {
        self.inner.pairing_code_ttl.as_secs()
    }

    #[must_use]
    pub fn session_ttl_seconds(&self) -> u64 {
        self.inner.session_ttl.as_secs()
    }

    /// Create a new time-bounded pairing code that the server will accept once.
    pub fn issue_pairing_code(&self) -> Result<Option<String>> {
        self.inner
            .setup_token
            .as_deref()
            .map(|token| generate_pairing_code(token, self.inner.pairing_code_ttl))
            .transpose()
    }

    /// Exchange a single-use pairing code or the persistent setup token for a session.
    pub async fn exchange(&self, credential: &str) -> Result<Option<String>> {
        if !self.is_enabled() {
            return Ok(None);
        }

        let now = Instant::now();
        let setup_token = self.inner.setup_token.as_deref().expect("enabled auth");
        let setup_token_matches = self
            .inner
            .setup_token
            .as_deref()
            .is_some_and(|expected| constant_time_eq(expected, credential.as_bytes()));
        let pairing_code_lifetime =
            verify_pairing_code(setup_token, credential, self.inner.pairing_code_ttl);
        let mut state = self.inner.state.lock().await;
        state.used_pairing_codes.retain(|_, expiry| *expiry > now);
        let pairing_code_matches =
            pairing_code_lifetime.is_some() && !state.used_pairing_codes.contains_key(credential);
        if !setup_token_matches && !pairing_code_matches {
            return Ok(None);
        }
        if let Some(lifetime) = pairing_code_lifetime {
            if state.used_pairing_codes.len() >= MAX_USED_PAIRING_CODES
                && let Some(oldest) = state
                    .used_pairing_codes
                    .iter()
                    .min_by_key(|(_, expiry)| **expiry)
                    .map(|(code, _)| code.clone())
            {
                state.used_pairing_codes.remove(&oldest);
            }
            state
                .used_pairing_codes
                .insert(credential.to_owned(), now + lifetime);
        }

        let session = random_url_token(SESSION_BYTES)?;
        state.sessions.retain(|_, expiry| *expiry > now);
        if state.sessions.len() >= MAX_SESSIONS
            && let Some(oldest) = state
                .sessions
                .iter()
                .min_by_key(|(_, expiry)| **expiry)
                .map(|(token, _)| token.clone())
        {
            state.sessions.remove(&oldest);
        }
        state
            .sessions
            .insert(session.clone(), now + self.inner.session_ttl);
        Ok(Some(session))
    }

    pub async fn verify_session(&self, candidate: Option<&str>) -> bool {
        if !self.is_enabled() {
            return true;
        }
        let Some(candidate) = candidate else {
            return false;
        };
        let now = Instant::now();
        let mut state = self.inner.state.lock().await;
        state.sessions.retain(|_, expiry| *expiry > now);
        state.sessions.contains_key(candidate)
    }

    pub async fn revoke_session(&self, candidate: Option<&str>) {
        if let Some(candidate) = candidate {
            self.inner.state.lock().await.sessions.remove(candidate);
        }
    }
}

fn constant_time_eq(expected: &[u8], candidate: &[u8]) -> bool {
    expected.len() == candidate.len() && bool::from(expected.ct_eq(candidate))
}

fn random_url_token(bytes: usize) -> Result<String> {
    let mut random = vec![0_u8; bytes];
    getrandom::fill(&mut random).context("operating system random generator failed")?;
    Ok(URL_SAFE_NO_PAD.encode(random))
}

pub fn create_pairing_code(setup_token: &str, ttl_seconds: u64) -> Result<String> {
    generate_pairing_code(setup_token.as_bytes(), Duration::from_secs(ttl_seconds))
}

fn generate_pairing_code(setup_token: &[u8], ttl: Duration) -> Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?;
    let expires = now
        .as_secs()
        .checked_add(ttl.as_secs())
        .context("pairing-code expiry overflowed")?;
    let nonce = random_url_token(PAIRING_NONCE_BYTES)?;
    let payload = format!("{PAIRING_CODE_VERSION}.{expires}.{nonce}");
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, setup_token);
    let signature = ring::hmac::sign(&key, payload.as_bytes());
    Ok(format!(
        "{payload}.{}",
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    ))
}

fn verify_pairing_code(
    setup_token: &[u8],
    candidate: &str,
    maximum_ttl: Duration,
) -> Option<Duration> {
    let mut parts = candidate.split('.');
    let version = parts.next()?;
    let expiry_text = parts.next()?;
    let nonce_text = parts.next()?;
    let signature_text = parts.next()?;
    if parts.next().is_some() || version != PAIRING_CODE_VERSION {
        return None;
    }
    let nonce = URL_SAFE_NO_PAD.decode(nonce_text).ok()?;
    if nonce.len() != PAIRING_NONCE_BYTES {
        return None;
    }
    let signature = URL_SAFE_NO_PAD.decode(signature_text).ok()?;
    let payload = format!("{version}.{expiry_text}.{nonce_text}");
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, setup_token);
    ring::hmac::verify(&key, payload.as_bytes(), &signature).ok()?;

    let expiry = expiry_text.parse::<u64>().ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let remaining = expiry.checked_sub(now)?;
    if remaining == 0 || remaining > maximum_ttl.as_secs() {
        return None;
    }
    Some(Duration::from_secs(remaining))
}

pub fn load_or_create(config: &Config) -> Result<AuthMaterial> {
    if config.auth_mode == AuthMode::Disabled {
        return Ok(AuthMaterial {
            authenticator: Authenticator::disabled(),
            newly_created: false,
            token_path: config.token_file.clone(),
        });
    }

    let (token, newly_created) = load_or_create_token(&config.token_file)?;
    Ok(AuthMaterial {
        authenticator: Authenticator::from_token_with_ttls(
            token.as_bytes(),
            Duration::from_secs(config.pairing_code_ttl_seconds),
            Duration::from_secs(config.session_ttl_seconds),
        ),
        newly_created,
        token_path: config.token_file.clone(),
    })
}

pub fn read_existing_token(path: &Path) -> Result<String> {
    let token = fs::read_to_string(path)
        .with_context(|| format!("failed to read token file {}", path.display()))?;
    validate_token(token.trim())
}

fn load_or_create_token(path: &Path) -> Result<(String, bool)> {
    match read_existing_token(path) {
        Ok(token) => return Ok((token, false)),
        Err(error) if path.exists() => return Err(error),
        Err(_) => {}
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create token directory {}", parent.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to secure token directory {}", parent.display()))?;
    }

    let token = random_url_token(TOKEN_BYTES)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    match options.open(path) {
        Ok(mut file) => {
            writeln!(file, "{token}")
                .with_context(|| format!("failed to write token file {}", path.display()))?;
            file.sync_all()
                .with_context(|| format!("failed to sync token file {}", path.display()))?;
            Ok((token, true))
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            Ok((read_existing_token(path)?, false))
        }
        Err(error) => {
            Err(error).with_context(|| format!("failed to create token file {}", path.display()))
        }
    }
}

fn validate_token(token: &str) -> Result<String> {
    let decoded = URL_SAFE_NO_PAD
        .decode(token)
        .context("token file does not contain URL-safe base64")?;
    if decoded.len() != TOKEN_BYTES {
        bail!("token file must contain exactly 256 bits of random data");
    }
    Ok(token.to_owned())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn creates_reuses_and_secures_token() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state/token");

        let (first, created) = load_or_create_token(&path).unwrap();
        let (second, created_again) = load_or_create_token(&path).unwrap();

        assert!(created);
        assert!(!created_again);
        assert_eq!(first, second);
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[tokio::test]
    async fn pairing_codes_are_single_use_and_sessions_can_be_revoked() {
        let auth = Authenticator::from_token(b"setup-token".to_vec());
        let code = auth.issue_pairing_code().unwrap().unwrap();
        let session = auth.exchange(&code).await.unwrap().unwrap();

        assert!(auth.exchange(&code).await.unwrap().is_none());
        assert!(auth.verify_session(Some(&session)).await);
        assert!(!auth.verify_session(Some("wrong")).await);
        auth.revoke_session(Some(&session)).await;
        assert!(!auth.verify_session(Some(&session)).await);
    }

    #[tokio::test]
    async fn pairing_codes_reject_tampering_and_the_wrong_setup_token() {
        let auth = Authenticator::from_token(b"setup-token".to_vec());
        let mut code = auth.issue_pairing_code().unwrap().unwrap();
        code.push('x');

        assert!(auth.exchange(&code).await.unwrap().is_none());

        let other = Authenticator::from_token(b"other-token".to_vec());
        let valid_for_other = other.issue_pairing_code().unwrap().unwrap();
        assert!(auth.exchange(&valid_for_other).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn persistent_setup_token_is_accepted_but_not_a_session() {
        let auth = Authenticator::from_token(b"setup-token".to_vec());
        let session = auth.exchange("setup-token").await.unwrap().unwrap();

        assert!(!auth.verify_session(Some("setup-token")).await);
        assert!(auth.verify_session(Some(&session)).await);
    }

    #[tokio::test]
    async fn sessions_expire() {
        let auth = Authenticator::from_token_with_ttls(
            b"setup-token".to_vec(),
            Duration::from_millis(1),
            Duration::from_millis(1),
        );
        let session = auth.exchange("setup-token").await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(!auth.verify_session(Some(&session)).await);
    }
}
