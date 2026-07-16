# Command-line reference

Portway has three application commands: `serve`, `token`, and `pair`. Clap also
provides the `help` command and the usual help/version flags.

```text
portway [--config PATH] <COMMAND> [COMMAND OPTIONS]
```

The global `--config` option may appear before or after a command. `-h` and
`--help` print help, while `-V` and `--version` print the application version.
`portway help [COMMAND]` is the long-form equivalent of `--help`.

## Configuration and precedence

For `serve`, configuration is resolved in this order:

```text
CLI flag > matching PORTWAY_* environment variable > TOML file > default
```

`PORTWAY_CONFIG` and `--config` select the TOML file for every command. The
other environment variables and configuration flags belong to `serve`; `token`
and `pair` load their settings from the selected file and defaults.

If `/etc/portway/config.toml` exists it is the implicit default, regardless of
the current `HOME`; this keeps normal and `sudo` commands on the installed
service configuration. Otherwise the default is
`$XDG_CONFIG_HOME/portway/config.toml`, then `$HOME/.config/portway/config.toml`,
then `./portway.toml`. Use `--config` or `PORTWAY_CONFIG` to override discovery.
An explicitly selected file must exist; an implicit user-level file may be
absent. Unknown TOML fields are rejected. See
[`config/portway.example.toml`](../config/portway.example.toml) for every key.

## `serve`

`portway serve` starts the HTTP or HTTPS site, pairing/session API, and
WebSocket controller. It runs until SIGINT or SIGTERM and releases held input
during a clean shutdown.

| Flag | Environment / TOML | Default | Meaning and validation |
| --- | --- | --- | --- |
| `--listen IP` | `PORTWAY_LISTEN` / `listen` | `0.0.0.0` | IP address to bind. Hostnames are not accepted. |
| `--port PORT` | `PORTWAY_PORT` / `port` | `2721` | TCP port, from 1 through 65535. |
| `--auth-mode MODE` | `PORTWAY_AUTH_MODE` / `auth_mode` | `token` | `token` requires pairing; `disabled` permits any network peer to control the host. |
| `--token-file PATH` | `PORTWAY_TOKEN_FILE` / `token_file` | `token` beside the resolved config file | Persistent 256-bit setup-token file. It is created securely on first authenticated `serve` or `token` use. |
| `--pairing-socket PATH` | `PORTWAY_PAIRING_SOCKET` / `pairing_socket` | configured token path plus `.socket` | Local Unix socket through which `portway pair` asks the running service to issue a code. The installer uses `/run/portway/pair.sock`. |
| `--pairing-allowed-uid UID[,UID...]` | `PORTWAY_PAIRING_ALLOWED_UIDS` / `pairing_allowed_uids` | empty | Additional local user IDs allowed to request codes. The socket owner and UID 0 are always allowed. May be repeated; CLI/environment values replace the TOML list. |
| `--tls-cert PATH` | `PORTWAY_TLS_CERT` / `tls_cert` | unset | PEM certificate chain for native HTTPS; requires `--tls-key` or `tls_key`. |
| `--tls-key PATH` | `PORTWAY_TLS_KEY` / `tls_key` | unset | PEM private key for native HTTPS; requires `--tls-cert` or `tls_cert`. |
| `--pairing-code-ttl-seconds SECONDS` | `PORTWAY_PAIRING_CODE_TTL_SECONDS` / `pairing_code_ttl_seconds` | `300` | Pairing-code lifetime, from 30 through 3600 seconds. |
| `--session-ttl-seconds SECONDS` | `PORTWAY_SESSION_TTL_SECONDS` / `session_ttl_seconds` | `43200` | Fixed browser-session lifetime, from 300 through 604800 seconds. Activity does not extend it. |
| `--backend BACKEND` | `PORTWAY_BACKEND` / `backend` | `auto` | `auto` tries uinput and stays up in a diagnostic degraded state on failure; `uinput` makes failure fatal; `mock` records events without controlling the host. |
| `--max-clients COUNT` | `PORTWAY_MAX_CLIENTS` / `max_clients` | `1` | Simultaneous WebSocket controllers, from 1 through 8. This is separate from authenticated browser sessions. |
| `--pointer-sensitivity VALUE` | `PORTWAY_POINTER_SENSITIVITY` / `pointer_sensitivity` | `1.0` | Server-side movement multiplier, from 0.1 through 5.0. The browser also has a separate local sensitivity setting. |
| `--log-level FILTER` | `PORTWAY_LOG_LEVEL` / `log_level` | `info` | A `tracing-subscriber` environment-filter expression, such as `debug` or `portway=debug`. Invalid filters stop startup. |
| `--allow-origin ORIGIN[,ORIGIN...]` | `PORTWAY_ALLOWED_ORIGINS` / `allowed_origins` | empty | Extra exact browser origins for a proxy. Values must be absolute `http://` or `https://` origins with no path or query. Same-host origins already work. |
| `--mouse-name NAME` | `PORTWAY_MOUSE_NAME` / `mouse_name` | `Portway virtual mouse` | uinput mouse device name, from 1 through 79 bytes. |
| `--keyboard-name NAME` | `PORTWAY_KEYBOARD_NAME` / `keyboard_name` | `Portway virtual keyboard` | uinput keyboard device name, from 1 through 79 bytes. |

