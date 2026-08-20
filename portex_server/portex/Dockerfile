FROM python:3.11-slim

# Keep .pyc files out of the image and logs unbuffered so `docker logs` is live.
ENV PYTHONDONTWRITEBYTECODE=1
ENV PYTHONUNBUFFERED=1

WORKDIR /portex

COPY requirements.txt /portex/
RUN pip install --no-cache-dir --upgrade pip \
    && pip install --no-cache-dir -r requirements.txt

# This used to copy into /app while WORKDIR was /portex, so the image only
# worked when compose bind-mounted the source over it.
COPY . /portex/

# Writable spots for a non-root run. Logs live outside /portex on purpose:
# compose bind-mounts the source tree over it during development, and a
# bind mount carries the host's ownership, so anything the container needs
# to write inside it would be denied.
RUN useradd --create-home --uid 10001 portex \
    && mkdir -p /portex/staticfiles /var/log/portex \
    && chown -R portex:portex /portex /var/log/portex
ENV LOG_DIR=/var/log/portex
USER portex

EXPOSE 9091

HEALTHCHECK --interval=30s --timeout=5s --start-period=40s --retries=3 \
    CMD python -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:9091/', timeout=4).status == 200 else 1)"

CMD ["bash", "entrypoint.sh"]
