use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use axum_server::tls_rustls::RustlsConfig;
use clap::Parser;
use portway::{
    auth,
    config::{AuthMode, Cli, Command, Config, ServeOptions},
    input::{SharedBackend, create_backend},
    local_pairing::{LocalPairingServer, request_pairing_code},
    server::{AppState, serve, serve_tls},
};
use tokio::net::TcpListener;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(options) => {
            let config = Config::load(cli.config, &options)?;
            init_logging(&config.log_level)?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("failed to build Tokio runtime")?
                .block_on(run(config))
        }
        Command::Token => print_token(cli.config),
        Command::Pair => print_pairing_code(cli.config),
    }
}

fn print_pairing_code(config_path: Option<std::path::PathBuf>) -> Result<()> {
    let config = Config::load(config_path, &ServeOptions::default())?;
    if config.auth_mode == AuthMode::Disabled {
        bail!("authentication is disabled; a pairing code is not required");
    }
    println!("{}", request_pairing_code(&config.pairing_socket)?);
    Ok(())
}

fn print_token(config_path: Option<std::path::PathBuf>) -> Result<()> {
    let config = Config::load(config_path, &ServeOptions::default())?;
    if config.auth_mode == AuthMode::Disabled {
        bail!("authentication is disabled; there is no token");
    }
    let _ = auth::load_or_create(&config)?;
    println!("{}", auth::read_existing_token(&config.token_file)?);
    Ok(())
}

async fn run(config: Config) -> Result<()> {
    let auth = auth::load_or_create(&config)?;
    let pairing_server = if auth.authenticator.is_enabled() {
        Some(LocalPairingServer::bind(
            config.pairing_socket.clone(),
            config.pairing_allowed_uids.clone(),
        )?)
    } else {
        None
    };
    let backend = create_backend(&config)?;
    let status = backend.status();
    let backend: SharedBackend = Arc::new(tokio::sync::Mutex::new(backend));
    let state = AppState::new(&config, Arc::clone(&backend), auth.authenticator.clone());
    let tls_config = match (&config.tls_cert, &config.tls_key) {
        (Some(cert), Some(key)) => {
            rustls::crypto::ring::default_provider()
                .install_default()
                .map_err(|_| anyhow::anyhow!("failed to install the Rustls Ring provider"))?;
            Some(
                RustlsConfig::from_pem_file(cert, key)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to load TLS certificate {} and key {}",
                            cert.display(),
                            key.display()
                        )
                    })?,
            )
        }
        (None, None) => None,
        _ => unreachable!("configuration validation requires both TLS paths"),
    };
    let scheme = if tls_config.is_some() {
        "https"
    } else {
        "http"
    };
    let address = SocketAddr::new(config.listen, config.port);
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to listen on {address}"))?;
    let bound = listener
        .local_addr()
        .context("failed to read listen address")?;
    let pairing_task = pairing_server.map(|server| {
        let pairing_auth = auth.authenticator.clone();
        tokio::spawn(async move {
            if let Err(error) = server.run(pairing_auth).await {
                error!(%error, "local pairing service stopped");
            }
        })
    });

    info!(config = %config.config_path.display(), "configuration loaded");
    info!(listen = %bound, "Portway server started");
    info!(url = %local_url(scheme, config.listen, bound.port()), "local control URL");
    if let Some(lan_ip) = advertised_lan_ip(config.listen) {
        info!(url = %url_for_ip(scheme, lan_ip, bound.port()), "LAN control URL");
    }
    info!(
        authentication = if config.auth_mode == AuthMode::Token { "token" } else { "disabled" },
        token_file = %auth.token_path.display(),
        pairing_code_file = %auth.pairing_code_path.display(),
        pairing_socket = %config.pairing_socket.display(),
        "authentication status"
    );
    if auth.newly_created {
        info!(token_file = %auth.token_path.display(), "persistent setup token created");
    }
    if config.auth_mode == AuthMode::Disabled {
        warn!("AUTHENTICATION IS DISABLED; any network peer can control this machine");
    }
    if tls_config.is_some() {
        info!("native HTTPS enabled; session cookies require secure transport");
    } else {
        warn!("HTTP transport is unencrypted; use only on a trusted network or configure TLS");
    }
    if status.available {
        info!(backend = %status.name, detail = status.detail.as_deref().unwrap_or("ready"), "input backend initialized");
    } else {
        error!(backend = %status.name, detail = status.detail.as_deref().unwrap_or("unknown"), "input backend unavailable; server is running in degraded mode");
    }

    let server_result = if let Some(tls_config) = tls_config {
        serve_tls(listener, state, tls_config, shutdown_signal()).await
    } else {
        serve(listener, state, shutdown_signal()).await
    };
    if let Some(task) = pairing_task {
        task.abort();
        let _ = task.await;
    }
    info!("shutdown requested; releasing all input state");
    if let Err(error) = backend.lock().await.release_all() {
        error!(%error, "final input cleanup failed");
    }
    server_result.context("HTTP server failed")?;
    info!("Portway stopped");
    Ok(())
}

fn init_logging(level: &str) -> Result<()> {
    let filter =
        EnvFilter::try_new(level).with_context(|| format!("invalid log level {level:?}"))?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize logging: {error}"))
}

fn local_url(scheme: &str, listen: IpAddr, port: u16) -> String {
    if listen.is_unspecified() || listen.is_loopback() {
        format!("{scheme}://localhost:{port}")
    } else {
        url_for_ip(scheme, listen, port)
    }
}

fn advertised_lan_ip(listen: IpAddr) -> Option<IpAddr> {
    if listen.is_loopback() {
        None
    } else if listen.is_unspecified() {
        detect_lan_ip()
    } else {
        Some(listen)
    }
}

fn url_for_ip(scheme: &str, ip: IpAddr, port: u16) -> String {
    format!("{scheme}://{}", SocketAddr::new(ip, port))
}

fn detect_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:80").ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then_some(address)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            result = tokio::signal::ctrl_c() => { let _ = result; }
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_advertise_loopback_as_lan_reachable() {
        assert_eq!(advertised_lan_ip(IpAddr::from([127, 0, 0, 1])), None);
        assert_eq!(
            advertised_lan_ip(IpAddr::from([192, 0, 2, 15])),
            Some(IpAddr::from([192, 0, 2, 15]))
        );
    }

    #[test]
    fn formats_ipv6_urls_with_brackets() {
        assert_eq!(
            url_for_ip("https", "2001:db8::1".parse().unwrap(), 2721),
            "https://[2001:db8::1]:2721"
        );
    }
}
