#!/usr/bin/env python3
"""A `cargo` shim for the comment-ablation sandboxes: one build at a time.

The harness copies this file into the sandbox root as `cargo` and puts that
directory first on PATH. A building subcommand takes an exclusive flock on
$CARGO_LOCK_FILE, logs how long it waited to $CARGO_LOCK_WAIT_LOG, then runs
the real cargo as a child and exits with its status.

The shim holds the lock itself and never passes the descriptor on. cargo's
descendants include the sccache server, which daemonizes and outlives the
build. An inherited descriptor kept the lock held for good, and every later
build in every sandbox waited forever.
"""

import fcntl
import os
import shutil
import signal
import subprocess
import sys
import time

BUILDING = {"build", "b", "test", "t", "check", "c", "clippy", "run", "r", "bench", "doc", "install"}


def real_cargo() -> str:
    here = os.path.dirname(os.path.realpath(__file__))
    path = os.pathsep.join(
        p for p in os.environ.get("PATH", "").split(os.pathsep) if os.path.realpath(p) != here
    )
    found = shutil.which("cargo", path=path)
    if not found:
        sys.exit("cargo shim: no real cargo on PATH")
    return found


def subcommand(args: list) -> str:
    for a in args:
        if not a.startswith(("+", "-")):
            return a
    return ""


def main() -> None:
    args = sys.argv[1:]
    target = real_cargo()
    lock_path = os.environ.get("CARGO_LOCK_FILE")
    if not lock_path or subcommand(args) not in BUILDING:
        os.execv(target, [target] + args)

    # O_CLOEXEC, and Popen closes it too: the child never sees this descriptor.
    fd = os.open(lock_path, os.O_CREAT | os.O_RDWR | os.O_CLOEXEC, 0o644)
    started = time.monotonic()
    fcntl.flock(fd, fcntl.LOCK_EX)
    waited = time.monotonic() - started
    log = os.environ.get("CARGO_LOCK_WAIT_LOG")
    if log:
        with open(log, "a") as f:
            f.write(f"{time.time():.0f}\t{waited:.1f}\t{subcommand(args)}\n")

    child = subprocess.Popen([target] + args, close_fds=True)
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, lambda s, _frame: child.send_signal(s))
    code = child.wait()
    sys.exit(128 - code if code < 0 else code)


if __name__ == "__main__":
    main()
