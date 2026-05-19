#!/usr/bin/env bash
# Full end-to-end smoke test of the local portex stack.
# Assumes docker compose is already up and the binary is built.

set -e
CLI=./portex_rust/target/release/portex
BASE=127.0.0.1
DJANGO=$BASE:9091
GATEWAY_HTTP=$BASE:8080
GATEWAY_QUIC=$BASE:4443
METRICS=$BASE:9090

green() { printf '\033[32m✓\033[0m %s\n' "$1"; }
red()   { printf '\033[31m✗\033[0m %s\n' "$1"; }
check() { [[ "$1" == "$2" ]] && green "$3" || { red "$3 — got: $1 expected: $2"; exit 1; }; }

# 1) Stack health
[[ "$(docker ps --filter name=portex --format '{{.Names}}' | wc -l | tr -d ' ')" == "5" ]] \
  && green "5 containers running" || { red "stack down"; exit 1; }

# 2) Django landing
status=$(curl -s -o /dev/null -w "%{http_code}" http://$DJANGO/)
check "$status" "200" "Django landing"

# 3) Dashboard redirects to login when not authed
status=$(curl -s -o /dev/null -w "%{http_code}" http://$DJANGO/dashboard/)
check "$status" "302" "Dashboard requires login"

# 4) Metrics endpoint
status=$(curl -s -o /dev/null -w "%{http_code}" http://$METRICS/metrics)
check "$status" "200" "Metrics endpoint"

# 5) Gateway returns 502 for unknown subdomain
status=$(curl -s -o /dev/null -w "%{http_code}" -H "Host: nope.localtest.me" http://$GATEWAY_HTTP/)
check "$status" "502" "Gateway 502 for unknown subdomain"

# 6) Issue a fresh token for john
TOKEN=$(docker exec portex_django python manage.py shell -c "
from django.contrib.auth import get_user_model
from app.models import AuthToken, ReservedSubdomain
u, _ = get_user_model().objects.get_or_create(username='john', defaults={'email':'j@x'})
u.set_password('johnpass123'); u.save()
AuthToken.objects.filter(user=u).delete()
ReservedSubdomain.objects.get_or_create(name='john', defaults={'user': u})
_, t = AuthToken.issue(u, name='e2e')
print(t)
" 2>&1 | tail -1)
[[ "${#TOKEN}" -gt 30 ]] && green "Token issued for john (${TOKEN:0:8}...)" || { red "no token"; exit 1; }

# 7) Local app
pkill -f "http.server 4444" 2>/dev/null || true
python3 -m http.server 4444 > /tmp/portex-local.log 2>&1 &
LOCAL_PID=$!
sleep 1
status=$(curl -s -o /dev/null -w "%{http_code}" http://$BASE:4444/)
check "$status" "200" "Local app on :4444"

# 8) Save token and start CLI
$CLI auth "$TOKEN" > /dev/null
pkill -f "$CLI http" 2>/dev/null || true
sleep 1
$CLI http -s john -p 4444 --server $GATEWAY_QUIC --insecure > /tmp/portex-cli.log 2>&1 &
CLI_PID=$!
sleep 3
grep -q "tunneling" /tmp/portex-cli.log \
  && green "Tunnel established" \
  || { red "tunnel failed"; tail /tmp/portex-cli.log; exit 1; }

# 9) Tunneled request
status=$(curl -s -o /dev/null -w "%{http_code}" -H "Host: john.localtest.me" http://$GATEWAY_HTTP/)
check "$status" "200" "Tunneled request"

# 10) Wrong-token rejection
$CLI auth "wrong" > /dev/null
err=$($CLI http -s john -p 4444 --server $GATEWAY_QUIC --insecure 2>&1 | grep -c "Unauthorized")
[[ "$err" -gt 0 ]] && green "Wrong token correctly rejected" || red "wrong-token test failed"

# 11) Restore good token
$CLI auth "$TOKEN" > /dev/null

# 12) Subdomain owned by someone else
err=$($CLI http -s hello -p 4444 --server $GATEWAY_QUIC --insecure 2>&1 | grep -c "SubdomainTaken")
[[ "$err" -gt 0 ]] && green "Other user's subdomain correctly blocked" || red "subdomain ownership test failed"

# Cleanup
kill $CLI_PID $LOCAL_PID 2>/dev/null || true

# 13) Final metric snapshot
echo
echo "── /metrics snapshot ──"
curl -s http://$METRICS/metrics | grep -E "^portex_" | head -10

echo
green "ALL END-TO-END CHECKS PASSED"
