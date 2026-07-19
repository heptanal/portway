# NixOS development and installation

Portway includes a NixOS package and service module. The generic installer
intentionally refuses NixOS because files written imperatively into `/usr/local`,
`/etc/systemd`, or `/etc/udev` would not be a reliable declarative installation.

## Declarative service

The recommended path is the included flake. It builds with the latest stable
Rust release exposed by `rust-overlay`. Add Portway to the host flake inputs and
module list:

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
            pairingAllowedUids = [ 1000 ];

            # Prefer a trusted LAN interface instead of every interface.
            firewallInterfaces = [ "wlp2s0" ];
          };
        }
      ];
    };
  };
}
```

Then rebuild through the host's normal flake or channel workflow:

```sh
sudo nixos-rebuild switch --flake /etc/nixos#your-host
```

The source flake can be evaluated and built independently before changing the
host configuration:

```sh
nix flake check --no-build path:/path/to/Portway
nix build path:/path/to/Portway#portway
```

The module:

- builds the locked Portway source;
- creates a dedicated `portway` system user/group;
- loads `uinput` during boot;
- installs the `0660` udev rule;
- writes a generated non-secret configuration at `/etc/portway/config.toml`;
- stores the setup token under `/var/lib/portway`;
- exposes a UID-authorized local pairing socket under `/run/portway`;
- enables a hardened, automatically restarting system service; and
- optionally opens only the selected firewall interfaces.

Generate a temporary six-digit pairing code after the service starts, then enter
it in the Portway website:

```sh
portway pair
```

Set `services.portway.pairingAllowedUids` to the numeric UIDs of trusted local
operators. Root and the `portway` service UID are accepted automatically. The
command discovers `/etc/portway/config.toml`; no `--config` or `sudo` is needed
for an allow-listed user.

## Exposure choices

Authenticated LAN HTTP with an interface-scoped firewall rule:

```nix
services.portway = {
  enable = true;
  listenAddress = "0.0.0.0";
  firewallInterfaces = [ "wlp2s0" ];
};
```

Localhost-only for a same-host reverse proxy, VPN agent, or SSH tunnel:

```nix
services.portway = {
  enable = true;
  listenAddress = "127.0.0.1";
};
```

Native HTTPS uses runtime string paths so the private key is not copied into the
world-readable Nix store:

```nix
services.portway = {
  enable = true;
  firewallInterfaces = [ "wlp2s0" ];
  tlsCertificate = "/var/lib/portway/tls/cert.pem";
  tlsPrivateKey = "/var/lib/portway/tls/key.pem";
};
```

Install those runtime files separately with owner `portway`, directory/key mode
`0700`/`0600`, and certificate mode `0644`. Controller devices must trust the
issuing CA. See [HTTPS.md](HTTPS.md).

`openFirewall = true` opens the port globally. Prefer `firewallInterfaces` when
the LAN device name is stable. Authentication defaults to `token`; setting
`authMode = "disabled"` is an explicit security downgrade.

## Plain module and package overrides

It is also possible to import `packaging/nixos/portway.nix` directly. Its default
package then uses the importing nixpkgs `rustPlatform`. Use the flake above to
build with the latest stable Rust release, or override
`services.portway.package` with an equivalent derivation:

```nix
services.portway.package = myPackages.portway;
```

## Upgrade and uninstall

Update the source/package input and rebuild to upgrade. Disable and rebuild to
uninstall executable behavior:

```nix
services.portway.enable = false;
```

NixOS retains `/var/lib/portway` because it contains credentials. Delete that
directory manually only when intentionally purging pairing state.

## Development and real-device test

Use the flake's development shell for the latest stable Rust release:

```sh
nix develop path:/path/to/Portway -c cargo test --all-targets --locked
nix develop path:/path/to/Portway -c cargo build --release --locked
```

After the module has applied uinput access, the opt-in real-device test is:

```sh
nix develop path:/path/to/Portway -c cargo test \
  input::linux::tests::creates_real_uinput_devices -- --ignored --nocapture
```

The build test proves event-device creation and writes. Recognition by a desktop,
display manager, or lock screen remains a separate host policy check.