`--allow-origin` accepts comma-separated values; the TOML form is an array of
strings. Supplying CLI/environment origins replaces the file list rather than
adding to it.

Examples:

```sh
portway serve --backend mock --listen 127.0.0.1
portway --config /etc/portway/config.toml serve
PORTWAY_PORT=3000 PORTWAY_LOG_LEVEL=debug portway serve
```

## `token`

`portway token` creates the configured setup token if it is missing, then prints
the persistent token. It refuses to run when `auth_mode = "disabled"`.

```sh
sudo -u portway portway token
```

Run it as the same user and with the same configuration as the server. Printing
the token is a deliberate recovery operation: anyone who obtains it can create
browser sessions. `token` does not rotate an existing token.

Options are the global `--config PATH` and `-h`/`--help` only.

## `pair`

`portway pair` connects to the configured local pairing socket and asks the
running server to create a random six-digit code. It prints only that code. The
server authorizes the caller from kernel-provided Unix peer credentials; the CLI
does not read the setup token or pairing record.

```sh
portway pair
```

Enter the code in the pairing dialog at the Portway website. It uses the
configured pairing-code lifetime, five minutes by default, and can be exchanged
once. Issuing a new code invalidates the previous one. The protected record is
stored at the token path with `.pairing` appended and contains an expiry and HMAC,
not the six-digit plaintext. The command fails when authentication is disabled,
the service is not running, or the local UID is not authorized. Installed systems
discover `/etc/portway/config.toml` automatically, including under `sudo`.

Options are global `--config PATH` and `-h`/`--help` only.

## Repository operations scripts

These scripts are separate from the `portway` binary:

### `scripts/install-linux`

Installs or upgrades a conventional systemd host. It elevates through `sudo`
when necessary and refuses to modify NixOS.

| Flag | Meaning |
| --- | --- |
| `--profile lan|local|https` | Select authenticated LAN HTTP, localhost HTTP, or native LAN HTTPS. Without it, an interactive terminal asks; unattended mode defaults to `lan`. |
| `--binary PATH` | Install this executable instead of auto-detecting `target/release/portway` or a sibling `portway` binary. |
| `--port PORT` | Configure TCP port 1 through 65535; default `2721`. An existing preserved config keeps its recorded port unless `--force-config` is used. |
| `--tls-cert PATH` | Source PEM certificate chain for the `https` profile. The installer copies a snapshot into `/var/lib/portway/tls`. |
| `--tls-key PATH` | Source PEM private key for the `https` profile; required with `--tls-cert`. |
| `--auth token|disabled` | Select token authentication or explicitly disable authentication; default `token`. |
| `--accept-risk` | Required together with `--auth disabled`; otherwise the installer refuses the unsafe configuration. |
| `--firewall auto|skip` | Add an absent rule to active ufw/firewalld when `auto`, or leave firewall policy untouched when `skip`; default `auto`. |
| `--pair-user USER` | Authorize this local account to run `portway pair` without privilege elevation. Defaults to the invoking `sudo` user, the previously recorded pairing user during upgrade, or root. |
| `--force-config` | Replace `/etc/portway/config.toml`, saving the previous file as `config.toml.bak`. Without it, upgrades preserve existing configuration and credentials. |
| `--no-start` | Install files without enabling or starting the systemd service. |
| `--yes` | Accept defaults and suppress interactive confirmation. |
| `-h`, `--help` | Print installer help without requiring root. |

### `scripts/uninstall-linux`

| Flag | Meaning |
| --- | --- |
| `--yes` | Suppress interactive confirmation. |
| `--purge` | Permanently delete `/etc/portway`, `/var/lib/portway`, and installer-owned service accounts in addition to executable behavior. Without it, those items are preserved. |
| `-h`, `--help` | Print uninstaller help without requiring root. |

The uninstaller disables/stops the service and removes the binary, unit, udev
rule, modules-load entry, and any installer-recorded firewall rule. It does not
unload the `uinput` kernel module.

### Development helpers

`scripts/deploy-linux USER@HOST` copies source with `rsync` and performs a locked
release build in `~/portway-build`. It has no flags. Set `PORTWAY_REMOTE_DIR` to
choose another dedicated remote build directory.

`scripts/generate-test-cert [OUTPUT [HOSTNAME [IP]]]` creates a seven-day
self-signed certificate for local smoke testing. Its positional defaults are
`.portway-tls`, `localhost`, and `127.0.0.1`; it is not a production PKI tool.
