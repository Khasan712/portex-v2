import ipaddress
import json
import logging

import requests

from app.models import DownloadApp
from celery import shared_task

logger = logging.getLogger('Common')

# (connect, read) — an unreachable geo API must not pin a worker forever.
IPINFO_TIMEOUT = (3.05, 5)


@shared_task()
def save_downloader(ip: str, user_agent: str = ''):
    """Record who downloaded install.sh.

    `ip` originates from a client-controlled X-Forwarded-For header, so it is
    validated before being interpolated into the lookup URL.
    """
    try:
        ipaddress.ip_address(ip)
    except ValueError:
        logger.info('save_downloader: ignoring malformed ip %r', ip[:60])
        return

    try:
        response = requests.get(f'https://ipinfo.io/{ip}/json', timeout=IPINFO_TIMEOUT)
        data = response.json()
    except Exception as exc:
        logger.warning('save_downloader: geo lookup failed: %s', exc)
        data = {}

    # DownloadApp.info is a CharField(300); keep a bounded, curated subset
    # rather than dumping the whole upstream payload into it.
    info = {
        key: value
        for key, value in (
            ('ip', ip),
            ('city', data.get('city')),
            ('country', data.get('country')),
            ('org', (data.get('org') or '')[:60]),
            ('ua', user_agent[:80]),
        )
        if value
    }
    DownloadApp.objects.create(info=json.dumps(info)[:300])
