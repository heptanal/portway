# Portway

Portway is a self-hosted remote mouse and keyboard controller for Linux. It runs
a small local HTTP(S)/WebSocket server and serves a phone-first browser controller.
Input is emitted through kernel `/dev/uinput` devices, so the basic path does not
depend on X11, Wayland, a cloud service, telemetry, or a native phone app.

This repository contains an initial usable milestone: authenticated control,
touchpad gestures, mouse buttons, US-ASCII text entry, sticky modifiers, special
keys, a recording test backend, and a direct Linux uinput backend.

## Security first

Portway defaults to `0.0.0.0:2721` so phones on the LAN can connect. This also
exposes the port on every reachable interface. Authentication is required by
default, but HTTP does not encrypt pairing or input. Use Portway only on a
trusted network, restrict it with a firewall, or configure native HTTPS with a
certificate trusted by the controller. See [docs/SECURITY.md](docs/SECURITY.md)
and [docs/HTTPS.md](docs/HTTPS.md).

Do not run the network server as root for normal use. The recommended setup is a
dedicated `portway` user/group with `0660` udev access to `/dev/uinput`. Membership
in that group is powerful because it permits synthetic input.

## Quick start on Linux

Portway currently targets Rust 1.97.0. On a conventional systemd distribution,
build and run the guided installer:

```sh
cargo build --release --locked
scripts/install-linux
```

Choose authenticated LAN HTTP, localhost-only, or certificate-backed HTTPS. The
installer creates an unprivileged boot service, configures `/dev/uinput`, loads
the kernel module at every boot, handles active `ufw`/`firewalld` when requested,
starts Portway, and prints a temporary pairing URL. Details and unattended flags
are in [docs/INSTALL.md](docs/INSTALL.md).

NixOS must remain declarative. Add the local checkout as a flake input so the
package uses the same pinned Rust 1.97.0 toolchain as development:

```nix
{
  inputs.portway.url = "path:/path/to/Portway";

  outputs = { nixpkgs, portway, ... }: {
    nixosConfigurations.your-host = nixpkgs.lib.nixosSystem {
      modules = [
        portway.nixosModules.default
        {
          services.portway = {
            enable = true;
            firewallInterfaces = [ "wlp2s0" ];
          };
        }
      ];
    };
  };
}
```

After installation, generate another five-minute, single-use pairing URL without
restarting the service:

```sh
sudo -u portway portway --config /etc/portway/config.toml pair
```

Open a reported URL from the host or phone, for example:

```text
http://localhost:2721/?pair=<temporary-code>
```

From a phone on the same LAN open the reported address, for example:

```text
http://192.168.1.42:2721/?pair=<temporary-code>
```

The browser removes the code from its address bar, exchanges it for an `HttpOnly`
session cookie, and does not persist the submitted credential in browser storage.
The persistent setup token is also accepted in the pairing form for recovery.
Neither credential is sent in a WebSocket URL. To create or deliberately display
the setup token as the same user running the service:

```sh
sudo -u portway portway --config /etc/portway/config.toml token
```

Sessions expire after 12 hours by default and are invalidated by logout or server
restart. Pairing-code replay tracking is memory-only: restarting the server within
a code's five-minute validity window can make that code usable once by the new
process. Native HTTPS configuration is described in
[docs/HTTPS.md](docs/HTTPS.md).

Find LAN addresses manually with `ip -brief address` or `hostname -I`. Firewalls
must permit inbound TCP port 2721 on the intended trusted interface.

## Controller behavior

- One-finger movement is coalesced to animation frames to reduce message volume.
- A short tap clicks left; two quick taps produce a normal double click.
- A two-finger tap clicks right; two-finger movement scrolls both axes.
- A long press initiates click-and-drag; Drag lock holds left explicitly.
- Explicit left, middle, and right controls support press-and-hold.
- Modifier taps latch; holding for 350 ms makes the press momentary.
- Release all clears every backend key/button and all local sticky state.
- Hiding/leaving the page, 30 seconds of held-state inactivity, WebSocket loss,
  server heartbeat timeout, and clean server shutdown all trigger cleanup.

The `text_input` path supports printable US-ASCII, newline, and tab. It maps a US
ANSI layout and handles Shift combinations. It does not claim arbitrary Unicode
or non-US layout support. Raw named key events cover navigation, modifiers,
F1-F12, and common media keys.

