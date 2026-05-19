# portex (Rust workspace)

High-throughput, low-resource tunnelling gateway and CLI. Replaces the Django
proxy + PyOxidizer connector on the data path.

## Layout

```
crates/
  common/    Wire protocol (HELLO/ACCEPT/REJECT frames, codec).
  gateway/   Server binary: QUIC tunnel + public HTTP ingress.
  cli/       Client binary `portex` that runs on user machines.
```

## Data path

```
public HTTP/1.1  ──► gateway ingress ─┐
                                      │  open new bi QUIC stream
                                      ▼
gateway ◄──── QUIC (one connection, many streams) ────► CLI ◄──► local app
```

Two crucial properties:

- **No JSON, no base64.** The control channel (handshake, pings) uses tiny
  binary frames; per-request streams carry raw HTTP/1.1 bytes untouched.
- **No buffering.** `tokio::io::copy` pipes bytes both ways. Uploading 10 MB
  costs the gateway a few KB of RAM, not 10 MB.

## Build

```
cd portex_rust
cargo build --release
```

Outputs:
- `target/release/portex-gateway` (server)
- `target/release/portex` (client)

## Run locally

Terminal 1 — gateway:
```
PORTEX_BASE_DOMAIN=localtest.me \
PORTEX_HTTP_ADDR=0.0.0.0:8080 \
PORTEX_TUNNEL_ADDR=0.0.0.0:4443 \
./target/release/portex-gateway
```
(`localtest.me` resolves `*.localtest.me` → `127.0.0.1`, useful for tests.)

Terminal 2 — local app on port 3000 (e.g. `python -m http.server 3000`).

Terminal 3 — client:
```
./target/release/portex auth dev-token        # dev mode skips real auth
./target/release/portex http -s acme -p 3000 \
    --server 127.0.0.1:4443 --insecure
```

Terminal 4 — request:
```
curl -H 'Host: acme.localtest.me:8080' http://127.0.0.1:8080/
```

## Auth (jprq-style)

Django (control plane) owns user accounts and issues tokens.

1. User signs up on `portex.live`.
2. Dashboard → "Generate token". Plaintext shown once; SHA-256 hash stored.
3. On token creation, Django writes `token:{hash} → user_id` to Redis.
4. User reserves a subdomain. Django writes `sub:{name} → user_id` to Redis.
5. CLI sends `HELLO { subdomain, token }` over QUIC. Gateway validates both
   keys against Redis. No DB round-trip on the hot path.

Until Redis is wired up, the gateway runs in **dev mode**: any token is
accepted (a warning is logged at startup).

## Protocol summary

Control stream frames (one bi-directional QUIC stream opened by the client):

```
[u8 type][u32 length BE][payload]

type:
  0x01 HELLO   = client → server
  0x02 ACCEPT  = server → client
  0x03 REJECT  = server → client
  0x04 PING
  0x05 PONG
```

Per-request streams are opened by the server with `open_bi()`; their content
is the verbatim HTTP/1.1 wire bytes in each direction. The CLI just pipes
them to `127.0.0.1:<port>`.

## ACME wildcard TLS (production)

The gateway can obtain and auto-renew a Let's Encrypt wildcard cert for
`*.{apex}` + `{apex}` using DNS-01 challenges against Cloudflare.

Required env:

```
PORTEX_ACME_DOMAIN=portex.live
PORTEX_ACME_EMAIL=ops@portex.live
CLOUDFLARE_API_TOKEN=<token with Zone:DNS:Edit>
CLOUDFLARE_ZONE_ID=<zone uuid>
PORTEX_STATE_DIR=/var/lib/portex
PORTEX_ACME_STAGING=true   # use staging endpoint while testing
```

When all four ACME vars are set, the gateway:
1. Validates the Cloudflare token at startup.
2. Reuses `state_dir/cert.pem` + `key.pem` if they're still valid (> 30 days).
3. Otherwise runs a fresh ACME order, publishes `_acme-challenge` TXT records
   via the Cloudflare API, polls for validation, persists the new cert.
4. Spawns a renewal task that wakes every 12 h and re-runs the flow once the
   cert is within the 30-day renewal window. The new cert is hot-swapped
   into both the HTTPS listener (per-accept `TlsAcceptor` rebuild) and the
   QUIC endpoint (`set_server_config`) — no restart required.

`SIGHUP` triggers the same reload using the on-disk `--tls-cert` / `--tls-key`
files, useful if cert rotation comes from an external process.

For local development the cert files (or auto-generated self-signed) work fine
without any of the ACME variables.

## Metrics

```
PORTEX_METRICS_ADDR=127.0.0.1:9090
```

Bind on a private network — `/metrics` has no auth. Exposes:

- `portex_active_tunnels` (gauge)
- `portex_tunnel_connects_total`, `portex_tunnel_disconnects_total` (counters)
- `portex_requests_total`, `portex_request_errors_total` (counters)
- `portex_bytes_upstream_total`, `portex_bytes_downstream_total` (counters)

## Roadmap

- [x] Workspace scaffold
- [x] Wire protocol + codec (round-trip tests)
- [x] Gateway: QUIC listener, handshake, in-memory registry
- [x] Gateway: HTTP + HTTPS ingress, Host parsing, byte splice
- [x] CLI: connect, handshake, per-stream forwarding, QUIC keepalive
- [x] Redis-backed auth + subdomain reservation (Django source of truth)
- [x] ACME wildcard TLS (Cloudflare DNS-01)
- [x] Prometheus metrics endpoint
- [x] Hot cert reload (ArcSwap + SIGHUP + ACME renewal)
- [ ] Additional DNS providers (Route53, DigitalOcean)
- [ ] Graceful shutdown with in-flight request draining
- [ ] Multi-instance routing (Redis pub/sub for connection placement)
- [ ] Bench harness vs current Django proxy
