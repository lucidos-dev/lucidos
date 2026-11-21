"""Host side of the Phase 11 fixture protocol. The guest receives no host mount."""

import base64
import errno
import os
import stat
from contextlib import contextmanager
from pathlib import PurePosixPath

WRITABLE = {"/workspace", "/scratch", "/granted"}
PRIVATE = "/workspace/protected"
DIRECTORY = os.O_DIRECTORY | os.O_NOFOLLOW
MAX_TRANSFER = 128 * 1024


class Refusal(OSError):
    def __init__(self, path, reason):
        super().__init__(errno.EACCES, reason, path)
        self.reason = reason


def within(path, directory):
    return path == directory or path.startswith(directory + "/")


class Service:
    def __init__(self, root):
        self.root_fd = os.open(root, DIRECTORY)
        self.handles = {}
        self.next_handle = 0
        self.refusals = []

    def close(self):
        for fd, _, _ in self.handles.values():
            os.close(fd)
        os.close(self.root_fd)

    def request(self, op, args):
        operations = {
            "getattr": self.getattr, "readdir": self.readdir,
            "open": self.open, "read": self.read, "write": self.write,
            "release": self.release, "truncate": self.truncate,
            "chmod": self.chmod, "chown": self.chown, "utimens": self.utimens,
            "setxattr": self.setxattr, "removexattr": self.setxattr,
            "unlink": self.unlink, "rename": self.rename, "link": self.link,
            "mkdir": self.mkdir, "rmdir": self.rmdir, "symlink": self.symlink,
        }
        try:
            return {"result": operations[op](*args)}
        except Refusal as exc:
            candidate = None
            if exc.reason == "outside scope":
                candidate = self.existing_parent(exc.filename)
            self.refusals.append({
                "op": op, "path": exc.filename, "reason": exc.reason,
                "folder": candidate,
            })
            return {"errno": exc.errno}
        except OSError as exc:
            return {"errno": exc.errno}

    @contextmanager
    def parent(self, path):
        parts = PurePosixPath(path).parts
        if not parts or parts[0] != "/" or ".." in parts:
            raise OSError(errno.EINVAL, "invalid path")
        fd = os.dup(self.root_fd)
        try:
            for part in parts[1:-1]:
                next_fd = os.open(part, DIRECTORY, dir_fd=fd)
                os.close(fd)
                fd = next_fd
            yield fd, parts[-1] if len(parts) > 1 else "."
        finally:
            os.close(fd)

    def existing_parent(self, path):
        for parent in PurePosixPath(path).parents:
            name = str(parent)
            if name == "/":
                break
            try:
                with self.parent(name) as (fd, leaf):
                    info = os.stat(leaf, dir_fd=fd, follow_symlinks=False)
                    if stat.S_ISDIR(info.st_mode):
                        return name
            except OSError:
                continue
        return None

    def scope(self, path, *, subtree=False, write=True):
        if within(path, PRIVATE) or (subtree and within(PRIVATE, path)):
            raise Refusal(path, "protected path")
        if write and not any(within(path, root) for root in WRITABLE):
            raise Refusal(path, "outside scope")

    def check_identity(self, path, info):
        if stat.S_ISLNK(info.st_mode) or (
            stat.S_ISREG(info.st_mode) and info.st_nlink != 1
        ):
            raise Refusal(path, "linked object")

    def check_entry(self, path, parent, leaf):
        try:
            info = os.stat(leaf, dir_fd=parent, follow_symlinks=False)
        except FileNotFoundError:
            return
        self.check_identity(path, info)

    def getattr(self, path):
        self.scope(path, write=False)
        with self.parent(path) as (fd, leaf):
            info = os.stat(leaf, dir_fd=fd, follow_symlinks=False)
        # Hide host symlinks and private contents. An attempted mutation still
        # reaches the service for checks on operations that don't need lookup.
        if stat.S_ISLNK(info.st_mode):
            raise Refusal(path, "linked object")
        return {
            key: getattr(info, key) for key in (
                "st_mode", "st_nlink", "st_size", "st_atime", "st_mtime",
                "st_ctime", "st_ino", "st_uid", "st_gid",
            )
        }

    def readdir(self, path):
        self.scope(path, write=False)
        with self.parent(path) as (parent, leaf):
            fd = os.open(leaf, DIRECTORY, dir_fd=parent)
            try:
                names = os.listdir(fd)
            finally:
                os.close(fd)
        return [".", ".."] + [
            name for name in sorted(names)
            if not within(path.rstrip("/") + "/" + name, PRIVATE)
        ]

    def open(self, path, access, options=(), mode=None):
        # Truncate only after checking the opened object. The actual write and
        # ftruncate operate on the same descriptor that passed the check.
        write = access != "read" or "create" in options or "truncate" in options
        self.scope(path, write=write)
        with self.parent(path) as (parent, leaf):
            if write:
                self.check_entry(path, parent, leaf)
            safe_flags = {"read": os.O_RDONLY, "write": os.O_WRONLY, "read-write": os.O_RDWR}[access]
            for name, bit in (("append", os.O_APPEND), ("create", os.O_CREAT), ("exclusive", os.O_EXCL)):
                if name in options:
                    safe_flags |= bit
            fd = os.open(
                leaf, safe_flags | os.O_NOFOLLOW | os.O_NONBLOCK,
                (mode or 0o600) & 0o777, dir_fd=parent,
            )
        try:
            info = os.fstat(fd)
            if not stat.S_ISREG(info.st_mode):
                raise OSError(errno.EINVAL, "regular file required")
            if write:
                self.check_identity(path, info)
            if "truncate" in options:
                os.ftruncate(fd, 0)
        except BaseException:
            os.close(fd)
            raise
        self.next_handle += 1
        self.handles[self.next_handle] = (fd, path, write)
        return self.next_handle

    def read(self, handle, size, offset):
        fd, _, _ = self.handles[handle]
        return base64.b64encode(os.pread(fd, min(size, MAX_TRANSFER), offset)).decode()

    def write(self, handle, data, offset):
        fd, path, writable = self.handles[handle]
        if not writable:
            raise Refusal(path, "read-only handle")
        self.check_identity(path, os.fstat(fd))
        return os.pwrite(fd, base64.b64decode(data), offset)

    def release(self, handle):
        fd, _, _ = self.handles.pop(handle)
        os.close(fd)

    def change_file(self, path, operation):
        self.scope(path, subtree=True)
        with self.parent(path) as (parent, leaf):
            fd = os.open(leaf, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
            try:
                self.check_identity(path, os.fstat(fd))
                operation(fd)
            finally:
                os.close(fd)

    def truncate(self, path, length, handle=None):
        self.scope(path, subtree=True)
        if handle is None:
            handle = self.open(path, "write")
            try:
                self.truncate(path, length, handle)
            finally:
                self.release(handle)
        else:
            fd, opened_path, writable = self.handles[handle]
            if not writable:
                raise Refusal(opened_path, "read-only handle")
            self.check_identity(opened_path, os.fstat(fd))
            os.ftruncate(fd, length)

    def chmod(self, path, mode):
        self.change_file(path, lambda fd: os.fchmod(fd, mode & 0o777))

    def chown(self, path, uid, gid):
        self.scope(path, subtree=True)
        raise OSError(errno.ENOSYS, "ownership changes are not implemented in the fixture")

    def utimens(self, path, times):
        if times is not None:
            times = tuple(times)
        self.change_file(path, lambda fd: os.utime(fd, times))

    def setxattr(self, path, *_):
        self.scope(path, subtree=True)
        raise OSError(errno.ENOSYS, "xattr changes are not implemented in the fixture")

    def unlink(self, path):
        self.scope(path, subtree=True)
        with self.parent(path) as (fd, leaf):
            self.check_entry(path, fd, leaf)
            os.unlink(leaf, dir_fd=fd)

    def rename(self, source, target):
        for path in (source, target):
            self.scope(path, subtree=True)
        with self.parent(source) as (src_fd, src), self.parent(target) as (dst_fd, dst):
            self.check_entry(source, src_fd, src)
            self.check_entry(target, dst_fd, dst)
            os.rename(src, dst, src_dir_fd=src_fd, dst_dir_fd=dst_fd)

    def link(self, source, target):
        for path in (source, target):
            self.scope(path, subtree=True)
        raise Refusal(target, "link creation disabled")

    def symlink(self, path, target):
        self.scope(path)
        raise Refusal(path, "link creation disabled")

    def mkdir(self, path, mode):
        self.scope(path)
        with self.parent(path) as (fd, leaf):
            os.mkdir(leaf, mode & 0o777, dir_fd=fd)

    def rmdir(self, path):
        self.scope(path, subtree=True)
        with self.parent(path) as (fd, leaf):
            os.rmdir(leaf, dir_fd=fd)
