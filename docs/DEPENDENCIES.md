# Dependency policy

Portway targets the current project toolchain, Rust 1.97.0. `Cargo.lock` is
committed and documented build commands use `--locked` for reproducibility.
The Nix flake locks both nixpkgs and `oxalica/rust-overlay`; the overlay is used
only to obtain that exact Rust toolchain when the distribution package set lags.

Core choices are limited to mature ecosystem components:

| Component | Purpose | Reason for inclusion |
| --- | --- | --- |
| Tokio | async runtime | Linux/macOS support, cancellation and networking primitives |
| Axum | HTTP/WebSocket | typed extractors on Tokio/Hyper with bounded WebSocket frames |
| axum-server / Rustls + Ring | optional native HTTPS | maintained Axum integration, modern TLS, lower build memory, no system OpenSSL runtime dependency |
| Serde / serde_json | protocol | explicit, deny-unknown-field schemas |
| Clap | CLI/environment | consistent parsing and documented options |
| tracing | logs | structured fields without request-query logging |
| libc (Linux only) | uinput | smallest practical boundary for required ioctls |

Small support crates provide error context, TOML parsing, constant-time setup
token comparison, base64, and OS randomness. Controller sessions remain
server-side and require no session framework or database. The browser controller
has zero external dependencies and no CDN.

Before releases, maintainers should run:

```sh
cargo update
cargo tree --duplicates
cargo deny check
```

`cargo-deny` is intentionally not a build dependency. Adding its policy and CI
installation is tracked in `TODO.md`. New dependencies require an active upstream,
a compatible permissive license, a clear security purpose, and evidence that a
small direct implementation would be worse.
