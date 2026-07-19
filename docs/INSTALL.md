# Installation and lifecycle

Portway provides two supported installation paths:

- conventional systemd Linux uses the guided `scripts/install-linux` and
  `scripts/uninstall-linux` commands;
- NixOS uses the included flake/module and manages the service through a
  normal system rebuild.

The result is a dedicated unprivileged `portway` service, enabled at boot, with
`uinput` loaded during boot and a narrow `0660` udev rule. The service restarts
after unexpected exits. Portway does not run its network listener as root.

## Conventional systemd Linux

From a release archive, place the `portway` binary beside the repository's
`scripts` and `packaging` directories. From source, build first:

```sh
cargo build --release --locked
scripts/install-linux
```

The installer uses `sudo` when needed and asks for an exposure profile:

| Profile | Listener | Transport | Intended use |
| --- | --- | --- | --- |
| `lan` | `0.0.0.0` | HTTP + authentication | Trusted home LAN; least friction |
| `local` | `127.0.0.1` | HTTP + authentication | Same-host TLS proxy, VPN agent, or SSH tunnel |
| `https` | `0.0.0.0` | Native HTTPS + authentication | LAN with an existing certificate trusted by controllers |

The `lan` profile is the unattended default because it immediately works from a
phone, but the installer prints the unencrypted-transport warning. Authentication
remains enabled in every profile by default. Disabling it requires both explicit
flags:

```sh
scripts/install-linux --profile lan --auth disabled --accept-risk
```

The user who invokes the installer through `sudo` is authorized to run
`portway pair` without privilege elevation. A root-run or unattended installer
can select that account explicitly with `--pair-user USER`.

Useful unattended examples:

```sh
scripts/install-linux --yes --profile lan
scripts/install-linux --yes --profile local --firewall skip
scripts/install-linux --yes --profile https \
  --tls-cert /path/to/cert.pem \
  --tls-key /path/to/key.pem
```

`ufw` or `firewalld` is opened only when it is detected as active and the rule is
absent. Use `--firewall skip` to manage policy yourself. Other nftables-based or
distribution firewalls are reported but not modified.

The HTTPS profile copies a snapshot of the certificate and key into
`/var/lib/portway/tls`. Renewed source files are not copied automatically yet;
rerun the installer with `--force-config` or update those files and restart the
service. See [HTTPS.md](HTTPS.md). Every installer, uninstaller, and application
flag is listed in [CLI.md](CLI.md).

## Pair another controller

The installer prints a temporary six-digit code after startup. Generate another
at any time without restarting Portway:

```sh
portway pair
```

The command automatically discovers `/etc/portway/config.toml` and asks the
running service for a code through `/run/portway/pair.sock`. The service verifies
the caller's kernel-reported user ID before printing the code; the caller never
gains access to the setup token or protected pairing record. Root, the service
account, and UIDs listed in `pairing_allowed_uids` are accepted. Open the Portway
website and enter the code in the pairing dialog. It expires after five minutes
by default and is accepted once; generating another invalidates the previous
one. `portway token` remains an explicit privileged recovery operation.

## Upgrade and reconfigure

Build or obtain the new binary, then rerun the installer. It replaces the binary,
unit, udev rule, and modules-load file but preserves `/etc/portway/config.toml`
and `/var/lib/portway` by default:

```sh
cargo build --release --locked
scripts/install-linux --yes
```

Use `--force-config` only when intentionally replacing the configuration. The
previous file is saved as `/etc/portway/config.toml.bak`. Upgrading an older
installation adds the pairing-socket settings when absent and makes the
non-secret system configuration readable for command discovery; credentials
under `/var/lib/portway` remain owner-only.

Inspect operation with:

```sh
systemctl status portway.service
journalctl -u portway.service -f
curl -i http://localhost:2721/healthz
```

## Uninstall

Normal uninstall removes executable behavior but retains secrets and settings for
a later reinstall:

```sh
scripts/uninstall-linux
```

Permanent removal requires an explicit purge confirmation:

```sh
scripts/uninstall-linux --purge
```

The uninstaller removes only a firewall rule that the installer recorded as
created. A normal uninstall preserves the service account and state; `--purge`
removes the account/group only if the installer recorded creating them. The
uninstaller does not unload `uinput`, which another local application may still
be using.

## Availability limits

The installed service starts during boot, waits for networking, automatically
restarts, and does not depend on a logged-in desktop process. This makes Portway
available whenever the machine is powered on, Linux is running, the network path
is reachable, and the desktop or login environment accepts the virtual devices.

Portway does not implement Wake-on-LAN, power-on, suspend wake-up, network repair,
or a guarantee that a compositor/display manager accepts input at a lock or login
screen. Test those policies on the actual machine before relying on Portway as the
only access method.
