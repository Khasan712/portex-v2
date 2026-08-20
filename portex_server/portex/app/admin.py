import ast
import json

from django.contrib import admin
from django.core.exceptions import ValidationError
from django.template.response import TemplateResponse

from .models import AuthToken, CodeBase, DownloadApp, FeedBack, ReservedSubdomain


@admin.register(AuthToken)
class AuthTokenAdmin(admin.ModelAdmin):
    list_display = ('id', 'user', 'name', 'token_hash', 'created_at', 'last_used_at')
    readonly_fields = ('token_hash', 'created_at', 'last_used_at')
    list_filter = ('created_at',)
    search_fields = ('user__username', 'name')
    actions = ('issue_new_token',)

    @admin.action(description="Issue new token for selected user(s)")
    def issue_new_token(self, request, queryset):
        # Reuse selected rows to identify users; issue fresh tokens for each.
        issued, refused = [], []
        for user in {t.user for t in queryset}:
            try:
                _, plaintext = AuthToken.issue(user, name='admin-issued')
            except ValidationError as exc:
                # Quotas apply to admin-issued tokens too — say so rather than
                # failing the whole action with a 500.
                refused.append((user, '; '.join(exc.messages)))
                continue
            issued.append((user, plaintext))
        # Rendered on its own page rather than pushed through `messages`,
        # which is session-backed and would write the plaintext to the
        # database the model takes care never to store.
        return TemplateResponse(
            request,
            'admin/issued_tokens.html',
            {
                **self.admin_site.each_context(request),
                'title': 'New auth tokens',
                'issued': issued,
                'refused': refused,
            },
        )


@admin.register(ReservedSubdomain)
class ReservedSubdomainAdmin(admin.ModelAdmin):
    list_display = ('id', 'name', 'user', 'created_at')
    search_fields = ('name', 'user__username')


@admin.register(CodeBase)
class CodeBaseAdmin(admin.ModelAdmin):
    list_display = ('id', 'header', 'rank', 'created_at', 'updated_at')


@admin.register(FeedBack)
class FeedBackAdmin(admin.ModelAdmin):
    list_display = ('id', 'created_at', 'updated_at', 'text')


@admin.register(DownloadApp)
class DownloadAppAdmin(admin.ModelAdmin):
    list_display = ('id', 'created_at', 'updated_at', 'country', 'info')

    def country(self, obj):
        # Newer rows are JSON; older ones are a repr() of a dict.
        for parse in (json.loads, ast.literal_eval):
            try:
                return parse(obj.info).get('country') or '-'
            except Exception:
                continue
        return '-'
    country.short_description = 'Country'
