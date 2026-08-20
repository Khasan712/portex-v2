"""Tests for the auth surface: token issuance, quotas, and the views that
expose them. These cover the behaviours that are easy to regress silently —
a token leaking into the session store, a quota that stops being enforced,
a throttle that stops counting.
"""

import hashlib
from datetime import datetime, timezone
from unittest import mock

from django.contrib.auth import get_user_model
from django.core.exceptions import ValidationError
from django.db.utils import IntegrityError
from django.test import TestCase, override_settings
from django.urls import reverse

from app.models import AuthToken, DownloadApp, ReservedSubdomain
from app.tasks import save_downloader

User = get_user_model()

LOCMEM_CACHE = {'default': {'BACKEND': 'django.core.cache.backends.locmem.LocMemCache'}}


class AuthTokenModelTests(TestCase):
    def setUp(self):
        self.user = User.objects.create_user('alice', password='pw')

    def test_issue_returns_plaintext_and_stores_only_the_hash(self):
        obj, plaintext = AuthToken.issue(self.user, name='laptop')
        self.assertTrue(plaintext)
        self.assertEqual(obj.token_hash, hashlib.sha256(plaintext.encode()).hexdigest())
        # The plaintext must not be recoverable from anything we persisted.
        self.assertNotIn(plaintext, str(AuthToken.objects.values_list()))

    @override_settings(PORTEX_MAX_TOKENS_PER_USER=2)
    def test_quota_is_enforced_and_released_by_revoking(self):
        AuthToken.issue(self.user)
        AuthToken.issue(self.user)
        with self.assertRaises(ValidationError):
            AuthToken.issue(self.user)

        AuthToken.objects.filter(user=self.user).first().delete()
        AuthToken.issue(self.user)  # a slot freed up
        self.assertEqual(AuthToken.objects.filter(user=self.user).count(), 2)

    @override_settings(PORTEX_MAX_TOKENS_PER_USER=1)
    def test_quota_is_per_user(self):
        other = User.objects.create_user('bob', password='pw')
        AuthToken.issue(self.user)
        AuthToken.issue(other)  # bob's quota is his own
        self.assertEqual(AuthToken.objects.count(), 2)

    @override_settings(PORTEX_MAX_TOKENS_PER_USER=0)
    def test_zero_quota_means_unlimited(self):
        for _ in range(12):
            AuthToken.issue(self.user)
        self.assertEqual(AuthToken.objects.filter(user=self.user).count(), 12)

    def test_refresh_last_used_folds_redis_marks_into_the_database(self):
        token, _ = AuthToken.issue(self.user)
        self.assertIsNone(token.last_used_at)

        seen = datetime(2026, 5, 1, 12, 0, tzinfo=timezone.utc)
        client = mock.Mock()
        client.mget.return_value = [str(int(seen.timestamp()))]
        with mock.patch('app.signals.redis_client', return_value=client):
            AuthToken.refresh_last_used(self.user)

        token.refresh_from_db()
        self.assertEqual(token.last_used_at, seen)

    def test_refresh_last_used_survives_an_unreachable_redis(self):
        AuthToken.issue(self.user)
        with mock.patch('app.signals.redis_client', return_value=None):
            tokens = AuthToken.refresh_last_used(self.user)
        self.assertEqual(len(tokens), 1)


