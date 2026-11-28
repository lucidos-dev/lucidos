"""Record owned package processes before shutdown and verify their release."""

import json
from pathlib import Path
import socket
import subprocess
import sys
import time


def processes():
    result = subprocess.run(
        ["ps", "-axo", "pid=,pgid=,stat=,lstart="],
        check=True, capture_output=True, text=True,
    )
    rows = {}
    for line in result.stdout.splitlines():
        pid, group, state, started = line.split(None, 3)
        if not state.startswith("Z"):
            rows[int(pid)] = (int(group), started)
    return rows


def snapshot(scope, port):
    scope = Path(scope)
    rows = processes()
    owners = set()
    for name in ("supervisor.pid", "gateway.pid", "gateway/pgdata/postmaster.pid"):
        path = scope / name
        if path.exists():
            owners.add(int(path.read_text().splitlines()[0]))
    # Postgres may daemonize into its own group. Track its workers too.
    groups = {rows[pid][0] for pid in owners if pid in rows}
    owned = {str(pid): started for pid, (pgid, started) in rows.items()
             if pid in owners or pgid in groups}
    ports = {int(port)}
    for path in (scope / "gateway/workspaces").glob("*/.lucidos/ports"):
        for line in path.read_text().splitlines():
            if line.startswith("API_PORT="):
                ports.add(int(line.split("=", 1)[1]))
    return {"processes": owned, "ports": sorted(ports)}


def remaining(evidence, rows):
    return [pid for pid, started in evidence["processes"].items()
            if int(pid) in rows and rows[int(pid)][1] == started]


def verify(scope, evidence, timeout=45):
    deadline = time.monotonic() + timeout
    while True:
        live = remaining(evidence, processes())
        bound = []
        for port in evidence["ports"]:
            with socket.socket() as client:
                client.settimeout(0.2)
                if client.connect_ex(("127.0.0.1", port)) == 0:
                    bound.append(port)
        pg = (Path(scope) / "gateway/pgdata/postmaster.pid").exists()
        if not live and not bound and not pg:
            return
        if time.monotonic() >= deadline:
            raise RuntimeError(f"cleanup failed: processes={live}, listeners={bound}, postmaster.pid={pg}")
        time.sleep(0.2)


if __name__ == "__main__":
    mode, root, arg = sys.argv[1:]
    if mode == "snapshot":
        print(json.dumps(snapshot(root, arg)))
    elif mode == "verify":
        verify(root, json.loads(Path(arg).read_text()))
        print("PASS: owned package processes, engine/gateway listeners and Postgres released")
    else:
        raise RuntimeError("unknown lifecycle operation")
