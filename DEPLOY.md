# Portex production deployment

This runbook walks through bringing a single-server portex deployment online
from a clean Ubuntu/Debian box.

## 1. Provision the server

Pick any VPS with at least:

- 2 vCPU, 2 GB RAM, 20 GB SSD (works for ~10k concurrent tunnels)
- Open ports: **80/tcp** (HTTP-to-HTTPS redirect, ACME), **443/tcp** (public
  HTTPS), **4443/udp** (QUIC tunnel)

On the box, install Docker:

```bash
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker $USER && newgrp docker
```

## 2. DNS setup

Point an apex domain and wildcard at the server:

```
A     portex.live          → <SERVER_IP>
A     *.portex.live        → <SERVER_IP>
AAAA  portex.live          → <SERVER_IPv6>   (optional)
AAAA  *.portex.live        → <SERVER_IPv6>   (optional)
```

The wildcard is what makes `acme.portex.live`, `myapp.portex.live`, etc.
resolve to the same server. The Rust gateway then routes by Host header /
SNI.

## 3. Cloudflare API token (for ACME DNS-01)

Wildcard certs require DNS-01 challenges — the gateway adds TXT records to
prove domain ownership.

1. https://dash.cloudflare.com/profile/api-tokens → Create Token
2. Custom token with permissions:
   - `Zone | DNS | Edit`
   - `Zone | Zone | Read`
3. Zone resources: `Include | Specific zone | portex.live`
4. Copy the token (shown once).
5. From the zone dashboard, copy the **Zone ID** (32-char hex).

## 4. Clone the repo

```bash
ssh deploy@<SERVER_IP>
git clone https://github.com/<you>/portex.git
cd portex
```

## 5. Configure environment

Copy the env template and fill it in:

```bash
cp portex_server/portex/.env.example portex_server/portex/.env
nano portex_server/portex/.env
```

Required values:

```
SECRET_KEY=<run: python -c "from secrets import token_urlsafe; print(token_urlsafe(50))">
DEBUG=False
POSTGRES_DB=portex
POSTGRES_USER=portex_user
POSTGRES_PASSWORD=<strong random>
POSTGRES_HOST=db
POSTGRES_PORT=5432
REDIS_HOST=redis
REDIS_PORT=6379
CORS_ALLOWED_ORIGINS=https://portex.live,https://www.portex.live
MAIN_HOST=portex.live
```

Then add an extra file at the repo root with the gateway-specific vars (do
NOT put these into the Django .env file — keeps blast radius tight):

```bash
cat > .env.gateway <<EOF
PORTEX_BASE_DOMAIN=portex.live
PORTEX_HTTP_ADDR=0.0.0.0:80
PORTEX_HTTPS_ADDR=0.0.0.0:443
PORTEX_TUNNEL_ADDR=0.0.0.0:4443
PORTEX_REDIS_URL=redis://redis:6379/0
PORTEX_ACME_DOMAIN=portex.live
PORTEX_ACME_EMAIL=ops@portex.live
PORTEX_ACME_STAGING=true
CLOUDFLARE_API_TOKEN=<paste here>
CLOUDFLARE_ZONE_ID=<paste here>
PORTEX_STATE_DIR=/var/lib/portex
PORTEX_METRICS_ADDR=127.0.0.1:9090
EOF
chmod 600 .env.gateway
```

Wire it into compose by adding `env_file: .env.gateway` and the port
mappings to the `gateway` service in `compose.yml`:

```yaml
gateway:
  build: ./portex_rust
  container_name: portex_gateway
  restart: unless-stopped
  env_file: .env.gateway
  volumes:
    - portex_state:/var/lib/portex
  depends_on:
    - redis
  ports:
    - "80:80/tcp"
    - "443:443/tcp"
    - "4443:4443/udp"
    - "127.0.0.1:9090:9090/tcp"   # metrics — bind to loopback only
```

And add the volume:

```yaml
volumes:
  postgres_data_portex:
  portex_redis_data:
  portex_state:
```

## 6. First start (staging cert)

```bash
docker compose up -d --build
docker compose logs -f gateway | grep -E "acme:|error"
```

You should see:

```
INFO acme: obtaining new wildcard cert domain=portex.live
INFO cloudflare: TXT created
INFO acme: publishing DNS-01 challenge
INFO cloudflare: TXT deleted
INFO ingress: HTTPS listening
INFO tunnel: QUIC endpoint listening
```

Issue from staging is not browser-trusted but proves the flow works. Verify
with curl:

```bash
curl -k --resolve portex.live:443:127.0.0.1 https://portex.live/
```

## 7. Promote to production cert

Flip the staging flag and recreate just the gateway:

```bash
sed -i 's/PORTEX_ACME_STAGING=true/PORTEX_ACME_STAGING=false/' .env.gateway
docker compose up -d --force-recreate gateway
docker compose logs -f gateway | grep -E "acme:|cert"
```

The gateway will obtain a real Let's Encrypt cert. After ~30 s:

```bash
curl https://portex.live/
```

should return the Django landing page over a trusted TLS cert.

## 8. Initial admin user

```bash
docker compose exec django python manage.py createsuperuser
```

Then visit `https://portex.live/admin/` and confirm everything looks right.

## 9. End-to-end smoke

On a client machine:

```bash
# Install the CLI (assuming binaries published to releases).
curl -fsSL https://portex.live/install/ | sudo bash
```

In a browser:
1. https://portex.live/accounts/login/ — sign in
2. /dashboard/ — issue a token, claim a subdomain (e.g. `acme`)

Back in the terminal:

```bash
portex auth <token>
python3 -m http.server 8000 &
portex http -s acme -p 8000
# In another shell, on any machine:
curl https://acme.portex.live/
```

You should see the directory listing served by Python.

## 10. Monitoring

Metrics are exposed on `127.0.0.1:9090/metrics` (Prometheus format, no
auth). Wire your favourite scraper to it. Key signals to alert on:

- `portex_active_tunnels` stays at 0 for > 5 min during business hours
- `portex_request_errors_total` rises faster than `portex_requests_total`
- Container restarts > 1 in an hour

Container logs are JSON-structured (via `tracing`):

```bash
docker compose logs -f --tail=100 gateway
```

## 11. Routine ops

- **Cert renewal** is automatic. The gateway renews when < 30 days remain
  and hot-swaps the new cert. No restart required. Watch
  `acme: cert renewed and reloaded` in logs every ~60 days.
- **DB backups**: nightly `pg_dump` to S3 or equivalent.
  ```bash
  docker compose exec -T db pg_dump -U portex_user portex | gzip > portex-$(date +%F).sql.gz
  ```
- **Updating**:
  ```bash
  git pull && docker compose up -d --build
  ```
  No data loss; volumes are preserved.

## 12. Known limits

- Single-instance only right now. For horizontal scale, the gateway needs a
  Redis-pub/sub-based subdomain → instance routing layer (not yet built).
- No graceful shutdown — `docker compose down` drops active tunnels. Trade
  off for simplicity; revisit when traffic grows.
- ACME provider lock-in: Cloudflare DNS only. Adding Route53/DigitalOcean
  is a self-contained module under `portex_rust/crates/gateway/src/acme/`.
