"""Small helpers shared across the app."""

import calendar
import os
from datetime import datetime
from pathlib import Path


def _usable(directory: Path) -> bool:
    """True if we can create `directory` and actually write inside it.

    `os.access` is not enough: the directory can exist and look writable
    while the container user still cannot create files in it.
    """
    try:
        directory.mkdir(parents=True, exist_ok=True)
        probe = directory / '.write-probe'
        probe.touch()
        probe.unlink()
        return True
    except OSError:
        return False


def get_log_dir(base_dir):
    """Return a writable directory for today's log file, or None.

    Logs are split by year and month under `LOG_DIR` (default
    `/var/log/portex` in the image, which is outside the source tree so a
    bind-mounted checkout cannot make it unwritable), falling back to
    `<project>/logs` for a plain local checkout.

    Returns None when nothing is writable. Callers must treat that as "log
    to the console only" — a logging path must never be the reason the
    process refuses to boot.
    """
    now = datetime.now()
    suffix = Path(str(now.year)) / calendar.month_name[now.month]

    candidates = []
    if os.getenv('LOG_DIR'):
        candidates.append(Path(os.environ['LOG_DIR']))
    candidates.append(Path(base_dir) / 'logs')

    for root in candidates:
        directory = root / suffix
        if _usable(directory):
            return str(directory)

    print(f'No writable log directory among {[str(c) for c in candidates]}; logging to console only')
    return None
