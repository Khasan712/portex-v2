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

# Writable spots for a non-root run: collectstatic output and the log tree.
RUN useradd --create-home --uid 10001 portex \
    && mkdir -p /portex/staticfiles /portex/logs \
    && chown -R portex:portex /portex
USER portex

EXPOSE 9091

HEALTHCHECK --interval=30s --timeout=5s --start-period=40s --retries=3 \
    CMD python -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:9091/', timeout=4).status == 200 else 1)"

CMD ["bash", "entrypoint.sh"]