## Configuration

For `serve`, precedence is predictable:

```text
CLI flags and PORTWAY_* environment variables > TOML file > defaults
```

The default file is `$XDG_CONFIG_HOME/portway/config.toml`, normally
`~/.config/portway/config.toml`. Pass another with `--config`. See
[config/portway.example.toml](config/portway.example.toml). The complete command,
flag, environment-variable, and validation reference is in
[docs/CLI.md](docs/CLI.md).

```sh
portway --config ./config/portway.example.toml serve \
  --backend mock \
  --token-file /tmp/portway-dev-token \
  --port 2721
portway serve --help
```

Important defaults are `listen = "0.0.0.0"`, `port = 2721`, token authentication,
`backend = "auto"`, one controller, and sensitivity `1.0`. On Linux, automatic
backend setup tries uinput and keeps the diagnostic web server running in a
degraded state if permission/device setup fails. `--backend uinput` makes that
failure fatal. `--backend mock` records input without controlling the host.

Native HTTPS is enabled only when both `tls_cert` and `tls_key` are set. Pairing
codes default to 300 seconds and controller sessions to 43200 seconds; use
`--pairing-code-ttl-seconds` and `--session-ttl-seconds` or their TOML/environment
equivalents to change those bounded values.

For a local HTTPS smoke test only:

```sh
scripts/generate-test-cert .portway-tls localhost 127.0.0.1
portway serve --backend mock --listen 127.0.0.1 \
  --tls-cert .portway-tls/cert.pem --tls-key .portway-tls/key.pem
curl --insecure --include https://localhost:2721/healthz
```

## Development and testing on Linux

To sync the source and build over SSH without moving the macOS toolchain:

```sh
scripts/deploy-linux user@linux-host
```

This uses `rsync`, then runs `cargo build --release --locked` remotely in
`~/portway-build`. Rust 1.97.0, Cargo, `rsync`, and `/dev/uinput` must be present.
On NixOS, where the packaged compiler or default user environment may not meet
those assumptions, use the reproducible rustup/linker commands and declarative
permission setup in [docs/NIXOS.md](docs/NIXOS.md).

After configuring the udev rule and refreshing group membership, the exact first
real-device test is:

```sh
cargo test input::linux::tests::creates_real_uinput_devices -- --ignored --nocapture
```

It creates real virtual devices and emits one movement plus a Shift press/release.
It is ignored by default and must run on a real Linux host. A mock-backed test
run cannot prove that a particular Linux compositor recognizes the virtual
devices.

## Tests and quality checks

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
```

Tests cover strict protocol parsing, malformed messages, secure token-file
creation, single-use pairing, session expiry/revocation, pairing throttling, US
key mapping, sticky Shift preservation, pointer/scroll handling, press/release
ordering, cleanup, release-all, configuration, origin checks, and an in-process
cookie-authenticated WebSocket using the recording backend.

## Service lifecycle

The installer enables Portway at boot and the hardened service restarts after
unexpected exits. Upgrade by rerunning `scripts/install-linux`; existing settings
and credentials are preserved. Uninstall is equally explicit:

```sh
scripts/uninstall-linux          # preserve configuration and credentials
scripts/uninstall-linux --purge  # permanently remove all Portway state
```

Portway cannot power on or wake a suspended machine, repair its network, or
guarantee that a compositor/login screen accepts virtual input. See the precise
availability boundary in [docs/INSTALL.md](docs/INSTALL.md).

## Platform caveats

Kernel virtual devices avoid direct X11/Wayland dependencies, but recognition and
policy remain system-specific. A compositor may ignore a device, a headless host
may have no input consumer, and input availability at lock/login screens varies
by display manager and policy. Portway makes no guarantee that remote control is
available across session boundaries. See [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md).

## Project documents

- [Architecture](docs/ARCHITECTURE.md)
- [Command-line reference](docs/CLI.md)
- [Installation and lifecycle](docs/INSTALL.md)
- [Protocol v1](docs/PROTOCOL.md)
- [Security model](docs/SECURITY.md)
- [HTTPS deployment](docs/HTTPS.md)
- [Troubleshooting](docs/TROUBLESHOOTING.md)
- [NixOS development and installation](docs/NIXOS.md)
- [Dependency policy](docs/DEPENDENCIES.md)
- [Tracked TODOs](TODO.md)

Portway is licensed under the MIT License.
