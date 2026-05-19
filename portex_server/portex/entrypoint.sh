#!/usr/bin/env bash
set -euo pipefail

python manage.py migrate --noinput
python manage.py collectstatic --noinput

# Django serves only the apex domain now; the Rust gateway handles
# *.portex.live tunnel traffic. WSGI is enough — no ASGI/Channels.
exec gunicorn config.wsgi:application \
    --bind 0.0.0.0:9091 \
    --workers 2 \
    --worker-class gevent \
    --timeout 60 \
    --access-logfile - \
    --error-logfile -
