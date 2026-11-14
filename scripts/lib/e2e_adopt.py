"""Adopt a new E2E slot without replacing another credential generation."""

import fcntl
import json
import os
from pathlib import Path
import ssl
import sys
import tempfile
import urllib.request


def adopt(base, directory, slug, port, token_file, transport=None):
    directory = Path(directory)
    if directory.name != slug or not slug or any(
        c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
        for c in slug
    ):
        raise ValueError("invalid E2E workspace identity")
    folder = directory.parent / ".cleanup-owners"
    folder.mkdir(exist_ok=True)
    # Same flock-backed lock as Rust File::try_lock on macOS/Linux. Never
    # unlink its inode, including after a credential generation is retired.
    with (folder / (slug + ".lock")).open("a+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        headers = {"content-type": "application/json"}
        if Path(token_file).is_file():
            headers["x-lucidos-local-token"] = Path(token_file).read_text().strip()
        opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({}),
            urllib.request.HTTPSHandler(context=ssl._create_unverified_context()),
        )

        def request(method, path, body=None):
            req = urllib.request.Request(
                base.rstrip("/") + "/~/api/v1/control/" + path,
                data=None if body is None else json.dumps(body).encode(),
                headers=headers,
                method=method,
            )
            if transport is not None:
                return transport(req)
            with opener.open(req, timeout=10) as response:
                return json.load(response)

        listed = request("GET", "workspaces").get("workspaces")
        if not isinstance(listed, list) or any(
            not isinstance(ws, dict) or not isinstance(ws.get("id"), str)
            for ws in listed
        ):
            raise ValueError("invalid gateway registry observation")
        existing = [ws for ws in listed if ws["id"] == slug]
        if existing:
            if len(existing) != 1 or existing[0].get("port") != port:
                raise ValueError("existing E2E registration has another port")
            return  # Existing protection and owner credentials stay unchanged.
        target = folder / (slug + ".json")
        if os.path.lexists(target):
            raise ValueError("unarchived E2E owner credential exists")
        payload = request("POST", "workspaces/adopt", {
            "dir": str(directory), "port": port, "origin": "e2e", "ephemeral": True,
        })
        workspace = payload.get("workspace", {})
        secret = payload.get("owner_token")
        if (workspace.get("id") != slug or workspace.get("port") != port
                or not isinstance(secret, str) or not secret):
            raise ValueError("E2E adoption returned no matching owner credential")
        fd, temporary = tempfile.mkstemp(dir=folder)
        try:
            with os.fdopen(fd, "w") as output:
                json.dump({"workspace_id": slug, "dir": str(directory),
                           "owner_token": secret}, output)
                output.flush()
                os.fsync(output.fileno())
            os.link(temporary, target)  # Atomic, no replacement; mkstemp is 0600.
            for durable_directory in (folder, folder.parent):
                directory_fd = os.open(durable_directory, os.O_RDONLY)
                try:
                    os.fsync(directory_fd)
                finally:
                    os.close(directory_fd)
        finally:
            os.unlink(temporary)


if __name__ == "__main__":
    try:
        adopt(sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), sys.argv[5])
    except Exception:
        # HTTP errors and response bodies must never expose the owner token.
        print("ERROR: E2E adoption or owner publication failed; resources retained", file=sys.stderr)
        sys.exit(1)
