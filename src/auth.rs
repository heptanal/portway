use std::{
    collections::HashMap,
    ffi::OsString,
    fs::{self, File, OpenOptions},
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
const SESSION_BYTES: usize = 32;
const MAX_SESSIONS: usize = 32;
const PAIRING_CODE_MODULUS: u32 = 1_000_000;
const PAIRING_CODE_VERSION: &str = "p2";

#[derive(Clone)]
pub struct Authenticator {
    inner: Arc<AuthInner>,
}

struct AuthInner {
    setup_token: Option<Vec<u8>>,
    pairing_code_path: Option<PathBuf>,
    state: Mutex<EphemeralState>,
    pairing_code_ttl: Duration,
    session_ttl: Duration,
}

#[derive(Default)]
struct EphemeralState {
    sessions: HashMap<String, Instant>,
}

pub struct AuthMaterial {
    pub authenticator: Authenticator,
    pub newly_created: bool,
    pub token_path: PathBuf,
    pub pairing_code_path: PathBuf,
}

impl Authenticator {
    #[must_use]
    pub fn disabled() -> Self {
        Self::new(None, None, Duration::from_mins(5), Duration::from_hours(12))
    }

    #[must_use]
    pub fn from_token(token: impl Into<Vec<u8>>, pairing_code_path: PathBuf) -> Self {
        Self::new(
            Some(token.into()),
            Some(pairing_code_path),
            Duration::from_mins(5),
            Duration::from_hours(12),
        )
    }

    #[must_use]
    pub fn from_token_with_ttls(
        token: impl Into<Vec<u8>>,
        pairing_code_path: PathBuf,
        pairing_code_ttl: Duration,
        session_ttl: Duration,
    ) -> Self {
        Self::new(
            Some(token.into()),
            Some(pairing_code_path),
            pairing_code_ttl,
            session_ttl,
        )
    }

    fn new(
        setup_token: Option<Vec<u8>>,
        pairing_code_path: Option<PathBuf>,
        pairing_code_ttl: Duration,
        session_ttl: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(AuthInner {
                setup_token,
                pairing_code_path,
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
        match (
            self.inner.setup_token.as_deref(),
            self.inner.pairing_code_path.as_deref(),
        ) {
            (Some(token), Some(path)) => {
                issue_pairing_code(token, path, self.inner.pairing_code_ttl).map(Some)
            }
            (None, None) => Ok(None),
            _ => unreachable!("enabled authentication requires a pairing-code path"),
        }
    }

    /// Exchange a single-use pairing code or the persistent setup token for a session.
    pub async fn exchange(&self, credential: &str) -> Result<Option<String>> {
        if !self.is_enabled() {
            return Ok(None);
        }

        let setup_token = self.inner.setup_token.as_deref().expect("enabled auth");
        let setup_token_matches = self
            .inner
            .setup_token
            .as_deref()
            .is_some_and(|expected| constant_time_eq(expected, credential.as_bytes()));
        let mut state = self.inner.state.lock().await;
        let pairing_code_matches = if setup_token_matches {
            false
        } else {
            consume_pairing_code(
                setup_token,
                self.inner
                    .pairing_code_path
                    .as_deref()
                    .expect("enabled authentication requires a pairing-code path"),
                credential,
                self.inner.pairing_code_ttl,
            )?
        };
        if !setup_token_matches && !pairing_code_matches {
            return Ok(None);
        }

        let now = Instant::now();
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

#[must_use]
pub fn pairing_code_path(token_path: &Path) -> PathBuf {
    let mut path = token_path.as_os_str().to_os_string();
    path.push(".pairing");
    PathBuf::from(path)
}

pub fn create_pairing_code(setup_token: &str, path: &Path, ttl_seconds: u64) -> Result<String> {
    issue_pairing_code(
        setup_token.as_bytes(),
        path,
        Duration::from_secs(ttl_seconds),
    )
}

fn issue_pairing_code(setup_token: &[u8], path: &Path, ttl: Duration) -> Result<String> {
    with_pairing_lock(path, || {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before the Unix epoch")?;
        let expires = now
            .as_secs()
            .checked_add(ttl.as_secs())
            .context("pairing-code expiry overflowed")?;
        let code = random_pairing_code()?;
        let payload = pairing_code_payload(expires, &code);
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, setup_token);
        let signature = ring::hmac::sign(&key, payload.as_bytes());
        let record = format!(
            "{PAIRING_CODE_VERSION}.{expires}.{}\n",
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        );
        write_pairing_record(path, record.as_bytes())?;
        Ok(code)
    })
}

fn random_pairing_code() -> Result<String> {
    let unbiased_limit = u32::MAX - (u32::MAX % PAIRING_CODE_MODULUS);
    loop {
        let mut random = [0_u8; 4];
        getrandom::fill(&mut random).context("operating system random generator failed")?;
        let value = u32::from_le_bytes(random);
        if value < unbiased_limit {
            return Ok(format!("{:06}", value % PAIRING_CODE_MODULUS));
        }
    }
}

fn pairing_code_payload(expires: u64, code: &str) -> String {
    format!("{PAIRING_CODE_VERSION}.{expires}.{code}")
}

fn write_pairing_record(path: &Path, record: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut last_collision = None;

    for _ in 0..8 {
        let temporary = temporary_pairing_path(path)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                last_collision = Some(error);
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create temporary pairing-code file in {}",
                        parent.display()
                    )
                });
            }
        };

        let write_result = (|| -> Result<()> {
            file.write_all(record).with_context(|| {
                format!(
                    "failed to write temporary pairing-code file {}",
                    temporary.display()
                )
            })?;
            file.sync_all().with_context(|| {
                format!(
                    "failed to sync temporary pairing-code file {}",
                    temporary.display()
                )
            })?;
            fs::rename(&temporary, path).with_context(|| {
                format!("failed to install pairing-code file {}", path.display())
            })?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return write_result;
    }

    Err(last_collision.unwrap_or_else(|| {
        std::io::Error::new(ErrorKind::AlreadyExists, "temporary file collision")
    }))
    .context("failed to allocate a temporary pairing-code file")
}

fn temporary_pairing_path(path: &Path) -> Result<PathBuf> {
    let mut name = path
        .file_name()
        .map_or_else(|| OsString::from("pairing-code"), OsString::from);
    name.push(".");
    name.push(random_url_token(6)?);
    name.push(".tmp");
    Ok(path.with_file_name(name))
}

fn pairing_lock_path(path: &Path) -> PathBuf {
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    PathBuf::from(lock_path)
}

fn with_pairing_lock<T>(path: &Path, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let lock_path = pairing_lock_path(path);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(&lock_path)
        .with_context(|| format!("failed to open pairing lock {}", lock_path.display()))?;
    File::lock(&lock)
        .with_context(|| format!("failed to acquire pairing lock {}", lock_path.display()))?;
    let result = operation();
    let unlock_result = File::unlock(&lock)
        .with_context(|| format!("failed to release pairing lock {}", lock_path.display()));
    match result {
        Ok(value) => {
            unlock_result?;
            Ok(value)
        }
        Err(error) => {
            let _ = unlock_result;
            Err(error)
        }
    }
}

fn consume_pairing_code(
    setup_token: &[u8],
    path: &Path,
    candidate: &str,
    maximum_ttl: Duration,
) -> Result<bool> {
    if candidate.len() != 6 || !candidate.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(false);
    }

    with_pairing_lock(path, || {
        consume_pairing_code_locked(setup_token, path, candidate, maximum_ttl)
    })
}

