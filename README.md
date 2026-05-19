# portex

Self-hosted tunnelling service — expose any local TCP/HTTP server to the
public internet through a single subdomain, with a Rust gateway on the hot
path and a Django control plane for accounts.

```
┌──────────────────┐        ┌──────────────────┐
│  Public client   │  HTTPS │ portex-gateway   │
│  (browser/curl)  │ ─────► │  (Rust + QUIC)   │
└──────────────────┘        └────────┬─────────┘
                                     │ QUIC bi-stream
                                     │ raw HTTP bytes
                                     ▼
                            ┌──────────────────┐
                            │  portex (CLI)    │
                            │  on user machine │
                            └────────┬─────────┘
                                     │ HTTP
                                     ▼
                            ┌──────────────────┐
                            │ Local app :PORT  │
                            └──────────────────┘
```

- **Hot path** (proxy + tunnel) = Rust (`tokio` + `quinn` + `hyper`)
- **Cold path** (landing + dashboard + admin + auth state) = Django + Postgres + Redis
- **Wire protocol** = binary framed control + raw HTTP byte splice — no
  JSON, no base64, no buffering. Streams 10 MB requests with ~50 KB RAM.

## Highlights

- **Tiny binaries.** `portex` CLI is **2.5 MB** statically linked (vs. 87 MB
  PyOxidizer bundle previously). Gateway is **4.7 MB**.
- **High throughput.** ~2,500 RPS on a single machine through the tunnel
  (lab benchmark — only the tunnel layer, local app was the bottleneck);
  100 MB/s+ on large payloads.
- **ACME wildcard TLS** built in (Cloudflare DNS-01) with hot reload via
  `ArcSwap` — no restart on cert renewal.
- **jprq-style auth**: per-user API tokens, reserved subdomains, validated
  against Redis on the hot path. Django is the source of truth.
- **Prometheus `/metrics`** for active tunnels, request counts, byte
  counters.

---

## Quickstart (local, no domain required)

Prerequisites: Docker Desktop, Rust toolchain (`curl -sSf https://sh.rustup.rs | sh`).

```bash
git clone https://github.com/Khasan712/portex-v2.git
cd portex-v2

# 1) Build the Rust binaries
cd portex_rust && cargo build --release && cd ..

# 2) Bring up the stack (Django + Postgres + Redis + gateway + celery)
docker compose up -d --build

# 3) Apply migrations + create an admin user
docker compose exec django python manage.py migrate
docker compose exec django bash -c "
  DJANGO_SUPERUSER_USERNAME=admin DJANGO_SUPERUSER_EMAIL=admin@portex.local \
  DJANGO_SUPERUSER_PASSWORD=pass \
  python manage.py createsuperuser --noinput
"

# 4) Smoke-test everything
./test_e2e.sh
```

Expected output of the smoke test:

```
✓ 5 containers running
✓ Django landing
✓ Dashboard requires login
✓ Metrics endpoint
✓ Gateway 502 for unknown subdomain
✓ Token issued for john
✓ Local app on :4444
✓ Tunnel established
✓ Tunneled request
✓ Wrong token correctly rejected
✓ Other user's subdomain correctly blocked
```

---

## What's where

```
portex/
├── compose.yml              ← full stack (5 services)
├── test_e2e.sh              ← automated end-to-end smoke test
├── DEPLOY.md                ← production deployment runbook
├── portex_rust/             ← Rust workspace
│   ├── Cargo.toml
│   ├── Dockerfile
│   ├── README.md            ← gateway/CLI internals
│   └── crates/
│       ├── common/          ← wire protocol (HELLO/ACCEPT/REJECT)
│       ├── gateway/         ← server binary: QUIC + HTTP + HTTPS + metrics + ACME
│       └── cli/             ← user-facing `portex` binary
└── portex_server/portex/    ← Django control plane
    ├── manage.py
    ├── entrypoint.sh        ← gunicorn+gevent
    ├── Dockerfile
    ├── app/                 ← models, views, signals, admin
    └── config/              ← settings, urls, asgi, celery
```

---

## Local URLs

