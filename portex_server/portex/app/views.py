import os

from django.conf import settings
from django.contrib import messages
from django.contrib.auth.mixins import LoginRequiredMixin
from django.core.exceptions import ValidationError
from django.http import FileResponse
from django.shortcuts import get_object_or_404, redirect, render, reverse
from django.views import View

from .models import AuthToken, CodeBase, FeedBack, ReservedSubdomain
from .tasks import save_downloader


def install_sh(request):
    file_path = os.path.join(settings.MEDIA_ROOT, 'install.sh')
    with open(file_path, 'r') as file:
        content = file.read()
    save_downloader.delay(request.META)
    return FileResponse(content, as_attachment=True, filename='install.sh', content_type='text/x-shellscript')


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


class DashboardView(LoginRequiredMixin, View):
    """Single page where a logged-in user manages tokens + subdomains."""

    def get(self, request):
        return render(request, 'dashboard.html', {
            'tokens': request.user.tokens.all(),
            'subdomains': request.user.subdomains.all(),
            'fresh_token': request.session.pop('fresh_token', None),
        })


class TokenIssueView(LoginRequiredMixin, View):
    """POST /tokens/ — issue a new token for the current user."""

    def post(self, request):
        name = (request.POST.get('name') or '').strip()[:64]
        _, plaintext = AuthToken.issue(request.user, name=name)
        request.session['fresh_token'] = plaintext
        messages.success(request, 'Token created. Copy it now — it will not be shown again.')
        return redirect(reverse('dashboard'))


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
            messages.error(request, f'Invalid subdomain: {"; ".join(exc.messages)}')
        except Exception:
            messages.error(request, f'Subdomain `{name}` is already taken.')
        return redirect(reverse('dashboard'))


class SubdomainReleaseView(LoginRequiredMixin, View):
    """POST /subdomains/<id>/release/ — release a previously claimed subdomain."""

    def post(self, request, pk):
        sub = get_object_or_404(ReservedSubdomain, pk=pk, user=request.user)
        sub.delete()
        messages.success(request, 'Subdomain released.')
        return redirect(reverse('dashboard'))
