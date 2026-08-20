"""Sync auth state to Redis so the Rust gateway can validate on the hot path.

Keys written:
    token:{sha256_hex}  -> user_id (string)
    sub:{name}          -> user_id (string)

The gateway only reads; Django is the source of truth. Two rules make that
relationship survive real-world failures:

* Writes fire on `transaction.on_commit`, so a rolled-back request never
  leaves a live credential behind in Redis.
* A Redis outage is logged, not raised. Losing the index degrades new tunnel
  handshakes; it must not take the dashboard down with it. Rebuild the index
  afterwards with `manage.py sync_redis`.
"""

import logging

import redis as _redis_lib
from django.conf import settings
from django.db import transaction
from django.db.models.signals import post_delete, post_save
from django.dispatch import receiver

from .models import AuthToken, ReservedSubdomain

logger = logging.getLogger('Common')

_client = None


def redis_client():
    global _client
    if _client is None:
        try:
            _client = _redis_lib.Redis.from_url(
                settings.REDIS_AUTH_INDEX_URL,
                decode_responses=True,
            )
        except Exception:
            logger.exception('redis: could not build client')
            return None
    return _client


def token_key(token_hash: str) -> str:
    return f'token:{token_hash}'


def subdomain_key(name: str) -> str:
    return f'sub:{name}'


def token_used_key(token_hash: str) -> str:
    """Where the gateway records the last time a token opened a tunnel."""
    return f'token_used:{token_hash}'


def write_key(key: str, value: str):
    r = redis_client()
    if r is None:
        return
    try:
        r.set(key, value)
    except Exception:
        logger.exception('redis: failed to write %s — run `manage.py sync_redis`', key)


def delete_key(key: str):
    r = redis_client()
    if r is None:
        return
    try:
        r.delete(key)
    except Exception:
        logger.exception('redis: failed to delete %s — run `manage.py sync_redis`', key)


@receiver(post_save, sender=AuthToken)
def push_token(sender, instance: AuthToken, **kwargs):
    key, value = token_key(instance.token_hash), str(instance.user_id)
    transaction.on_commit(lambda: write_key(key, value))


@receiver(post_delete, sender=AuthToken)
def drop_token(sender, instance: AuthToken, **kwargs):
    keys = (token_key(instance.token_hash), token_used_key(instance.token_hash))
    transaction.on_commit(lambda: [delete_key(k) for k in keys])


@receiver(post_save, sender=ReservedSubdomain)
def push_subdomain(sender, instance: ReservedSubdomain, **kwargs):
    key, value = subdomain_key(instance.name), str(instance.user_id)
    transaction.on_commit(lambda: write_key(key, value))


@receiver(post_delete, sender=ReservedSubdomain)
def drop_subdomain(sender, instance: ReservedSubdomain, **kwargs):
    key = subdomain_key(instance.name)
    transaction.on_commit(lambda: delete_key(key))