fn consume_pairing_code_locked(
    setup_token: &[u8],
    path: &Path,
    candidate: &str,
    maximum_ttl: Duration,
) -> Result<bool> {
    let record = match fs::read_to_string(path) {
        Ok(record) => record,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read pairing-code file {}", path.display()));
        }
    };
    let mut parts = record.trim().split('.');
    let version = parts.next().context("pairing-code record has no version")?;
    let expiry_text = parts.next().context("pairing-code record has no expiry")?;
    let signature_text = parts
        .next()
        .context("pairing-code record has no signature")?;
    if parts.next().is_some() || version != PAIRING_CODE_VERSION {
        bail!("pairing-code record has an unsupported format");
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature_text)
        .context("pairing-code record has an invalid signature encoding")?;
    let expiry = expiry_text
        .parse::<u64>()
        .context("pairing-code record has an invalid expiry")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();
    let Some(remaining) = expiry.checked_sub(now) else {
        remove_pairing_record(path)?;
        return Ok(false);
    };
    if remaining == 0 || remaining > maximum_ttl.as_secs() {
        remove_pairing_record(path)?;
        return Ok(false);
    }

    let payload = pairing_code_payload(expiry, candidate);
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, setup_token);
    if ring::hmac::verify(&key, payload.as_bytes(), &signature).is_err() {
        return Ok(false);
    }
    remove_pairing_record(path)?;
    Ok(true)
}

fn remove_pairing_record(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to remove pairing-code file {}", path.display())),
    }
}

