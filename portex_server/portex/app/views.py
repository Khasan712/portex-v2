import logging
import os

from django.conf import settings
from django.contrib import messages
from django.contrib.auth import views as auth_views
from django.contrib.auth.mixins import LoginRequiredMixin
from django.core.cache import cache
from django.core.exceptions import ValidationError
from django.db import IntegrityError
from django.http import FileResponse
from django.shortcuts import get_object_or_404, redirect, render, reverse
from django.views import View

from .models import AuthToken, CodeBase, FeedBack, ReservedSubdomain
from .tasks import save_downloader

logger = logging.getLogger('Common')


def _client_ip(request) -> str:
    """Best-effort client IP. X-Forwarded-For is client-controlled — the task
    that consumes this validates it before use."""
    forwarded = request.META.get('HTTP_X_FORWARDED_FOR')
    if forwarded:
        return forwarded.split(',')[0].strip()
    return request.META.get('REMOTE_ADDR', '')


def install_sh(request):
    """Serve the CLI installer script.

    The download record is a side-channel: if the broker is down, the user
    still gets their installer.
    """
    file_path = os.path.join(settings.MEDIA_ROOT, 'install.sh')
    try:
        save_downloader.delay(
            _client_ip(request),
            request.META.get('HTTP_USER_AGENT', '')[:200],
        )
    except Exception:
        logger.warning('install.sh: could not queue download record', exc_info=True)
    # FileResponse needs a file object — handing it a str skips set_headers(),
    # which drops Content-Disposition and streams one byte per chunk.
    return FileResponse(
        open(file_path, 'rb'),
        as_attachment=True,
        filename='install.sh',
        content_type='text/x-shellscript',
    )


class HomeView(View):
    """Apex landing page + feedback form.

    All `*.portex.live` traffic now terminates on the Rust gateway; Django
    only serves `portex.live` itself.
    """

    @staticmethod
    def _codes():
        return CodeBase.objects.order_by('rank')

    def get(self, request):
        return render(request, 'index.html', {'codes': self._codes()})

    def post(self, request):
        text = request.POST.get('text')
        if text:
            FeedBack.objects.create(text=text)
            messages.success(request, 'Thanks for feedback!')
        return redirect(reverse('home'))


def _throttle_get(key, default=0):
    """Cache reads that fail open — a Redis outage must not lock users out."""
    try:
        return cache.get(key, default)
    except Exception:
        logger.warning('login throttle: cache unavailable', exc_info=True)
        return default


def _throttle_write(op, *args):
    try:
        op(*args)
    except Exception:
        logger.warning('login throttle: cache write failed', exc_info=True)


class ThrottledLoginView(auth_views.LoginView):
    """Sign-in with a per-IP attempt cap.

    This is the only password endpoint in the project and it had no brute
    force protection at all.
    """

    template_name = 'login.html'

    def post(self, request, *args, **kwargs):
        limit = settings.PORTEX_LOGIN_MAX_ATTEMPTS
        key = f'login-throttle:{_client_ip(request)}'
        attempts = _throttle_get(key) if limit else 0

        if limit and attempts >= limit:
            messages.error(
                request,
                'Too many sign-in attempts. Wait a few minutes and try again.',
            )
            return self.render_to_response(
                self.get_context_data(form=self.get_form()), status=429
            )

        response = super().post(request, *args, **kwargs)

        if request.user.is_authenticated:
            _throttle_write(cache.delete, key)
        elif limit:
            _throttle_write(cache.set, key, attempts + 1, settings.PORTEX_LOGIN_THROTTLE_SECONDS)
        return response


class DashboardView(LoginRequiredMixin, View):
    """Single page where a logged-in user manages tokens + subdomains."""

    @staticmethod
    def context(user, fresh_token=None):
        return {
            'tokens': AuthToken.refresh_last_used(user),
            'subdomains': user.subdomains.all(),
            'fresh_token': fresh_token,
        }

    def get(self, request):
        return render(request, 'dashboard.html', self.context(request.user))


class TokenIssueView(LoginRequiredMixin, View):
    """POST /tokens/ — issue a new token for the current user."""

    def post(self, request):
        name = (request.POST.get('name') or '').strip()[:64]
        try:
            _, plaintext = AuthToken.issue(request.user, name=name)
        except ValidationError as exc:
            messages.error(request, '; '.join(exc.messages))
            return redirect(reverse('dashboard'))
        messages.success(request, 'Token created. Copy it now — it will not be shown again.')
        # Rendered straight into this one response rather than stashed in the
        # session and picked up after a redirect: the session store is the
        # database, unencrypted, so a round-trip through it would persist the
        # plaintext the model deliberately never saves.
        return render(
            request,
            'dashboard.html',
            DashboardView.context(request.user, fresh_token=plaintext),
        )


class TokenRevokeView(LoginRequiredMixin, View):
    """POST /tokens/<id>/revoke/ — delete a token."""

    def post(self, request, pk):
        token = get_object_or_404(AuthToken, pk=pk, user=request.user)
        token.delete()
        messages.success(request, 'Token revoked.')
        return redirect(reverse('dashboard'))


class SubdomainClaimView(LoginRequiredMixin, View):
    """POST /subdomains/ — claim a subdomain."""

    def post(self, request):
        name = (request.POST.get('name') or '').strip().lower()
        if not name:
            messages.error(request, 'Subdomain name is required.')
            return redirect(reverse('dashboard'))
        try:
            ReservedSubdomain(user=request.user, name=name).save()
            messages.success(request, f'Subdomain `{name}` is yours.')
        except ValidationError as exc:
            # Covers both the RFC 1035 shape check and the per-user quota, so
            # report the underlying message rather than guessing at the cause.
            messages.error(request, '; '.join(exc.messages))
        except IntegrityError:
            messages.error(request, f'Subdomain `{name}` is already taken.')
        return redirect(reverse('dashboard'))


class SubdomainReleaseView(LoginRequiredMixin, View):
    """POST /subdomains/<id>/release/ — release a previously claimed subdomain."""

    def post(self, request, pk):
        sub = get_object_or_404(ReservedSubdomain, pk=pk, user=request.user)
        sub.delete()
        messages.success(request, 'Subdomain released.')
        return redirect(reverse('dashboard'))
