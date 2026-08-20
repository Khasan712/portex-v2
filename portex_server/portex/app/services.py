"""Small helpers shared across the app."""

import calendar
import os
from datetime import datetime
from pathlib import Path


def get_log_dir(base_dir):
    """Return the directory for today's log file, creating it if needed.

    Logs live under `LOG_DIR` (default `<project>/logs`), split by year and
    month. This used to build the path from the filesystem root, which meant
    it wrote to `/logs` — outside any mounted volume, so the files vanished on
    every container restart, and it failed outright on a read-only root.

    Falls back to `base_dir` if the directory cannot be created; logging
    configuration must never be the reason the process won't boot.
    """
    root = Path(os.getenv('LOG_DIR') or Path(base_dir) / 'logs')
    now = datetime.now()
    directory = root / str(now.year) / calendar.month_name[now.month]
    try:
        directory.mkdir(parents=True, exist_ok=True)
        return str(directory)
    except OSError as exc:
        print(f'Cannot create log directory {directory} ({exc}); falling back to {base_dir}')
        return str(base_dir)