| What                | URL                                |
|---------------------|------------------------------------|
| Django landing      | http://127.0.0.1:9091/             |
| User dashboard      | http://127.0.0.1:9091/dashboard/   |
| User login          | http://127.0.0.1:9091/accounts/login/ |
| Admin panel         | http://127.0.0.1:9091/admin/       |
| Gateway HTTP        | http://127.0.0.1:8080/             |
| Gateway QUIC tunnel | `127.0.0.1:4443/udp`               |
| Metrics             | http://127.0.0.1:9090/metrics      |

For local subdomain testing we use **`*.localtest.me`**, which is a public
DNS zone that always resolves to `127.0.0.1`. So `acme.localtest.me` and
`hello.localtest.me` work out of the box.

---

## Testing as the admin

Default credentials from the quickstart: **`admin / pass`**

1. **Login**: http://127.0.0.1:9091/admin/
2. **Add a user** — *Authentication and authorization → Users → Add user*.
   Create `john / johnpass123`, leave `is_staff` off.
3. **Issue a token for that user** — *App → Auth tokens → select user(s) →
   action menu "Issue new token for selected user(s)" → Go*. The plaintext
   token appears in the warning banner (shown **once**, copy now).
4. **Claim a subdomain on their behalf** — *App → Reserved subdomains →
   Add*. Pick the user and a name (lowercase, hyphens OK).
5. **Browse the rest**:
   - *Code bases* — install snippets that appear on the landing page.
   - *Feed backs* — anonymous notes posted from the landing form.
   - *Download apps* — IP/geo records of who downloaded `install.sh`.

---

## Testing as a regular user

In any browser:

