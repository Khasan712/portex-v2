import hashlib
import secrets

from django.conf import settings
from django.core.validators import RegexValidator
from django.db import models

# RFC 1035 label: lowercase a-z, digits, hyphens; no leading/trailing hyphen;
# 1-63 chars. Subdomain reservations follow the same shape.
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
        """Create a new token. Returns (model, plaintext) — store the plaintext now or lose it."""
        plaintext = secrets.token_urlsafe(32)
        obj = cls.objects.create(user=user, name=name, token_hash=_hash_token(plaintext))
        return obj, plaintext


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