pub fn load_or_create(config: &Config) -> Result<AuthMaterial> {
    let pairing_code_path = pairing_code_path(&config.token_file);
    if config.auth_mode == AuthMode::Disabled {
        return Ok(AuthMaterial {
            authenticator: Authenticator::disabled(),
            newly_created: false,
            token_path: config.token_file.clone(),
            pairing_code_path,
        });
    }

    let (token, newly_created) = load_or_create_token(&config.token_file)?;
    Ok(AuthMaterial {
        authenticator: Authenticator::from_token_with_ttls(
            token.as_bytes(),
            pairing_code_path.clone(),
            Duration::from_secs(config.pairing_code_ttl_seconds),
            Duration::from_secs(config.session_ttl_seconds),
        ),
        newly_created,
        token_path: config.token_file.clone(),
        pairing_code_path,
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
        let dir = tempdir().unwrap();
        let pairing_path = dir.path().join("token.pairing");
        let issuer = Authenticator::from_token(b"setup-token".to_vec(), pairing_path.clone());
        let code = issuer.issue_pairing_code().unwrap().unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
        assert!(!fs::read_to_string(&pairing_path).unwrap().contains(&code));
        assert_eq!(
            fs::metadata(&pairing_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(pairing_lock_path(&pairing_path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let auth = Authenticator::from_token(b"setup-token".to_vec(), pairing_path.clone());
        let session = auth.exchange(&code).await.unwrap().unwrap();

        assert!(!pairing_path.exists());
        assert!(auth.exchange(&code).await.unwrap().is_none());
        assert!(auth.verify_session(Some(&session)).await);
        assert!(!auth.verify_session(Some("wrong")).await);
        auth.revoke_session(Some(&session)).await;
        assert!(!auth.verify_session(Some(&session)).await);
    }

    #[tokio::test]
    async fn pairing_codes_reject_tampering_and_the_wrong_setup_token() {
        let dir = tempdir().unwrap();
        let pairing_path = dir.path().join("token.pairing");
        let auth = Authenticator::from_token(b"setup-token".to_vec(), pairing_path.clone());
        let code = auth.issue_pairing_code().unwrap().unwrap();
        let mut tampered = code.into_bytes();
        tampered[0] = if tampered[0] == b'0' { b'1' } else { b'0' };
        let tampered = String::from_utf8(tampered).unwrap();

        assert!(auth.exchange(&tampered).await.unwrap().is_none());

        let other = Authenticator::from_token(b"other-token".to_vec(), pairing_path);
        let valid_for_other = other.issue_pairing_code().unwrap().unwrap();
        assert!(auth.exchange(&valid_for_other).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn issuing_a_new_pairing_code_invalidates_the_previous_code() {
        let dir = tempdir().unwrap();
        let auth =
            Authenticator::from_token(b"setup-token".to_vec(), dir.path().join("token.pairing"));
        let first = auth.issue_pairing_code().unwrap().unwrap();
        let second = loop {
            let candidate = auth.issue_pairing_code().unwrap().unwrap();
            if candidate != first {
                break candidate;
            }
        };

        assert!(auth.exchange(&first).await.unwrap().is_none());
        assert!(auth.exchange(&second).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn expired_pairing_codes_are_rejected_and_removed() {
        let dir = tempdir().unwrap();
        let pairing_path = dir.path().join("token.pairing");
        let auth = Authenticator::from_token_with_ttls(
            b"setup-token".to_vec(),
            pairing_path.clone(),
            Duration::from_millis(1),
            Duration::from_hours(1),
        );
        let code = auth.issue_pairing_code().unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;

        assert!(auth.exchange(&code).await.unwrap().is_none());
        assert!(!pairing_path.exists());
    }

    #[tokio::test]
    async fn persistent_setup_token_is_accepted_but_not_a_session() {
        let dir = tempdir().unwrap();
        let auth =
            Authenticator::from_token(b"setup-token".to_vec(), dir.path().join("token.pairing"));
        let session = auth.exchange("setup-token").await.unwrap().unwrap();

        assert!(!auth.verify_session(Some("setup-token")).await);
        assert!(auth.verify_session(Some(&session)).await);
    }

    #[tokio::test]
    async fn sessions_expire() {
        let dir = tempdir().unwrap();
        let auth = Authenticator::from_token_with_ttls(
            b"setup-token".to_vec(),
            dir.path().join("token.pairing"),
            Duration::from_millis(1),
            Duration::from_millis(1),
        );
        let session = auth.exchange("setup-token").await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(!auth.verify_session(Some(&session)).await);
    }
}
