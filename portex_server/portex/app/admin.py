import ast
import json
from django.contrib import admin, messages
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
        users = {t.user for t in queryset}
        for user in users:
            _, plaintext = AuthToken.issue(user, name='admin-issued')
            messages.warning(
                request,
                f"Token for {user}: {plaintext}  (shown once — copy now)",
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
        try:
            fixed_info = json.dumps(ast.literal_eval(obj.info))
            info = json.loads(fixed_info)
            return info.get('country')
        except Exception as e:
            print(str(e))
            return '-'
    country.short_description = 'Country'
