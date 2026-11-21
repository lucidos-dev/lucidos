"""Run the host fixture, record trusted events and measure guest startup."""

import argparse
import errno
import json
import os
import selectors
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from service import Service

HERE = Path(__file__).resolve().parent
CONTAINER = [
    "docker", "run", "--rm", "-i", "--network", "none",
    "--cap-drop", "ALL", "--cap-add", "SYS_ADMIN", "--cap-add", "SETUID",
    "--cap-add", "SETGID", "--cap-add", "KILL", "--security-opt", "no-new-privileges",
    "--device", "/dev/fuse", "lucidos-phase11-guest",
]
VM = [
    "qemu-system-aarch64", "-machine", "virt", "-cpu", "cortex-a72", "-m", "512",
    "-display", "none", "-serial", "stdio", "-monitor", "none", "-nic", "none",
    "-no-reboot", "-kernel", "/assets/Image", "-initrd", "/assets/fixture.cpio.gz",
    "-append", "console=ttynull quiet rdinit=/phase11-init lsm=landlock",
]


def fixture(root):
    for name in ("workspace", "scratch", "granted", "outside", "workspace/protected"):
        directory = root / name
        directory.mkdir()
        (directory / "seed").write_text("seed")
    (root / "workspace/host-symlink").symlink_to("../outside/seed")
    os.link(root / "outside/seed", root / "workspace/host-hardlink")
    (root.parent / "host-only-sentinel").write_text("outside export")


def service_controls(root, service):
    # A host-side tool can submit exactly the operations the guest submits.
    handle = service.request("open", ["/workspace/tool", "write", ["create"]])["result"]
    assert service.request("write", [handle, "dG9vbA==", 0]) == {"result": 4}
    assert service.request("release", [handle]) == {"result": None}
    assert (root / "workspace/tool").read_text() == "tool"
    denied = service.request("open", ["/outside/tool", "write", ["create"]])
    assert denied == {"errno": errno.EACCES} and not (root / "outside/tool").exists()
    assert service.refusals.pop()["path"] == "/outside/tool"

    # Resolve and open the allowed object, then let a host process move it.
    # Record where the next descriptor write actually landed.
    handle = service.request("open", ["/workspace/tool", "write"])["result"]
    (root / "workspace/tool").rename(root / "outside/moved")
    (root / "workspace/tool").write_text("replacement")
    result = service.request("write", [handle, "ZGF0YQ==", 0])
    assert service.request("release", [handle]) == {"result": None}
    assert (root / "workspace/tool").read_text() == "replacement"
    print(json.dumps({
        "control": "tool and host rename",
        "descriptor_write": result,
        "moved_contents": (root / "outside/moved").read_text(),
    }))


def run(command, expected_abi, disconnect=False):
    with tempfile.TemporaryDirectory(dir=HERE) as base:
        root = Path(base) / "export"
        root.mkdir()
        fixture(root)
        service = Service(root)
        service_controls(root, service)
        start = time.monotonic()
        cidfile = Path(base) / "container-id"
        is_container = command[0] == "docker"
        if is_container:
            command = command[:2] + ["--cidfile", str(cidfile)] + command[2:]
        child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, bufsize=0)
        ready_ms = None
        failures = []
        seen = {}
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(child.stdout, selectors.EVENT_READ)
                pending = bytearray()
                deadline = start + 40
                while selector.select(max(0, deadline - time.monotonic())):
                    fragment = os.read(child.stdout.fileno(), 64 * 1024)
                    if not fragment:
                        break
                    pending.extend(fragment)
                    while b"\n" in pending:
                        line, _, pending = pending.partition(b"\n")
                        try:
                            message = json.loads(line)
                        except json.JSONDecodeError:
                            if is_container:
                                raise
                            # The VM shares its serial line with guest diagnostics.
                            sys.stderr.buffer.write(line + b"\n")
                            sys.stderr.buffer.flush()
                            continue
                        if "op" in message:
                            if disconnect and (
                                message["op"] == "open"
                                and message["args"][0] == "/outside/seed"
                            ):
                                child.wait(timeout=8)
                                assert child.returncode == 70
                                print(json.dumps({"disconnect_status": child.returncode}))
                                return ready_ms
                            response = service.request(message["op"], message["args"])
                            child.stdin.write(json.dumps(response).encode() + b"\n")
                        elif "ready" in message:
                            ready_ms = (time.monotonic() - start) * 1000
                        elif "done" in message:
                            assert not failures, failures
                            assert (root / "outside/seed").read_bytes() == b"seed"
                            assert not (root / "workspace/orphan").exists()
                            if "shell" in seen:
                                for name, data in {"scratch": "temp", "granted": "grant"}.items():
                                    assert (root / name / "result").read_text() == data
                            return ready_ms
                        else:
                            if "kernel" in message and expected_abi is not None:
                                assert message["landlock_abi"] == expected_abi, message
                            if "case" in message:
                                name = message["case"]
                                seen[name] = True
                                message["refusals"] = service.refusals[:]
                                service.refusals.clear()
                                expected = {
                                    "shell": 1, "python": 6, "background": 1, "links": 7,
                                }.get(name, 0)
                                if name == "cli":
                                    assert json.loads(message["stdout"]) == {"phase11": "fixture-ok"}, message
                                if name in {"shell", "python", "background"}:
                                    assert message["refusals"][0]["path"] == "/outside/seed", message
                                if message["status"] != 0 or len(message["refusals"]) != expected:
                                    failures.append(name)
                            print(json.dumps(message), flush=True)
                raise RuntimeError(f"guest stopped before DONE (status {child.poll()})")
        finally:
            if child.poll() is None:
                # For QEMU this kills the whole guest. For Docker, the trusted
                # runner exits when this pipe closes or its case times out.
                if child.stdin and not child.stdin.closed:
                    child.stdin.close()
                if command[0] == "qemu-system-aarch64":
                    child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    if is_container and cidfile.exists():
                        subprocess.run(
                            ["docker", "rm", "-f", cidfile.read_text().strip()],
                            check=False, stdout=subprocess.DEVNULL,
                        )
                    child.kill()
                    child.wait()
            service.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--vm", action="store_true")
    parser.add_argument("--expect-abi", type=int)
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--disconnect", action="store_true")
    parser.add_argument("cases", nargs="*")
    args = parser.parse_args()
    if args.vm and (args.disconnect or args.cases):
        parser.error("the VM always runs the entire suite")
    if args.disconnect and args.cases != ["shell"]:
        parser.error("use --disconnect shell to drop the response at the denied open")
    if args.repeat < 1:
        parser.error("--repeat must be positive")
    command = VM if args.vm else CONTAINER + args.cases
    samples = [run(command, args.expect_abi, args.disconnect) for _ in range(args.repeat)]
    if all(sample is not None for sample in samples):
        print(json.dumps({"mount_ready_ms": samples, "median_ms": statistics.median(samples)}))


if __name__ == "__main__":
    main()