1. http://127.0.0.1:9091/accounts/login/ — sign in with `john / johnpass123`
2. You land on **/dashboard/**. Empty at first.
3. **Issue a token**: type an optional label, click *Issue new token*. The
   token appears in the green box — **copy it now**.
4. **Claim a subdomain**: type something like `john`, click *Claim*.
5. **Revoke / Release** at any time from the same tables.

### Subdomain validation rules

Try these to see them rejected (RFC 1035 + uniqueness):

| Input            | Why it fails                  |
|------------------|-------------------------------|
| `John`           | uppercase letters             |
| `-hello`         | leading hyphen                |
| `hello-`         | trailing hyphen               |
| `hello.world`    | dots not allowed              |
| `hello`          | already taken by another user |

---

## Testing the tunnel (CLI)

```bash
# 1) Save the token you got from the dashboard
./portex_rust/target/release/portex auth <TOKEN>

# 2) Run any local server in another terminal
python3 -m http.server 4000

# 3) Open the tunnel
./portex_rust/target/release/portex http \
  -s john -p 4000 \
  --server 127.0.0.1:4443 --insecure
```

Expected output:

```
✓ tunneling https://john.portex.live → http://127.0.0.1:4000
```

In a third terminal:

```bash
curl -H "Host: john.localtest.me" http://127.0.0.1:8080/
```

You should see the Python directory listing.

### Error cases (try these)

| Command                                        | Expected error                                          |
|------------------------------------------------|---------------------------------------------------------|
| `portex http -s john -p 4000` with garbage token | `Unauthorized` — invalid auth token                     |
| `portex http -s unreserved -p 4000`             | `SubdomainNotReserved` — nobody owns that subdomain     |
| `portex http -s hello -p 4000` (admin owns it)  | `SubdomainTaken` — owned by a different user            |
| Two CLIs claiming the same subdomain at once    | `SubdomainTaken` — already connected                    |

---

## Metrics

```bash
curl http://127.0.0.1:9090/metrics
```

```
# HELP portex_active_tunnels Currently connected tunnels
# TYPE portex_active_tunnels gauge
portex_active_tunnels 1
portex_tunnel_connects_total 2
portex_tunnel_disconnects_total 1
portex_requests_total 2
portex_request_errors_total 2
portex_bytes_upstream_total 160
portex_bytes_downstream_total 1324
```

Scrape with Prometheus or watch live:

```bash
watch -n 1 'curl -s http://127.0.0.1:9090/metrics | grep ^portex_'
```

---

## Wire protocol

A single QUIC connection per tunnel client. The **first** bidirectional
stream is the control channel; it carries length-prefixed binary frames:

```
[u8 type][u32 length BE][payload]

types:
  0x01  HELLO   client → server { version u16, subdomain str, token bytes }
  0x02  ACCEPT  server → client { server_version u16, assigned_subdomain str }
  0x03  REJECT  server → client { reason u8, message str }
  0x04  PING
  0x05  PONG
```

For each public request, the gateway opens a **new** bidirectional stream to
the client and writes the verbatim HTTP/1.1 wire bytes; the CLI splices
that stream to `127.0.0.1:PORT` and copies bytes both ways with
`tokio::io::copy`. There is no JSON, no base64, no buffering on the data
path.

REJECT reasons: `Unauthorized`, `SubdomainTaken`, `SubdomainNotReserved`,
`VersionIncompatible`, `ServerFull`, `Malformed`.

---

## Auth flow

1. User signs up (admin creates the account today; self-signup is a
   roadmap item).
2. Dashboard → *Issue token*. Django generates `token_urlsafe(32)`,
   stores **only the SHA-256 hash** in `AuthToken.token_hash`, and emits
   a `post_save` signal.
3. The signal writes `token:{hash} → user_id` into Redis.
4. Dashboard → *Claim subdomain*. Similar flow writes `sub:{name} → user_id`.
5. CLI sends `HELLO { subdomain, token }` over QUIC.
6. Gateway hashes the token (SHA-256), reads `token:{hash}` from Redis,
   reads `sub:{subdomain}`, checks the two user IDs match — all without
   touching Postgres on the hot path.

Revoking a token deletes it from the DB and clears the Redis key
atomically through the same signal.

---

## Production deployment

See [**DEPLOY.md**](./DEPLOY.md) for a step-by-step runbook:

- Server provisioning + ports (`80/tcp`, `443/tcp`, `4443/udp`)
- DNS setup (apex + wildcard A records)
- Cloudflare API token for ACME DNS-01
- `.env` + `.env.gateway` templates
- Staging → production cert promotion
- Routine ops (cert renewal, backups, updates)

---

## Tech stack

| Layer                  | Stack                                                     |
|------------------------|-----------------------------------------------------------|
| Hot-path proxy + tunnel | Rust 1.95, `tokio`, `quinn` (QUIC), `hyper`, `rustls`     |
| ACME                   | `instant-acme` + Cloudflare API client (`reqwest`)        |
| Control plane          | Django 5.1 + DRF + Celery + gevent                        |
| Database               | Postgres 16                                               |
| Cache + auth lookup    | Redis 7                                                   |
| Reverse-proxy of hot path | `tokio::io::copy` — zero-copy bytes both ways         |
| Build / package        | Cargo workspace + multi-stage Dockerfile                  |

---

## Repository layout (one more time, with weights)

| Path                                  | Size    | Purpose                            |
|---------------------------------------|---------|------------------------------------|
| `portex_rust/crates/common/`          | ~5 KB   | Wire protocol + codec              |
| `portex_rust/crates/gateway/`         | ~25 KB  | Server binary                      |
| `portex_rust/crates/cli/`             | ~10 KB  | Client binary                      |
| `portex_server/portex/app/`           | ~15 KB  | Django models, views, signals      |
| `portex_server/portex/templates/`     | ~10 KB  | Landing, dashboard, login          |
| `compose.yml`                         | ~2 KB   | Local + prod stack                 |
| `DEPLOY.md`                           | ~7 KB   | Production runbook                 |
| `test_e2e.sh`                         | ~4 KB   | Automated smoke test               |

Total source: ~80 KB. Built binaries: ~7 MB combined.

---

## Roadmap

- [x] Workspace scaffold + wire protocol + round-trip tests
- [x] QUIC tunnel listener + handshake + in-memory registry
- [x] HTTP + HTTPS ingress with byte splice
- [x] CLI: connect, handshake, per-stream forwarding, QUIC keepalive
- [x] Django auth (`AuthToken`, `ReservedSubdomain`) + Redis sync
- [x] Self-service dashboard (issue/revoke/claim/release)
- [x] ACME wildcard TLS (Cloudflare DNS-01)
- [x] Prometheus metrics endpoint
- [x] Hot cert reload (ArcSwap + SIGHUP + ACME renewal)
- [x] Docker compose production stack
- [ ] Additional DNS providers (Route53, DigitalOcean)
- [ ] Graceful shutdown with in-flight request draining
- [ ] Multi-instance routing (Redis pub/sub for connection placement)
- [ ] Self-signup flow + email verification
- [ ] Free vs paid tiers

---

## License

MIT. See `portex_rust/Cargo.toml` for the package metadata.
