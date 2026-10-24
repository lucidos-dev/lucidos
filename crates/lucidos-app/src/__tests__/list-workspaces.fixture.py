"""Gateway inventory is authoritative; stale port assignments prove no health."""
import http.server
import json
import os
from pathlib import Path
import subprocess
import ssl
import tempfile
import threading
import unittest


class WorkspaceListTests(unittest.TestCase):
    def setUp(self):
        repo_root = Path(__file__).resolve().parents[4]
        scratch = repo_root / ".lucidos"
        scratch.mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.token = self.root / "token"
        self.token.write_text("fixture-token")
        stale = self.root / "workspaces" / "stale-directory" / ".lucidos"
        stale.mkdir(parents=True)
        (stale / "ports").write_text("API_PORT=5173\n")
        self.truncate = False
        self.status = 200
        self.payload = {"workspaces": [
            {"name": "Registered elsewhere", "health": "healthy"},
            {"name": "Stopped with old ports", "health": "unhealthy", "last_error": "not started"},
        ]}
        self.requests = []
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                owner.requests.append((self.path, self.headers.get("x-lucidos-local-token")))
                self.send_response(owner.status)
                if owner.status == 302:
                    self.send_header("Location", "/unexpected-redirect")
                body = json.dumps(owner.payload).encode()
                if owner.truncate:
                    self.send_header("Content-Length", str(len(body) + 10))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        self.handler = Handler
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.worker = threading.Thread(target=self.server.serve_forever)
        self.worker.start()
        self.addCleanup(self.stop_server)
        self.origin = f"http://127.0.0.1:{self.server.server_port}"
        self.script = Path(os.environ.get("WORKSPACE_LIST_HELPER", repo_root / "system-knowhow/scripts/list-workspaces.sh"))

    def stop_server(self):
        self.server.shutdown()
        self.server.server_close()
        self.worker.join()

    def run_script(self, origin=None, extra=()):
        env = dict(os.environ, HOME=str(self.root))
        return subprocess.run(["bash", str(self.script), origin or self.origin, str(self.token), *extra],
                              env=env, capture_output=True, text=True, timeout=15)

    def test_uses_gateway_inventory_and_health_instead_of_stale_ports(self):
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), [
            '"Registered elsewhere"\t"healthy"',
            '"Stopped with old ports"\t"unhealthy"',
        ])
        self.assertEqual(self.requests, [("/~/api/v1/control/workspaces", "fixture-token")])
        self.assertNotIn("fixture-token", result.stdout + result.stderr)

    def test_unreadable_health_does_not_print_partial_success(self):
        self.payload["workspaces"].append({"name": "Incomplete"})
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")

    def test_redirect_is_not_followed_with_the_token(self):
        self.status = 302
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(self.requests), 1)
        self.assertNotIn("fixture-token", result.stdout + result.stderr)

    def test_nonlocal_origin_is_rejected_before_sending_credentials(self):
        result = self.run_script("https://example.com")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.requests, [])

    def test_https_requires_trust_and_accepts_an_explicit_ca(self):
        cert, key = self.root / "cert.pem", self.root / "key.pem"
        config = self.root / "openssl.cnf"
        config.write_text("[req]\ndistinguished_name=dn\nx509_extensions=ext\n"
                          "[dn]\n[ext]\nsubjectAltName=IP:127.0.0.1\n"
                          "basicConstraints=critical,CA:TRUE\n")
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                        "-keyout", str(key), "-out", str(cert), "-days", "1",
                        "-subj", "/CN=localhost", "-config", str(config)],
                       check=True, capture_output=True, timeout=15)
        self.stop_server()
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), self.handler)
        self.server.socket = context.wrap_socket(self.server.socket, server_side=True)
        self.worker = threading.Thread(target=self.server.serve_forever)
        self.worker.start()
        self.origin = f"https://127.0.0.1:{self.server.server_port}"
        refused = self.run_script()
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(self.requests, [])
        trusted = self.run_script(extra=("--ca-file", str(cert)))
        self.assertEqual(trusted.returncode, 0, trusted.stderr)
        self.assertIn('"Registered elsewhere"', trusted.stdout)
        self.assertEqual(len(self.requests), 1)
        self.assertNotIn("fixture-token", trusted.stdout + trusted.stderr)

    def test_bad_port_has_a_clean_error(self):
        result = self.run_script("http://127.0.0.1:bad")
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("Traceback", result.stderr)
        self.assertIn("Cannot read gateway workspace health", result.stderr)

    def test_truncated_response_has_a_clean_error(self):
        self.truncate = True
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertNotIn("Traceback", result.stderr)
        self.assertIn("Cannot read gateway workspace health", result.stderr)

    def test_invalid_token_is_not_echoed(self):
        self.token.write_text("fixture-token\ninjected")
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("fixture-token", result.stdout + result.stderr)
        self.assertEqual(self.requests, [])


if __name__ == "__main__":
    unittest.main()
