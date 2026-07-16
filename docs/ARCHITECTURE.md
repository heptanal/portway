# Portway architecture

## Stack decision

Portway uses the pinned current Rust 1.97 toolchain with Tokio, Axum, Serde, and tracing. The controller is
plain HTML, CSS, and JavaScript embedded with `include_str!`. This gives the
project one deployable binary, no runtime asset or package-manager dependency,
strong message validation, and a network stack that remains testable on macOS.

The dependency set is deliberately small and consists of widely used,
permissively licensed Rust libraries. Linux device access uses a direct `libc`
wrapper instead of a large input abstraction. That wrapper is the only module
allowed to contain unsafe code and is compiled only on Linux.

## Layers

1. `config` resolves defaults, a TOML file, environment variables, and CLI flags.
2. `auth` owns the `0600` setup token, signed one-use pairing codes, and sessions.
3. `protocol` defines the versioned WebSocket schema and validation bounds.
4. `input` defines `InputBackend`, US key mapping, recording and unavailable backends.
5. `input::linux` owns `/dev/uinput` file descriptors and ioctl/event details.
6. `session` applies validated commands, tracks held state, rate-limits clients,
   and guarantees session-scoped cleanup.
7. `server` performs HTTP(S)/WebSocket lifecycle, pairing/session/origin checks,
   controller admission, heartbeat expiry, security headers, and asset serving.
8. `web` is the phone-first controller and contains no trusted policy decisions.

The HTTP and WebSocket layers never call ioctl. `InputBackend` is the boundary:

```text
browser -> strict protocol -> session state -> InputBackend -> uinput or recorder
```

## Process and privilege model

The initial release is one unprivileged process. A udev rule grants `/dev/uinput`
to members of a dedicated `portway` group. This avoids exposing a root network
service and is simpler to audit than a privileged helper. Membership in that
group is security-sensitive: it effectively permits synthetic input across the
machine. The interface permits a future Unix-socket helper without changing the
protocol or server.

Running `sudo portway serve` works for initial testing but is not the recommended
service configuration because both the listener and token file then run as root.

## Lifecycle and state

The default controller limit is one. Every session tracks its own keys and mouse
buttons. Disconnect, heartbeat timeout, browser inactivity, explicit reset, and
server shutdown all release held state. The backend independently tracks state
and releases everything during final shutdown and device teardown.

The server remains available in a degraded state if automatic uinput setup
fails, allowing diagnostics and non-root tests. Selecting `backend = "uinput"`
explicitly makes initialization failure fatal; `backend = "mock"` records events.

## Text behavior

Protocol key events identify physical, layout-independent keys. The initial
`text_input` path accepts printable US-ASCII plus newline and tab, translates it
using a modular US keyboard map, and preserves the prior Shift state. Arbitrary
Unicode and non-US layouts are not claimed. They require a future layout module
or compositor-specific text protocol.

## Security boundaries

Static assets are public. HMAC-signed pairing codes carry an expiry and nonce, so
the local CLI can create them from the protected setup secret without an IPC
service or server restart. The server retains a bounded replay set until expiry.
Pairing exchanges a code or the constant-time-checked recovery token for a random
server-side session. The browser receives only an `HttpOnly`, `SameSite=Strict`
cookie; WebSocket upgrades require that session and never accept URL credentials.
Pairing, logout, and WebSocket requests must have an HTTP(S) `Origin` whose
authority matches `Host`, unless the origin is explicitly allow-listed.

Native HTTPS uses Rustls with an operator-provided certificate and key. TLS state
remains in the server lifecycle and does not cross the `InputBackend` boundary.
HTTP remains available for isolated development and trusted-LAN migration but is
explicitly logged as unencrypted.

Messages are capped at 4 KiB, deny unknown fields, are sequence checked, range
validated, and rate limited. Portway never offers shell, filesystem, plugin, or
HTML-rendering operations.

## Testing strategy

Unit tests exercise config precedence, schemas, key mapping, ordering, cleanup,
and rate limiting. An in-process integration test uses the recording backend and
a real WebSocket. Linux compilation and tests run on a real Linux host. The
ignored uinput test is opt-in and requires permission to access the device.
