"""Synthetic workloads run without the broker's descriptors or privileges."""

DENIALS = """
import errno, os
def refused(operation, *args):
    try:
        operation(*args)
    except OSError as error:
        assert error.errno == errno.EACCES, error
    else:
        raise AssertionError(operation)
"""

CASES = {
    "shell": ("sh", "-ec", """
        test "$PWD" = /fixture/workspace
        cat seed
        printf shell > result
        printf temp > /fixture/scratch/result
        printf grant > /fixture/granted/result
        printf local > /tmp/local
        if printf denied > /fixture/outside/seed; then false; fi
        exit 0
    """),
    "python": ("python3", "-c", DENIALS + """
from pathlib import Path
with open('result', 'wb') as output:
    output.write(b'python')
    output.flush()
    os.ftruncate(output.fileno(), 3)
    os.fchmod(output.fileno(), 0o640)
assert Path('result').read_bytes() == b'pyt'
os.utime('result', (1, 1))
assert Path('result').stat().st_mtime == 1
refused(Path('/fixture/outside/seed').write_text, 'denied')
refused(os.chmod, '/fixture/outside/seed', 0o600)
refused(os.chown, '/fixture/outside/seed', os.getuid(), os.getgid())
refused(os.setxattr, '/fixture/outside/seed', 'user.phase11', b'denied')
refused(os.utime, '/fixture/outside/seed', (1, 1))
with open('/fixture/outside/seed', 'rb') as source:
    refused(os.fchmod, source.fileno(), 0o600)
    try:
        os.write(source.fileno(), b'denied')
    except OSError as error:
        assert error.errno == errno.EBADF
    else:
        raise AssertionError('read-only descriptor was writable')
"""),
    "background": ("sh", "-ec", """
        ( sleep 0.05
          if printf denied > /fixture/outside/seed; then false; fi
        ) &
        wait "$!"
        ( sleep 30; printf orphan > /fixture/workspace/orphan ) &
    """),
    "links": ("python3", "-c", DENIALS + """
refused(os.rename, 'seed', '/fixture/outside/seed')
refused(os.link, '/fixture/outside/seed', 'link')
refused(os.symlink, '/fixture/outside/seed', 'sym')
refused(os.open, 'host-symlink', os.O_WRONLY)
refused(os.open, 'host-hardlink', os.O_WRONLY | os.O_TRUNC)
refused(os.rename, '/fixture/workspace', '/fixture/scratch/moved')
refused(os.stat, 'protected/seed')
"""),
    "ordinary": ("python3", "-c", """
from pathlib import Path
try:
    Path('absent').read_bytes()
except FileNotFoundError:
    pass
else:
    raise AssertionError('expected ENOENT')
"""),
    "surface": ("python3", "-c", """
import os
from pathlib import Path
assert os.getuid() == 65534
assert Path('/proc/self/status').read_text().split('CapEff:', 1)[1].split()[0] == '0000000000000000'
assert not Path('/host-only-sentinel').exists()
assert not Path('/var/run/docker.sock').exists()
assert not Path('/dev/fuse').exists() or not os.access('/dev/fuse', os.R_OK)
assert not os.access('/proc/1/fd/0', os.R_OK)
assert not os.path.exists('/proc/self/fd/3')
"""),
    "cli": (
        "/opt/glibc/ld-linux-aarch64.so.1", "--library-path", "/opt/glibc",
        "/runner/lucidos", "events", "query", "--limit", "1",
    ),
    "socket": ("python3", "-c", """
import socket
with socket.socket(socket.AF_UNIX) as stream:
    stream.connect('/run/phase11.sock')
    stream.sendall(b'status')
    assert stream.recv(16) == b'fixture-ok'
"""),
}
