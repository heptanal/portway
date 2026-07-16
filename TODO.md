# Portway TODO

## Next milestone

- Test device recognition and all key codes on representative Wayland and X11 hosts.
- Add automatic TLS certificate reload and a documented local-CA provisioning workflow.
- Test the guided installer/uninstaller across Debian, Fedora, Arch, and openSUSE.
- Produce signed release archives containing the binary, installer, service files, and checksums.
- Add configurable keyboard layout modules beyond US ANSI.
- Add compositor-aware Unicode text input as a separate optional backend.
- Improve multi-controller ownership with global key/button reference counting.
- Add QR rendering without an external service.
- Add packaging for common Linux distributions.
- Add CI across x86_64/aarch64 Linux and macOS.
- Add a committed `cargo-deny` policy and license/advisory CI check.
- Perform an accessibility pass with VoiceOver and TalkBack.

## Real hardware validation

- Repeat the passing `/dev/uinput` integration test across additional kernels and architectures.
- Verify login-screen and lock-screen behavior without making support claims.
- Verify horizontal wheel support across common applications.
- Measure touch latency and tune coalescing/rate limits on low-end phones.
