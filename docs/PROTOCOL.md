# Control protocol v1

The control channel is a WebSocket at `/ws`. Authentication uses the
`portway_session` cookie; origin and session validation happen before HTTP
upgrade. Persistent setup tokens and temporary pairing codes are never accepted
in the WebSocket URL. Text frames only are accepted, with a maximum decoded size
of 4096 bytes.

The browser obtains a session through `POST /api/pair` with a strict JSON body:

```json
{"code":"temporary-code-or-setup-token"}
```

The request body is limited to 512 bytes and must have a same-host or explicitly
allowed `Origin`. Eight failed attempts per source IP are accepted per minute.
A valid temporary code is HMAC-signed by the setup secret, time-bounded, and
consumed once. Success sets an expiring `HttpOnly`, `SameSite=Strict` cookie;
HTTPS additionally sets `Secure`. `GET /api/session` reports browser session
state, and `POST /api/session/logout` revokes it.

Every client message is a strict JSON envelope:

```json
{"v":1,"seq":12,"event":{"type":"pointer_move","dx":8,"dy":-3}}
```

`seq` must increase within a connection. The event variants are:

| Type | Fields | Meaning |
| --- | --- | --- |
| `pointer_move` | `dx`, `dy` integers | Relative movement, each -2048..2048 |
| `pointer_button` | `button`, `state` | `left/right/middle`, `down/up` |
| `scroll` | `dx`, `dy` integers | Horizontal/vertical wheel, each -120..120 |
| `key` | `code`, `state` | Named physical key, `down/up` |
| `text_input` | `text` | At most 128 printable US-ASCII/newline/tab characters |
| `release_all` | none | Release all backend state and clear this session |
| `heartbeat` | none | Keep the controller session alive |
| `client_state_reset` | none | Alias for safety cleanup after client state loss |

Unknown event types, fields, key names, and enum values are rejected. A malformed
or non-monotonic message closes the connection after a structured error.

Server messages also use `v: 1` and a tagged `type`. `ready` includes backend
availability and the configured sensitivity; `pong` acknowledges a heartbeat;
`error` contains a stable code and safe description.

Key names include `key_a` through `key_z`, `digit_0` through `digit_9`, US
punctuation, modifiers, navigation, F1-F12, and common volume/playback controls.
The exact enum in `src/protocol.rs` is authoritative.
