"""Assertions for e2e-packaged.sh's already-running, disposable workspace."""

import json
import secrets
import ssl
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def credential_helper():
    """Single migration seam for script credential delivery and assertion."""
    return '''def credential():
    import os
    value = os.environ.get("CRED_PACKAGED_SMOKE")
    if not value:
        raise RuntimeError("packaged script did not receive its credential")
    return value
'''


def run(base, token_file, cafile):
    token = Path(token_file).read_text().strip()
    if not token:
        raise RuntimeError("missing disposable gateway token")
    secret = secrets.token_hex(24)
    nonce = secrets.token_hex(16)
    received = set()
    lock = threading.Lock()

    class Upstream(BaseHTTPRequestHandler):
        def do_GET(self):
            valid = self.headers.get("Authorization") == "Bearer " + secret
            valid = valid and self.path in ("/handshake", "/python")
            if valid:
                with lock:
                    received.add(self.path)
            self.send_response(200 if valid else 401)
            self.end_headers()
            self.wfile.write((nonce if valid else "authentication failed").encode())

        def log_message(self, *_args):
            pass

    # No proxy discovery: all traffic belongs to this disposable local test.
    # Trusts only the scope's loopback CA, like scripts/lib/loopback_curl.sh.
    client = urllib.request.build_opener(
        urllib.request.ProxyHandler({}),
        urllib.request.HTTPSHandler(context=ssl.create_default_context(cafile=cafile)),
    )

    def request(path, method="GET", body=None, text=False):
        headers = {"x-lucidos-local-token": token}
        if body is not None:
            headers["Content-Type"] = "text/plain" if text else "application/json"
            body = body.encode() if text else json.dumps(body).encode()
        req = urllib.request.Request(base + "/api/v1/" + path, body, headers, method=method)
        try:
            with client.open(req, timeout=45) as response:
                raw = response.read().decode()
        except urllib.error.HTTPError as error:
            detail = error.read().decode(errors="replace").replace(secret, "[synthetic secret]")
            raise RuntimeError(f"{method} {path}: HTTP {error.code}: {detail}") from None
        result = raw if text else json.loads(raw)
        if isinstance(result, dict) and result.get("success") is False:
            raise RuntimeError(f"{method} {path}: {result}".replace(secret, "[synthetic secret]"))
        return result

    server = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    upstream = f"http://127.0.0.1:{server.server_port}"
    try:
        request("credentials", "POST", {
            "service_name": "packaged-smoke", "base_url": upstream,
            "auth_type": "api_key", "auth_value": secret,
        })
        script = "scripts/auth/packaged-smoke.py"
        request("data/" + script, "PUT", credential_helper() + '''
import json
print(json.dumps({"headers": {"Authorization": "Bearer " + credential()}, "expires_in": 60}))
''', text=True)
        request("data/config/apis.json", "PUT", json.dumps({"packaged-smoke": {
            "base_url": upstream, "auth": {"pipeline": [{
                "type": "script_handshake", "script": script, "credential": "packaged-smoke",
            }]},
        }}), text=True)
        request("handshake-scripts/approve", "POST", {"path": script})
        states = request("handshake-scripts")["scripts"]
        if not any(s["path"] == "data/" + script and s["approved"] for s in states):
            raise RuntimeError("handshake approval did not land")
        if request("proxy/packaged-smoke/handshake", text=True) != nonce:
            raise RuntimeError("proxy did not return the authenticated upstream response")
        with lock:
            if "/handshake" not in received:
                raise RuntimeError("fake upstream did not authenticate the handshake request")
        print("PASS: approved packaged script_handshake authenticated through proxy", flush=True)

        # Scheduled scripts use the same build_script_env_vars credential path
        # as run_python. No LLM or developer-host Python can satisfy this case.
        script = "triggers/packaged-smoke/scripts/run.py"
        source = credential_helper() + f'''
import urllib.request
client = urllib.request.build_opener(urllib.request.ProxyHandler({{}}))
req = urllib.request.Request({upstream + "/python"!r}, headers={{"Authorization": "Bearer " + credential()}})
with client.open(req, timeout=10) as response:
    if response.read().decode() != {nonce!r}:
        raise RuntimeError("unexpected upstream response")
print("packaged credential use succeeded")
'''
        request("data/" + script, "PUT", source, text=True)
        request("triggers", "POST", {
            "name": "Packaged credential smoke", "slug": "packaged-smoke",
            "run": {"type": "script", "path": script},
            "cron_expressions": ["0 0 3 1 1 *"],
        })
        trigger = next(t for t in request("triggers")["triggers"] if t["slug"] == "packaged-smoke")
        if trigger.get("last_run") is not None:
            raise RuntimeError("script ran before the explicit smoke invocation")
        request("triggers/run?id=" + trigger["id"], "POST")
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            result = next(t for t in request("triggers")["triggers"] if t["id"] == trigger["id"])
            if result.get("last_run") is not None:
                if result.get("last_run_status") != "ok":
                    raise RuntimeError("packaged Python script failed: inspect the disposable service log; do not fix execution in this branch")
                with lock:
                    if "/python" not in received:
                        raise RuntimeError("script finished without authenticated credential use")
                print("PASS: packaged Python script received and used its credential", flush=True)
                break
            time.sleep(0.25)
        else:
            raise RuntimeError("packaged Python script did not finish within 60 seconds")
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=5)


if __name__ == "__main__":
    try:
        run(*sys.argv[1:])
    except Exception as error:
        print(f"FAIL: packaged proxy/script smoke: {error}", file=sys.stderr)
        sys.exit(1)
