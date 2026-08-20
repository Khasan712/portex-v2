import hashlib
import logging
import secrets
from datetime import datetime, timezone as _tz

# django.utils.timezone.utc was removed in Django 5.0; use the stdlib one.
UTC = _tz.utc

from django.conf import settings
from django.core.exceptions import ValidationError
from django.core.validators import RegexValidator
from django.db import models

# RFC 1035 label: lowercase a-z, digits, hyphens; no leading/trailing hyphen;
# 1-63 chars. Subdomain reservations follow the same shape.
logger = logging.getLogger('Common')

SUBDOMAIN_VALIDATOR = RegexValidator(
    regex=r'^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$',
    message='Subdomain must be 1-63 chars, a-z/0-9/hyphen, no leading/trailing hyphen.',
)


def _hash_token(plaintext: str) -> str:
    return hashlib.sha256(plaintext.encode()).hexdigest()


class AuthToken(models.Model):
    """Personal access token issued to a user via the dashboard.

    Only the SHA-256 hash is stored. The plaintext is shown once at creation
    time and never persisted.
    """
    user = models.ForeignKey(settings.AUTH_USER_MODEL, on_delete=models.CASCADE, related_name='tokens')
    name = models.CharField(max_length=64, blank=True, default='')
    token_hash = models.CharField(max_length=64, unique=True, db_index=True)
    created_at = models.DateTimeField(auto_now_add=True)
    last_used_at = models.DateTimeField(null=True, blank=True)

    class Meta:
        ordering = ('-created_at',)

    def __str__(self):
        return f'{self.user_id}: {self.name or self.token_hash[:8]}'

    @classmethod
    def issue(cls, user, name: str = '') -> tuple['AuthToken', str]:
        """Create a new token. Returns (model, plaintext) — store the plaintext now or lose it.

        Raises ValidationError when the user is at their token quota. Enforced
        here rather than in the view so the admin action is covered too.
        """
        limit = settings.PORTEX_MAX_TOKENS_PER_USER
        if limit and cls.objects.filter(user=user).count() >= limit:
            raise ValidationError(
                f'Token limit reached ({limit}). Revoke an existing token first.'
            )
        plaintext = secrets.token_urlsafe(32)
        obj = cls.objects.create(user=user, name=name, token_hash=_hash_token(plaintext))
        return obj, plaintext


    @classmethod
    def refresh_last_used(cls, user):
        """Pull the gateway's usage timestamps into the database.

        The gateway authorizes against Redis and never touches Postgres, so
        `last_used_at` would otherwise stay null forever and the dashboard
        would always read "never". Called when the dashboard renders; the
        per-user token quota keeps this to a handful of keys.
        """
        from .signals import redis_client, token_used_key

        tokens = list(cls.objects.filter(user=user))
        if not tokens:
            return tokens

        client = redis_client()
        if client is None:
            return tokens

        try:
            marks = client.mget([token_used_key(t.token_hash) for t in tokens])
        except Exception:
            logger.warning('could not read token usage from redis', exc_info=True)
            return tokens

        stale = []
        for token, mark in zip(tokens, marks):
            if not mark:
                continue
            seen = datetime.fromtimestamp(int(mark), tz=UTC)
            if token.last_used_at is None or seen > token.last_used_at:
                token.last_used_at = seen
                stale.append(token)
        if stale:
            cls.objects.bulk_update(stale, ['last_used_at'])
        return tokens


class ReservedSubdomain(models.Model):
    """A subdomain a user has claimed."""
    user = models.ForeignKey(settings.AUTH_USER_MODEL, on_delete=models.CASCADE, related_name='subdomains')
    name = models.CharField(max_length=63, unique=True, db_index=True, validators=[SUBDOMAIN_VALIDATOR])
    created_at = models.DateTimeField(auto_now_add=True)

    class Meta:
        ordering = ('name',)

    def __str__(self):
        return self.name

    def save(self, *args, **kwargs):
        self.name = self.name.lower()
        limit = settings.PORTEX_MAX_SUBDOMAINS_PER_USER
        if self._state.adding and limit:
            if ReservedSubdomain.objects.filter(user=self.user).count() >= limit:
                raise ValidationError(
                    f'Subdomain limit reached ({limit}). Release one first.'
                )
        self.full_clean()
        super().save(*args, **kwargs)


class CodeBase(models.Model):
    header = models.CharField(max_length=255, blank=True, null=True)
    code = models.CharField(max_length=300, blank=True, null=True)
    extra_info = models.CharField(max_length=300, blank=True, null=True)
    rank = models.IntegerField(default=0)
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

    def __str__(self):
        return f'{self.id} - {self.header}'


class FeedBack(models.Model):
    text = models.CharField(max_length=500)
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

    def __str__(self):
        return f'{self.id}'


class DownloadApp(models.Model):
    info = models.CharField(max_length=300)
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

    def __str__(self):
        return f'{self.id}'