class ReservedSubdomainTests(TestCase):
    def setUp(self):
        self.user = User.objects.create_user('alice', password='pw')

    def test_name_is_lowercased(self):
        sub = ReservedSubdomain(user=self.user, name='ACME')
        sub.save()
        self.assertEqual(sub.name, 'acme')

    def test_invalid_shapes_are_rejected(self):
        for name in ('-acme', 'acme-', 'a.b', 'a' * 64, 'has space'):
            with self.subTest(name=name):
                with self.assertRaises(ValidationError):
                    ReservedSubdomain(user=self.user, name=name).save()

    def test_duplicate_name_raises_integrity_error(self):
        ReservedSubdomain(user=self.user, name='acme').save()
        other = User.objects.create_user('bob', password='pw')
        with self.assertRaises((IntegrityError, ValidationError)):
            ReservedSubdomain(user=other, name='acme').save()

    @override_settings(PORTEX_MAX_SUBDOMAINS_PER_USER=2)
    def test_quota_is_enforced(self):
        ReservedSubdomain(user=self.user, name='one').save()
        ReservedSubdomain(user=self.user, name='two').save()
        with self.assertRaises(ValidationError) as ctx:
            ReservedSubdomain(user=self.user, name='three').save()
        self.assertIn('limit', ' '.join(ctx.exception.messages).lower())


class DashboardViewTests(TestCase):
    def setUp(self):
        self.user = User.objects.create_user('alice', password='pw-alice-123')
        self.client.force_login(self.user)

    def test_dashboard_requires_login(self):
        self.client.logout()
        response = self.client.get(reverse('dashboard'))
        self.assertEqual(response.status_code, 302)
        self.assertIn(reverse('login'), response['Location'])

    def test_issued_token_is_shown_but_never_stored_in_the_session(self):
        response = self.client.post(reverse('token-issue'), {'name': 'laptop'})
        self.assertEqual(response.status_code, 200)

        token = AuthToken.objects.get(user=self.user)
        body = response.content.decode()
        # The plaintext appears exactly once, in this response.
        shown = [w for w in body.split() if hashlib.sha256(w.encode()).hexdigest() == token.token_hash]
        self.assertTrue(shown, 'the new token should be rendered for the user to copy')
        self.assertNotIn('fresh_token', self.client.session)
        for value in self.client.session.values():
            self.assertNotIn(shown[0], str(value))

    @override_settings(PORTEX_MAX_TOKENS_PER_USER=1)
    def test_hitting_the_token_quota_reports_it_instead_of_erroring(self):
        AuthToken.issue(self.user)
        response = self.client.post(reverse('token-issue'), {'name': 'second'}, follow=True)
        self.assertEqual(response.status_code, 200)
        self.assertContains(response, 'limit reached')

    def test_cannot_revoke_another_users_token(self):
        victim = User.objects.create_user('bob', password='pw')
        token, _ = AuthToken.issue(victim)
        response = self.client.post(reverse('token-revoke', args=[token.pk]))
        self.assertEqual(response.status_code, 404)
        self.assertTrue(AuthToken.objects.filter(pk=token.pk).exists())

    def test_cannot_release_another_users_subdomain(self):
        victim = User.objects.create_user('bob', password='pw')
        sub = ReservedSubdomain(user=victim, name='bobs')
        sub.save()
        response = self.client.post(reverse('subdomain-release', args=[sub.pk]))
        self.assertEqual(response.status_code, 404)
        self.assertTrue(ReservedSubdomain.objects.filter(pk=sub.pk).exists())

    def test_claiming_an_invalid_subdomain_reports_the_reason(self):
        response = self.client.post(reverse('subdomain-claim'), {'name': '-nope'}, follow=True)
        self.assertContains(response, 'Subdomain must be')


@override_settings(CACHES=LOCMEM_CACHE, PORTEX_LOGIN_MAX_ATTEMPTS=3)
class LoginThrottleTests(TestCase):
    def setUp(self):
        from django.core.cache import cache
        cache.clear()
        User.objects.create_user('alice', password='pw-alice-123')

    def _attempt(self, password):
        return self.client.post(reverse('login'), {'username': 'alice', 'password': password})

    def test_repeated_failures_are_throttled(self):
        for _ in range(3):
            self.assertEqual(self._attempt('wrong').status_code, 200)
        self.assertEqual(self._attempt('wrong').status_code, 429)

    def test_throttle_blocks_even_the_correct_password(self):
        for _ in range(3):
            self._attempt('wrong')
        self.assertEqual(self._attempt('pw-alice-123').status_code, 429)

    def test_successful_login_clears_the_counter(self):
        self._attempt('wrong')
        self._attempt('wrong')
        self.assertEqual(self._attempt('pw-alice-123').status_code, 302)
        self.client.logout()
        # Counter reset, so a fresh run of failures is allowed again.
        self.assertEqual(self._attempt('wrong').status_code, 200)

    @override_settings(PORTEX_LOGIN_MAX_ATTEMPTS=0)
    def test_zero_disables_the_throttle(self):
        for _ in range(6):
            self.assertEqual(self._attempt('wrong').status_code, 200)


