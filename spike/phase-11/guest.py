"""Guest filesystem frontend and root supervisor. Standard I/O is the control pipe."""

import base64
import ctypes
import errno
import json
import os
import select
import signal
import socket
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from fuse import FUSE, FuseOSError, Operations

from workloads import CASES


def send(message):
    print(json.dumps(message, separators=(",", ":")), flush=True)


channel_lost = threading.Event()


class Mount(Operations):
    def remote(self, op, *args):
        if channel_lost.is_set():
            raise FuseOSError(errno.EIO)
        try:
            send({"op": op, "args": args})
            if not select.select([sys.stdin], [], [], 3)[0]:
                raise TimeoutError
            response = sys.stdin.readline()
            if not response:
                raise EOFError
        except (OSError, EOFError):
            channel_lost.set()
            # Finish the blocked syscall before the PID namespace is torn down.
            raise FuseOSError(errno.EIO)
        decoded = json.loads(response)
        if "errno" in decoded:
            raise FuseOSError(decoded["errno"])
        return decoded["result"]

    def getattr(self, path, fh=None):
        return self.remote("getattr", path)

    def readdir(self, path, fh):
        return self.remote("readdir", path)

    def open(self, path, flags):
        access = {
            os.O_RDONLY: "read", os.O_WRONLY: "write", os.O_RDWR: "read-write",
        }[flags & os.O_ACCMODE]
        options = [
            name for bit, name in (
                (os.O_APPEND, "append"), (os.O_CREAT, "create"),
                (os.O_EXCL, "exclusive"), (os.O_TRUNC, "truncate"),
            ) if flags & bit
        ]
        return self.remote("open", path, access, options)

    def create(self, path, mode, fi=None):
        return self.remote("open", path, "read-write", ["create", "exclusive"], mode)

    def read(self, path, size, offset, fh):
        return base64.b64decode(self.remote("read", fh, size, offset))

    def write(self, path, data, offset, fh):
        return self.remote("write", fh, base64.b64encode(data).decode(), offset)

    def release(self, path, fh):
        return self.remote("release", fh) or 0

    def truncate(self, path, length, fh=None):
        return self.remote("truncate", path, length, fh)

    def chmod(self, path, mode):
        return self.remote("chmod", path, mode)

    def chown(self, path, uid, gid):
        return self.remote("chown", path, uid, gid)

    def utimens(self, path, times=None):
        return self.remote("utimens", path, times)

    def setxattr(self, path, name, value, options, position=0):
        return self.remote("setxattr", path, name)

    def removexattr(self, path, name):
        return self.remote("removexattr", path, name)

    def rename(self, old, new):
        return self.remote("rename", old, new)

    def link(self, target, source):
        return self.remote("link", source, target)

    def symlink(self, target, source):
        return self.remote("symlink", target, source)

    def mkdir(self, path, mode):
        return self.remote("mkdir", path, mode)

    def rmdir(self, path):
        return self.remote("rmdir", path)

    def unlink(self, path):
        return self.remote("unlink", path)


class MockAPI(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/api/v1/events/query?limit=1":
            self.send_error(404)
            return
        body = b'{"phase11":"fixture-ok"}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def work():
    for _ in range(250):
        # mountinfo is local. Avoid attributing a broker request to readiness.
        if " /fixture " in Path("/proc/self/mountinfo").read_text():
            break
        time.sleep(0.02)
    else:
        os._exit(71)
    send({"ready": True})
    with socket.socket(socket.AF_UNIX) as listener, HTTPServer(("127.0.0.1", 0), MockAPI) as api:
        threading.Thread(target=api.serve_forever, daemon=True).start()
        listener.bind("/run/phase11.sock")
        os.chmod("/run/phase11.sock", 0o666)
        listener.listen()

        def fixture_endpoint():
            conn, _ = listener.accept()
            with conn:
                if conn.recv(16) == b"status":
                    conn.sendall(b"fixture-ok")

        threading.Thread(target=fixture_endpoint, daemon=True).start()
        for name in sys.argv[1:] or CASES:
            started = time.monotonic()
            child = subprocess.Popen(
                CASES[name], cwd="/fixture/workspace", stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, user=65534, group=65534,
                extra_groups=[], start_new_session=True,
                env={
                    "PATH": "/usr/bin:/bin", "TMPDIR": "/tmp",
                    "LUCIDOS_WORKSPACE": "/fixture/workspace",
                    "LUCIDOS_API_BASE_URL": f"http://127.0.0.1:{api.server_port}",
                },
            )
            try:
                status = child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                status = None
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            stdout, stderr = child.communicate(timeout=2)
            if channel_lost.is_set():
                os._exit(70)
            send({
                "case": name, "status": status, "ms": (time.monotonic() - started) * 1000,
                "stdout": stdout.decode(errors="replace"),
                "stderr": stderr.decode(errors="replace"),
            })
    # The parent namespace/container or VM owns complete reaping.
    send({"done": True})
    os._exit(0)


def guarded_work():
    try:
        work()
    except BaseException:
        if channel_lost.is_set():
            os._exit(70)
        import traceback
        traceback.print_exc()
        os._exit(72)


def main():
    Path("/fixture").mkdir(exist_ok=True)
    if Path("/dev/fuse").exists():
        os.chmod("/dev/fuse", 0o600)
    libc = ctypes.CDLL(None, use_errno=True)
    abi = libc.syscall(444, ctypes.c_void_p(), ctypes.c_size_t(0), 1)
    send({
        "kernel": os.uname().release, "arch": os.uname().machine,
        "landlock_abi": abi, "landlock_errno": ctypes.get_errno() if abi < 0 else 0,
        "mounts": [entry.split()[4] for entry in Path("/proc/self/mountinfo").read_text().splitlines()],
    })
    threading.Thread(target=guarded_work, daemon=True).start()
    FUSE(Mount(), "/fixture", foreground=True, nothreads=True, allow_other=True,
         attr_timeout=0, entry_timeout=0, negative_timeout=0,
         direct_io=True, nosuid=True, nodev=True)


if __name__ == "__main__":
    main()
