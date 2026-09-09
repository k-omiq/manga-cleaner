"""Parent process watchdog for Manga Cleaner sidecar.

The sidecar process holds multi-gigabyte neural network weights and GPU allocations
in unified memory. If the parent application crashes or is terminated abruptly,
the operating system does not automatically terminate child background processes
that are listening on network sockets.

This watchdog thread polls the parent process ID at regular intervals and forces
an immediate exit (`os._exit(0)`) the moment the parent process disappears.
"""

from __future__ import annotations

import os
import sys
import threading
import time
from typing import Optional


def start_parent_watchdog(parent_pid: Optional[int], poll_interval_sec: float = 1.0) -> None:
    """Start a background daemon thread that monitors the parent PID."""
    if parent_pid is None or parent_pid <= 1:
        return

    def _watchdog_loop() -> None:
        while True:
            time.sleep(poll_interval_sec)

            # Check if parent PID is still alive by sending signal 0
            try:
                os.kill(parent_pid, 0)
            except OSError:
                # The parent process no longer exists. Terminate immediately without
                # running standard Python exit handlers to prevent hanging on locks.
                os._exit(0)

            # On POSIX systems, if the parent process dies, the child process is
            # immediately adopted by init or launchd (PID 1). If getppid() no longer
            # equals the original parent_pid, the original parent has exited.
            if sys.platform != "win32":
                try:
                    if os.getppid() != parent_pid:
                        os._exit(0)
                except Exception:
                    pass

    thread = threading.Thread(
        target=_watchdog_loop,
        name="manga-cleaner-parent-watchdog",
        daemon=True,
    )
    thread.start()
