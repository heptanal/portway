# Troubleshooting

## The page does not open from a phone

Confirm startup says `0.0.0.0:2721` (or the intended LAN interface), then check:

```sh
ss -ltnp | grep 2721
ip -brief address
curl -i http://localhost:2721/healthz
```

The phone and Linux host must have a route to one another. Guest Wi-Fi often
enables client isolation. Permit TCP 2721 in the host firewall only for the
trusted LAN. A listener bound to `127.0.0.1` is intentionally unreachable from
other devices.

## Authentication repeatedly fails

Generate a fresh temporary URL without restarting the service:

```sh
sudo -u portway portway --config /etc/portway/config.toml pair
```

It expires after five minutes by default and can be used only once by the running
server. For recovery, run `portway token` as the same user and with the same
`--config` path as the server, then enter that setup token in the pairing dialog.
Tokens under root's home differ from the service user's token. Tap connection
status to revoke the current browser session and pair again. Server logs record
attempt outcomes but not credential values.

## HTTPS certificate errors

The certificate SAN must match the exact hostname or IP in the browser, and its
issuer must be trusted by the controller device. Portway intentionally does not
fall back to HTTP when certificate loading fails. Test the endpoint with:

```sh
curl --cacert /path/to/ca.pem -i https://localhost:2721/healthz
```

The repository's generated self-signed certificate is only for `curl --insecure`
smoke tests. See [HTTPS.md](HTTPS.md) for deployment guidance.

## `/dev/uinput` is missing

```sh
sudo modprobe uinput
ls -l /dev/uinput
```

If loading succeeds, configure the sample udev rule. Some kernels build uinput as
a module and others include it directly. An environment without the device is
expected to use the mock backend.

## Permission denied opening uinput

```sh
id
stat -c '%A %U %G %n' /dev/uinput
getent group portway
```

The device should be group `portway`, mode `0660`, and the service/user should be
in that group. Log out after `usermod`; a new shell alone may retain old groups.
Do not solve this with mode `0666`.

## The backend is ready but input does nothing

Check `libinput list-devices` or `/proc/bus/input/devices` for the two Portway
devices. Recognition is compositor/display-manager policy, not proof of a
Portway protocol failure. Test in an unlocked desktop session first. Headless
systems may have no consumer for injected events.

## A key appears stuck

Use the red **Release all** control. If the controller is unavailable, stop
Portway with SIGTERM so clean shutdown releases state and destroys both virtual
devices. A hard process kill or kernel/device failure cannot run userspace cleanup;
device destruction normally causes the input stack to discard its state.

## Text characters are rejected

The initial text path is US-ASCII only. Use raw special-key controls for
navigation and modifiers. Unicode and non-US desktop layouts are tracked future
work rather than being silently mistranslated.
