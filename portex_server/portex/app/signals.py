"""Sync auth state to Redis so the Rust gateway can validate on the hot path.

Keys written:
    token:{sha256_hex}  -> user_id (string)
    sub:{name}          -> user_id (string)

The gateway only reads; Django is the source of truth.
"""

import redis as _redis_lib
from django.conf import settings
from django.db.models.signals import post_delete, post_save
from django.dispatch import receiver

from .models import AuthToken, ReservedSubdomain

_client = None


def _redis():
    global _client
    if _client is None:
        try:
            _client = _redis_lib.Redis(
                host=settings.REDIS_HOST,
                port=int(settings.REDIS_PORT),
                decode_responses=True,
            )
        except Exception:
            return None
    return _client


@receiver(post_save, sender=AuthToken)
def push_token(sender, instance: AuthToken, **kwargs):
    r = _redis()
    if r is None:
        return
    r.set(f"token:{instance.token_hash}", str(instance.user_id))


@receiver(post_delete, sender=AuthToken)
def drop_token(sender, instance: AuthToken, **kwargs):
    r = _redis()
    if r is None:
        return
    r.delete(f"token:{instance.token_hash}")


@receiver(post_save, sender=ReservedSubdomain)
def push_subdomain(sender, instance: ReservedSubdomain, **kwargs):
    r = _redis()
    if r is None:
        return
    r.set(f"sub:{instance.name}", str(instance.user_id))


@receiver(post_delete, sender=ReservedSubdomain)
def drop_subdomain(sender, instance: ReservedSubdomain, **kwargs):
    r = _redis()
    if r is None:
        return
    r.delete(f"sub:{instance.name}")
