use std::{
    env, fs,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Deserialize;

pub const DEFAULT_PORT: u16 = 2721;
pub const DEFAULT_PAIRING_CODE_TTL_SECONDS: u64 = 300;
pub const DEFAULT_SESSION_TTL_SECONDS: u64 = 43_200;

#[derive(Debug, Parser)]
#[command(name = "portway", version, about)]
pub struct Cli {
    /// Configuration file path.
    #[arg(long, global = true, env = "PORTWAY_CONFIG")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the HTTP(S) and WebSocket server.
    Serve(Box<ServeOptions>),
    /// Print the persistent setup token to the terminal.
    Token,
    /// Print a six-digit pairing code for a running server.
    Pair,
}

#[derive(Debug, Default, Args)]
pub struct ServeOptions {
    #[arg(long, env = "PORTWAY_LISTEN")]
    pub listen: Option<IpAddr>,

    #[arg(long, env = "PORTWAY_PORT")]
    pub port: Option<u16>,

    #[arg(long, env = "PORTWAY_AUTH_MODE")]
    pub auth_mode: Option<AuthMode>,

    #[arg(long, env = "PORTWAY_TOKEN_FILE")]
    pub token_file: Option<PathBuf>,

    /// PEM certificate chain for native HTTPS. Requires --tls-key.
    #[arg(long, env = "PORTWAY_TLS_CERT")]
    pub tls_cert: Option<PathBuf>,

    /// PEM private key for native HTTPS. Requires --tls-cert.
    #[arg(long, env = "PORTWAY_TLS_KEY")]
    pub tls_key: Option<PathBuf>,

    #[arg(long, env = "PORTWAY_PAIRING_CODE_TTL_SECONDS")]
    pub pairing_code_ttl_seconds: Option<u64>,

    #[arg(long, env = "PORTWAY_SESSION_TTL_SECONDS")]
    pub session_ttl_seconds: Option<u64>,

    #[arg(long, env = "PORTWAY_BACKEND")]
    pub backend: Option<BackendKind>,

    #[arg(long, env = "PORTWAY_MAX_CLIENTS")]
    pub max_clients: Option<usize>,

    #[arg(long, env = "PORTWAY_POINTER_SENSITIVITY")]
    pub pointer_sensitivity: Option<f64>,

    #[arg(long, env = "PORTWAY_LOG_LEVEL")]
    pub log_level: Option<String>,

    /// Additional accepted browser origins. Same-host origins work by default.
    #[arg(
        long = "allow-origin",
        value_delimiter = ',',
        env = "PORTWAY_ALLOWED_ORIGINS"
    )]
    pub allowed_origins: Vec<String>,

    #[arg(long, env = "PORTWAY_MOUSE_NAME")]
    pub mouse_name: Option<String>,

    #[arg(long, env = "PORTWAY_KEYBOARD_NAME")]
    pub keyboard_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Token,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Auto,
    Uinput,
    Mock,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub config_path: PathBuf,
    pub listen: IpAddr,
    pub port: u16,
    pub auth_mode: AuthMode,
    pub token_file: PathBuf,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub pairing_code_ttl_seconds: u64,
    pub session_ttl_seconds: u64,
    pub backend: BackendKind,
    pub max_clients: usize,
    pub pointer_sensitivity: f64,
    pub log_level: String,
    pub allowed_origins: Vec<String>,
    pub mouse_name: String,
    pub keyboard_name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    listen: Option<IpAddr>,
    port: Option<u16>,
    auth_mode: Option<AuthMode>,
    token_file: Option<PathBuf>,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
    pairing_code_ttl_seconds: Option<u64>,
    session_ttl_seconds: Option<u64>,
    backend: Option<BackendKind>,
    max_clients: Option<usize>,
    pointer_sensitivity: Option<f64>,
    log_level: Option<String>,
    allowed_origins: Option<Vec<String>>,
    mouse_name: Option<String>,
    keyboard_name: Option<String>,
}

