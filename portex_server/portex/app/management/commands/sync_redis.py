"""Rebuild the Redis auth index the Rust gateway reads.

The index is written incrementally by post_save/post_delete signals. It can
still drift from the database — a Redis restart without persistence, a
flushed database, an outage that swallowed a write. When it does, every
token and subdomain stops authorizing and there is otherwise no way back
short of re-saving every row by hand.

    manage.py sync_redis            # write every key the database knows about
    manage.py sync_redis --prune    # also drop keys with no matching row
    manage.py sync_redis --dry-run  # report what would change, touch nothing
"""

from django.core.management.base import BaseCommand

from app.models import AuthToken, ReservedSubdomain
from app.signals import redis_client, subdomain_key, token_key


class Command(BaseCommand):
    help = 'Rebuild the Redis token/subdomain index from the database.'

    def add_arguments(self, parser):
        parser.add_argument(
            '--prune',
            action='store_true',
            help='Delete token:/sub: keys that have no matching database row.',
        )
        parser.add_argument(
            '--dry-run',
            action='store_true',
            help='Report the changes without applying them.',
        )

    def handle(self, *args, **options):
        prune = options['prune']
        dry_run = options['dry_run']

        client = redis_client()
        if client is None:
            self.stderr.write(self.style.ERROR('Could not build a Redis client — check REDIS_* settings.'))
            return
        try:
            client.ping()
        except Exception as exc:
            self.stderr.write(self.style.ERROR(f'Redis is unreachable: {exc}'))
            return

        expected = {
            token_key(token_hash): str(user_id)
            for token_hash, user_id in AuthToken.objects.values_list('token_hash', 'user_id')
        }
        expected.update({
            subdomain_key(name): str(user_id)
            for name, user_id in ReservedSubdomain.objects.values_list('name', 'user_id')
        })

        written = 0
        for key, value in expected.items():
            if client.get(key) == value:
                continue
            if not dry_run:
                client.set(key, value)
            written += 1

        pruned = 0
        if prune:
            # token_used:* is written by the gateway, not derived from the
            # database, so it is pruned by its token key instead.
            for pattern in ('token:*', 'sub:*', 'token_used:*'):
                for key in client.scan_iter(match=pattern, count=500):
                    if key in expected:
                        continue
                    if key.startswith('token_used:'):
                        if f'token:{key.removeprefix("token_used:")}' in expected:
                            continue
                    if not dry_run:
                        client.delete(key)
                    pruned += 1

        prefix = 'Would sync' if dry_run else 'Synced'
        self.stdout.write(self.style.SUCCESS(
            f'{prefix} {len(expected)} keys: {written} written, {pruned} pruned.'
        ))
        if not prune:
            self.stdout.write('Stale keys left in place — pass --prune to remove them.')
