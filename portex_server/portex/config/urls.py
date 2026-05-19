from django.conf import settings
from django.conf.urls.static import static
from django.contrib import admin
from django.contrib.auth import views as auth_views
from django.urls import path

from app.views import (
    DashboardView,
    HomeView,
    SubdomainClaimView,
    SubdomainReleaseView,
    TokenIssueView,
    TokenRevokeView,
    install_sh,
)

urlpatterns = [
    path('admin/', admin.site.urls),
    path('install/', install_sh),

    # Self-service dashboard for tokens + subdomains.
    path('accounts/login/', auth_views.LoginView.as_view(template_name='login.html'), name='login'),
    path('accounts/logout/', auth_views.LogoutView.as_view(next_page='home'), name='logout'),
    path('dashboard/', DashboardView.as_view(), name='dashboard'),
    path('tokens/', TokenIssueView.as_view(), name='token-issue'),
    path('tokens/<int:pk>/revoke/', TokenRevokeView.as_view(), name='token-revoke'),
    path('subdomains/', SubdomainClaimView.as_view(), name='subdomain-claim'),
    path('subdomains/<int:pk>/release/', SubdomainReleaseView.as_view(), name='subdomain-release'),

    path('', HomeView.as_view(), name='home'),
] + static(
    settings.MEDIA_URL,
    document_root=settings.MEDIA_ROOT,
) + static(
    settings.STATIC_URL,
    document_root=settings.STATIC_ROOT,
)
