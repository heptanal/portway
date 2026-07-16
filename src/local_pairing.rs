use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

const MAX_RESPONSE_BYTES: u64 = 256;
const ERROR_PREFIX: &str = "error: ";

#[cfg(unix)]
use std::os::unix::{
    fs::{FileTypeExt, MetadataExt, PermissionsExt},
    net::UnixStream as StdUnixStream,
};

#[cfg(unix)]
use tokio::{io::AsyncWriteExt, net::UnixListener};

#[cfg(unix)]
use tracing::{info, warn};

#[cfg(unix)]
use crate::auth::Authenticator;

/// Ask the running local Portway service to create a pairing code.
#[cfg(unix)]
pub fn request_pairing_code(path: &Path) -> Result<String> {
    let stream = StdUnixStream::connect(path)
        .with_context(|| format!("failed to connect to pairing socket {}", path.display()))?;
    let mut response = String::new();
    BufReader::new(stream)
        .take(MAX_RESPONSE_BYTES)
        .read_line(&mut response)
        .with_context(|| format!("failed to read pairing socket {}", path.display()))?;
    let response = response.trim();
    if let Some(message) = response.strip_prefix(ERROR_PREFIX) {
        bail!("{message}");
    }
    if response.len() != 6 || !response.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("pairing service returned an invalid response");
    }
    Ok(response.to_owned())
}

#[cfg(not(unix))]
pub fn request_pairing_code(_path: &Path) -> Result<String> {
    bail!("local pairing sockets are not supported on this platform")
}

/// Local pairing-code service protected by kernel-reported peer user IDs.
#[cfg(unix)]
#[derive(Debug)]
pub struct LocalPairingServer {
    listener: UnixListener,
    path: PathBuf,
    owner_uid: u32,
    allowed_uids: Vec<u32>,
}

#[cfg(unix)]
impl LocalPairingServer {
    pub fn bind(path: PathBuf, allowed_uids: Vec<u32>) -> Result<Self> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create pairing socket directory {}",
                parent.display()
            )
        })?;
        remove_stale_socket(&path)?;
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("failed to bind pairing socket {}", path.display()))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666))
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
        let owner_uid = fs::metadata(&path)
            .with_context(|| format!("failed to inspect pairing socket {}", path.display()))?
            .uid();

        Ok(Self {
            listener,
            path,
            owner_uid,
            allowed_uids,
        })
    }

    pub async fn run(self, auth: Authenticator) -> Result<()> {
        info!(
            path = %self.path.display(),
            allowed_uids = ?self.allowed_uids,
            "local pairing service started"
        );
        loop {
            let (mut stream, _) = self.listener.accept().await.with_context(|| {
                format!("failed to accept on pairing socket {}", self.path.display())
            })?;
            let peer_uid = match stream.peer_cred() {
                Ok(credentials) => credentials.uid(),
                Err(error) => {
                    warn!(%error, "failed to identify local pairing caller");
                    let _ = stream
                        .write_all(b"error: local pairing caller could not be identified\n")
                        .await;
                    continue;
                }
            };
            if !peer_uid_allowed(peer_uid, self.owner_uid, &self.allowed_uids) {
                warn!(peer_uid, "unauthorized local pairing request");
                let _ = stream
                    .write_all(b"error: this local user is not allowed to request pairing codes\n")
                    .await;
                continue;
            }

            match auth.issue_pairing_code() {
                Ok(Some(code)) => {
                    if let Err(error) = stream.write_all(format!("{code}\n").as_bytes()).await {
                        warn!(peer_uid, %error, "local pairing caller disconnected");
                    } else {
                        info!(peer_uid, "local pairing code issued");
                    }
                }
                Ok(None) => {
                    if let Err(error) = stream
                        .write_all(b"error: authentication is disabled\n")
                        .await
                    {
                        warn!(peer_uid, %error, "failed to return local pairing status");
                    }
                }
                Err(error) => {
                    warn!(peer_uid, %error, "failed to issue local pairing code");
                    if let Err(error) = stream
                        .write_all(b"error: pairing service failed to create a code\n")
                        .await
                    {
                        warn!(peer_uid, %error, "failed to return local pairing failure");
                    }
                }
            }
        }
    }
}

#[cfg(unix)]
fn peer_uid_allowed(peer_uid: u32, owner_uid: u32, allowed_uids: &[u32]) -> bool {
    peer_uid == 0 || peer_uid == owner_uid || allowed_uids.contains(&peer_uid)
}

#[cfg(unix)]
impl Drop for LocalPairingServer {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| metadata.file_type().is_socket()) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn remove_stale_socket(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(path)
            .with_context(|| format!("failed to remove stale pairing socket {}", path.display())),
        Ok(_) => bail!(
            "refusing to replace non-socket pairing path {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect pairing socket {}", path.display())),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::auth::Authenticator;
    use tempfile::tempdir;

    #[tokio::test(flavor = "multi_thread")]
    async fn local_service_issues_a_six_digit_single_use_code() {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("pair.sock");
        let pairing_path = dir.path().join("token.pairing");
        let auth = Authenticator::from_token(b"setup-token".to_vec(), pairing_path);
        let server = LocalPairingServer::bind(socket_path.clone(), Vec::new()).unwrap();
        assert_eq!(
            fs::metadata(&socket_path).unwrap().permissions().mode() & 0o777,
            0o666
        );
        let task = tokio::spawn(server.run(auth.clone()));

        let request_path = socket_path.clone();
        let code = tokio::task::spawn_blocking(move || request_pairing_code(&request_path))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
        assert!(auth.exchange(&code).await.unwrap().is_some());
        assert!(auth.exchange(&code).await.unwrap().is_none());

        task.abort();
        let _ = task.await;
        assert!(!socket_path.exists());
    }

    #[test]
    fn refuses_to_replace_a_non_socket_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("pair.sock");
        fs::write(&path, "keep me").unwrap();

        let error = LocalPairingServer::bind(path.clone(), Vec::new()).unwrap_err();

        assert!(error.to_string().contains("refusing to replace"));
        assert_eq!(fs::read_to_string(path).unwrap(), "keep me");
    }

    #[test]
    fn authorizes_root_owner_and_explicit_uids_only() {
        assert!(peer_uid_allowed(0, 991, &[]));
        assert!(peer_uid_allowed(991, 991, &[]));
        assert!(peer_uid_allowed(1000, 991, &[1000]));
        assert!(!peer_uid_allowed(1001, 991, &[1000]));
    }
}
