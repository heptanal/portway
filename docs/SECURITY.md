# Security notes

Portway grants remote control of a machine. Treat its setup token, controller
sessions, TLS private key, and uinput group membership like credentials.

## Defaults

- Authentication is enabled and a 256-bit setup token is generated on first use.
- The setup-token file is created with owner-only permissions (`0600` on Unix).
- `portway pair` prints a random six-digit, single-use code that expires in five minutes.
- Local code requests are authorized from kernel-reported Unix peer user IDs.
- Browsers receive a random `HttpOnly`, `SameSite=Strict` session cookie.
- The server listens on `0.0.0.0:2721` for LAN usability.
- One controller is permitted at a time.
- Pairing, logout, and WebSocket origins must match the request host.
- Pairing bodies are bounded and attempts are limited per source address.
- Commands are schema/range checked, bounded, sequenced, and rate limited.
- Held input is released on all normal and abnormal session endings.

Binding to `0.0.0.0` exposes the login page and WebSocket port to every reachable
network interface. Authentication prevents unauthenticated control but HTTP does
not prevent passive capture or an active network attacker from stealing a pairing
code, session cookie, or input. Use only a trusted LAN, bind a specific interface,
apply a host firewall, or enable native HTTPS. See [HTTPS.md](HTTPS.md).

Authentication can be disabled only by an explicit configuration or CLI choice.
That mode is intended for isolated test environments and logs a prominent warning.

## Pairing and token handling

The persistent setup token and pairing code never appear in an HTTP or WebSocket
URL. `portway pair` connects to a local Unix socket, normally
`/run/portway/pair.sock`, and the running service checks the caller's UID using
kernel-supplied peer credentials. UID 0, the socket-owning service account, and
UIDs explicitly listed in `pairing_allowed_uids` may request a code. The socket
path is connectable by local users so the service can perform that credential
check; connecting alone grants no code or token access. The installer authorizes
the invoking desktop user by default.

After authorization, the service generates a uniformly random six-digit code and
writes a `0600` record beside the token, normally
`/var/lib/portway/token.pairing`. The record contains the exact expiry and an
HMAC-SHA256 of the expiry and code, not the six-digit plaintext. The CLI receives
only the temporary code and never reads the persistent setup token or pairing
record.

Only one pairing code is outstanding. Issuing another atomically replaces the
record and invalidates the previous code. A successful exchange deletes the
record before creating the session, making the code single-use across server
restarts. Expired records are deleted when checked. The browser receives a
12-hour memory-backed session by default; server restart invalidates all sessions
but does not make a consumed code reusable.

Run `portway token` as the service user to create or display the recovery token
deliberately. Entering it in the browser exchanges it for a session; the
credential is handled transiently by the page but is not persisted in browser
storage. Stop Portway before replacing the token file, then restart so the
in-memory credential and all sessions are replaced together.

Six decimal digits provide one million possible codes, so online rate limiting
is an essential part of the design. Pairing accepts eight total attempts per
source IP in a one-minute window. Successful pairing clears that address's
attempt window. Portway keeps at most
32 authenticated browser sessions in memory, while `max_clients` separately
limits simultaneous controllers. Portway logs authentication outcomes but not
submitted credentials, session values, typed text, or individual input events.

## Normal authenticated login flow

1. The operator starts Portway with token authentication, then runs
   `portway pair`. Installed systems automatically select
   `/etc/portway/config.toml`. The command connects to the local pairing socket;
   after authorizing its UID, the running service replaces any outstanding
   pairing record and returns a six-digit code.
2. The controller opens the Portway website. Static controller assets are public,
   so the pairing dialog can load before authentication. The user enters the
   six-digit code shown by Portway. A rejected or expired code leaves the dialog
   open and displays an inline error associated with the code field.
3. The page sends the code to `POST /api/pair`. The recovery setup
   token can be submitted through the same form. The request must have an
   accepted `Origin`, fit the body/credential bounds, and pass the per-IP
   pairing limiter.
4. The server verifies the code against the expiry and HMAC in the protected
   pairing record, then deletes that record. On success, it creates a random,
   fixed-lifetime in-memory session and returns a `portway_session` cookie with
   `HttpOnly`, `SameSite=Strict`, and `Max-Age`; HTTPS also adds `Secure`.
5. The page opens `/ws`. The browser attaches the cookie automatically; the
   WebSocket URL contains no credential. Portway checks the origin, session, and
   simultaneous-controller limit before upgrading, then sends `ready`.
6. On a later visit, `GET /api/session` lets the page reuse a still-valid cookie
   without pairing again. Activity and heartbeats do not extend session expiry.
   Tapping connection status posts to `/api/session/logout`, revokes that session,
   expires the cookie, and returns to the pairing dialog. Server restart clears
   every in-memory session.

HTTPS sets `Secure` on the session cookie and HSTS on responses. HTTP omits
`Secure` only so isolated development remains possible; that does not make HTTP
safe on an untrusted network.

## uinput access

The sample udev rule grants the `portway` group read/write access to uinput; it
does not make the device world-writable. Any member able to open uinput can inject
input, potentially at login screens or into other sessions depending on system
policy. Keep group membership narrow.