impl Config {
    /// Resolve configuration with precedence CLI/environment > file > defaults.
    pub fn load(path_override: Option<PathBuf>, cli: &ServeOptions) -> Result<Self> {
        let explicit_path = path_override.is_some();
        let config_path = path_override.unwrap_or_else(default_config_path);
        let file = read_file_config(&config_path, explicit_path)?;
        let token_default = config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("token");

        let config = Self {
            config_path,
            listen: cli
                .listen
                .or(file.listen)
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            port: cli.port.or(file.port).unwrap_or(DEFAULT_PORT),
            auth_mode: cli.auth_mode.or(file.auth_mode).unwrap_or(AuthMode::Token),
            token_file: cli
                .token_file
                .clone()
                .or(file.token_file)
                .unwrap_or(token_default),
            tls_cert: cli.tls_cert.clone().or(file.tls_cert),
            tls_key: cli.tls_key.clone().or(file.tls_key),
            pairing_code_ttl_seconds: cli
                .pairing_code_ttl_seconds
                .or(file.pairing_code_ttl_seconds)
                .unwrap_or(DEFAULT_PAIRING_CODE_TTL_SECONDS),
            session_ttl_seconds: cli
                .session_ttl_seconds
                .or(file.session_ttl_seconds)
                .unwrap_or(DEFAULT_SESSION_TTL_SECONDS),
            backend: cli.backend.or(file.backend).unwrap_or(BackendKind::Auto),
            max_clients: cli.max_clients.or(file.max_clients).unwrap_or(1),
            pointer_sensitivity: cli
                .pointer_sensitivity
                .or(file.pointer_sensitivity)
                .unwrap_or(1.0),
            log_level: cli
                .log_level
                .clone()
                .or(file.log_level)
                .unwrap_or_else(|| "info".to_owned()),
            allowed_origins: if cli.allowed_origins.is_empty() {
                file.allowed_origins.unwrap_or_default()
            } else {
                cli.allowed_origins.clone()
            },
            mouse_name: cli
                .mouse_name
                .clone()
                .or(file.mouse_name)
                .unwrap_or_else(|| "Portway virtual mouse".to_owned()),
            keyboard_name: cli
                .keyboard_name
                .clone()
                .or(file.keyboard_name)
                .unwrap_or_else(|| "Portway virtual keyboard".to_owned()),
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.port == 0 {
            bail!("port must be between 1 and 65535");
        }
        if !(1..=8).contains(&self.max_clients) {
            bail!("max_clients must be between 1 and 8");
        }
        if self.tls_cert.is_some() != self.tls_key.is_some() {
            bail!("tls_cert and tls_key must be configured together");
        }
        if !(30..=3_600).contains(&self.pairing_code_ttl_seconds) {
            bail!("pairing_code_ttl_seconds must be between 30 and 3600");
        }
        if !(300..=604_800).contains(&self.session_ttl_seconds) {
            bail!("session_ttl_seconds must be between 300 and 604800");
        }
        if !self.pointer_sensitivity.is_finite() || !(0.1..=5.0).contains(&self.pointer_sensitivity)
        {
            bail!("pointer_sensitivity must be between 0.1 and 5.0");
        }
        if self.mouse_name.is_empty() || self.mouse_name.len() > 79 {
            bail!("mouse_name must contain 1 to 79 bytes");
        }
        if self.keyboard_name.is_empty() || self.keyboard_name.len() > 79 {
            bail!("keyboard_name must contain 1 to 79 bytes");
        }
        for origin in &self.allowed_origins {
            validate_origin(origin)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn tls_enabled(&self) -> bool {
        self.tls_cert.is_some()
    }
}

#[must_use]
pub fn default_config_path() -> PathBuf {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join("portway/config.toml");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".config/portway/config.toml");
    }
    PathBuf::from("portway.toml")
}

fn read_file_config(path: &Path, required: bool) -> Result<FileConfig> {
    match fs::read_to_string(path) {
        Ok(contents) => toml::from_str(&contents)
            .with_context(|| format!("failed to parse configuration {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {
            Ok(FileConfig::default())
        }
        Err(error) => {
            Err(error).with_context(|| format!("failed to read configuration {}", path.display()))
        }
    }
}

fn validate_origin(origin: &str) -> Result<()> {
    let uri: http::Uri = origin
        .parse()
        .with_context(|| format!("invalid allowed origin {origin:?}"))?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.authority().is_none() {
        bail!("allowed origin must be an absolute http(s) origin: {origin:?}");
    }
    if uri
        .path_and_query()
        .is_some_and(|value| value.as_str() != "/")
    {
        bail!("allowed origin must not contain a path or query: {origin:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, net::Ipv4Addr};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn loads_file_and_applies_cli_precedence() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("portway.toml");
        fs::write(
            &path,
            "listen = \"127.0.0.1\"\nport = 3000\nmax_clients = 2\nbackend = \"mock\"\n",
        )
        .unwrap();
        let cli = ServeOptions {
            port: Some(4000),
            ..ServeOptions::default()
        };

        let config = Config::load(Some(path), &cli).unwrap();

        assert_eq!(config.listen, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(config.port, 4000);
        assert_eq!(config.max_clients, 2);
        assert_eq!(config.backend, BackendKind::Mock);
    }

    #[test]
    fn rejects_unknown_file_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("portway.toml");
        fs::write(&path, "surprise = true\n").unwrap();
        let error = Config::load(Some(path), &ServeOptions::default()).unwrap_err();
        assert!(error.to_string().contains("failed to parse"));
    }

    #[test]
    fn validates_limits() {
        let cli = ServeOptions {
            max_clients: Some(0),
            ..ServeOptions::default()
        };
        assert!(Config::load(None, &cli).is_err());
    }

    #[test]
    fn requires_a_complete_tls_keypair() {
        let cli = ServeOptions {
            tls_cert: Some(PathBuf::from("cert.pem")),
            ..ServeOptions::default()
        };
        let error = Config::load(None, &cli).unwrap_err();
        assert!(error.to_string().contains("configured together"));
    }
}