class RedisSyncSignalTests(TestCase):
    """The gateway authorizes off Redis, so these writes are load-bearing."""

    def setUp(self):
        self.user = User.objects.create_user('alice', password='pw')
        self.client_mock = mock.Mock()

    def test_token_is_written_to_redis_only_after_commit(self):
        with mock.patch('app.signals.redis_client', return_value=self.client_mock):
            with self.captureOnCommitCallbacks(execute=True):
                token, _ = AuthToken.issue(self.user)
                # Nothing yet: the transaction has not committed.
                self.client_mock.set.assert_not_called()
        self.client_mock.set.assert_called_once_with(
            f'token:{token.token_hash}', str(self.user.id)
        )

    def test_revoking_clears_both_the_token_and_its_usage_mark(self):
        token, _ = AuthToken.issue(self.user)
        token_hash = token.token_hash
        with mock.patch('app.signals.redis_client', return_value=self.client_mock):
            with self.captureOnCommitCallbacks(execute=True):
                token.delete()
        deleted = {call.args[0] for call in self.client_mock.delete.call_args_list}
        self.assertEqual(deleted, {f'token:{token_hash}', f'token_used:{token_hash}'})

    def test_a_redis_outage_does_not_break_the_write(self):
        self.client_mock.set.side_effect = RuntimeError('redis down')
        with mock.patch('app.signals.redis_client', return_value=self.client_mock):
            with self.captureOnCommitCallbacks(execute=True):
                AuthToken.issue(self.user)
        # No exception escaped; the row is still there for sync_redis to replay.
        self.assertEqual(AuthToken.objects.count(), 1)


class InstallScriptTests(TestCase):
    def test_installer_is_served_as_a_download(self):
        with mock.patch('app.views.save_downloader') as task:
            response = self.client.get('/install/')
        self.assertEqual(response.status_code, 200)
        self.assertIn('attachment', response['Content-Disposition'])
        self.assertIn('install.sh', response['Content-Disposition'])
        self.assertTrue(b''.join(response.streaming_content).startswith(b'#!'))
        task.delay.assert_called_once()

    def test_a_broken_broker_still_serves_the_installer(self):
        with mock.patch('app.views.save_downloader') as task:
            task.delay.side_effect = RuntimeError('broker down')
            response = self.client.get('/install/')
        self.assertEqual(response.status_code, 200)


class SaveDownloaderTests(TestCase):
    def test_malformed_ip_is_ignored(self):
        # The IP comes from a client-controlled X-Forwarded-For header.
        save_downloader('../../etc/passwd', 'curl')
        self.assertEqual(DownloadApp.objects.count(), 0)

    def test_record_is_bounded_to_the_column_width(self):
        with mock.patch('app.tasks.requests.get') as get:
            get.return_value.json.return_value = {
                'city': 'Tashkent', 'country': 'UZ', 'org': 'X' * 500,
            }
            save_downloader('8.8.8.8', 'A' * 500)
        record = DownloadApp.objects.get()
        self.assertLessEqual(len(record.info), 300)
        self.assertIn('Tashkent', record.info)

    def test_geo_lookup_failure_still_records_the_download(self):
        with mock.patch('app.tasks.requests.get', side_effect=RuntimeError('timeout')):
            save_downloader('8.8.8.8', 'curl')
        self.assertEqual(DownloadApp.objects.count(), 1)
