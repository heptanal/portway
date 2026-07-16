# Security notes

Portway grants remote control of a machine. Treat its setup token, controller
sessions, TLS private key, and uinput group membership like credentials.

## Defaults

- Authentication is enabled and a 256-bit setup token is generated on first use.
- The setup-token file is created with owner-only permissions (`0600` on Unix).
- Startup pairing links contain an HMAC-signed, single-use code that expires in five minutes.
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

The persistent setup token never appears in automatic startup URLs. Each pairing
code contains an expiry and random nonce authenticated with HMAC-SHA256 by the
setup secret. It expires and is consumed by its first successful exchange. This
allows the local `portway pair` command to issue a URL without restarting or
contacting the server, while the persistent secret never leaves its `0600` file.
The running server remembers used codes until expiry to reject replay. The browser
removes the code from the address bar before exchange and receives a 12-hour
memory-backed session by default. Server restart invalidates all sessions.

The replay cache is deliberately memory-only. Restarting the server during a
code's short validity window also resets that cache, so an already exchanged code
could be accepted once by the new process until it expires. Use HTTPS anywhere a
network peer could observe the temporary URL.

Run `portway token` as the service user to create or display the recovery token
deliberately. Entering it in the browser exchanges it for a session; the
credential is handled transiently by the page but is not persisted in browser
storage. Stop Portway before replacing the token file, then restart so the
in-memory credential and all sessions are replaced together.

Pairing accepts eight total attempts per source IP in a one-minute window.
Successful pairing clears that address's attempt window. Portway keeps at most
32 authenticated browser sessions in memory, while `max_clients` separately
limits simultaneous controllers. Portway logs authentication outcomes but not
submitted credentials, session values, typed text, or individual input events.

## Normal authenticated login flow

1. The operator starts Portway with token authentication. Portway loads or
   creates the persistent setup token and issues a signed, short-lived pairing
   code for its startup URLs. `portway pair` can issue another code later from
   the same protected token without contacting or restarting the server.
2. The controller opens a pairing URL. Static controller assets are public, so
   the page can load before authentication. JavaScript reads the `pair` query
   value and removes it from the visible address before submitting it.
3. The page sends the temporary code to `POST /api/pair`. The recovery setup
   token can be submitted through the same form. The request must have an
   accepted `Origin`, fit the body/credential bounds, and pass the per-IP
   pairing limiter.
4. The server verifies the credential. A pairing code must have a valid HMAC and
   expiry and must not already be in the running process's replay cache. On
   success, the server creates a random, fixed-lifetime in-memory session and
   returns a `portway_session` cookie with `HttpOnly`, `SameSite=Strict`, and
   `Max-Age`; HTTPS also adds `Secure`.
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
