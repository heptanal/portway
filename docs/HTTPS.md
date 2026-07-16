# HTTPS deployment

Portway supports native HTTPS with a PEM certificate chain and private key. It
does not create or distribute a trusted certificate automatically. Controller
devices must trust the issuing CA, and the certificate subject alternative names
must cover the exact hostname or IP address opened in the browser.

## Configure native TLS

Store the files where the unprivileged service user can read them. With the
sample systemd unit, `/var/lib/portway/tls` is suitable:

```toml
tls_cert = "/var/lib/portway/tls/cert.pem"
tls_key = "/var/lib/portway/tls/key.pem"
```

Both values are required together. Portway fails closed if the certificate or
key cannot be parsed. When TLS is enabled, startup URLs use `https://`, WebSocket
connections use `wss://`, session cookies have `Secure`, and responses include
HSTS. Portway does not also open an HTTP listener or redirect HTTP traffic.

Use a certificate from a private CA already trusted by the phone, such as a
managed home-lab CA. Copy only the server certificate chain and private key to
the Linux host, make the key readable only by the Portway service user, and keep
the CA private key elsewhere.

```sh
sudo install -d -m0700 -o portway -g portway /var/lib/portway/tls
sudo install -m0644 -o portway -g portway cert.pem /var/lib/portway/tls/cert.pem
sudo install -m0600 -o portway -g portway key.pem /var/lib/portway/tls/key.pem
sudo systemctl restart portway
```

Opening an IP address requires that IP in the certificate SAN. A DNS-only
certificate is not valid for `https://192.168.1.42:2721`.

## Local smoke test

The repository helper creates a seven-day self-signed certificate. It is for
protocol testing with verification explicitly bypassed, not normal phone use:

```sh
scripts/generate-test-cert .portway-tls localhost 127.0.0.1
portway serve --backend mock \
  --token-file /tmp/portway-test-token \
  --listen 127.0.0.1 \
  --tls-cert .portway-tls/cert.pem \
  --tls-key .portway-tls/key.pem
curl --insecure --include https://localhost:2721/healthz
```

Do not train users to bypass certificate warnings for remote control. A browser
may refuse `wss://` when the certificate is untrusted even after the page warning
is bypassed. Use a trusted CA for interactive controller testing.

## Reverse proxies

Native HTTPS is the smallest deployment. A reverse proxy is also viable when it:

- forwards WebSocket upgrades without changing the request `Host`;
- terminates TLS with a certificate trusted by the controller;
- does not log cookies or pairing request bodies;
- does not cache Portway responses;
- restricts access to intended LAN or VPN interfaces; and
- configures its public HTTPS origin in `allowed_origins` only when `Host` cannot
  be preserved.

Do not expose an unencrypted proxy-to-Portway hop across another machine or
untrusted network. Binding Portway to `127.0.0.1` is appropriate when the proxy
runs on the same Linux host.
