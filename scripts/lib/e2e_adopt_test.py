"""Pure temporary-file and request-contract tests; never contacts a gateway."""

import fcntl
import json
from pathlib import Path
import tempfile
import unittest

from e2e_adopt import adopt


class AdoptionTests(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        self.addCleanup(self.root.cleanup)
        self.directory = Path(self.root.name) / "e2e-test"
        self.token = Path(self.root.name) / "local-token"
        self.token.write_text("test-local-secret")
        self.calls = []
        self.listed = []
        self.response = {"workspace": {"id": "e2e-test", "port": 5341},
                         "owner_token": "test-owner-secret"}

    def request(self, request):
        self.calls.append(request)
        self.assertEqual(request.get_header("X-lucidos-local-token"), "test-local-secret")
        return {"workspaces": self.listed} if request.method == "GET" else self.response

    def run_adopt(self):
        adopt("http://127.0.0.1:1", self.directory, "e2e-test", 5341,
              self.token, self.request)

    def test_new_slot_publishes_private_owner_and_provenance(self):
        self.run_adopt()
        body = json.loads(self.calls[1].data)
        self.assertEqual(body, {"dir": str(self.directory), "port": 5341,
                                "origin": "e2e", "ephemeral": True})
        path = self.directory.parent / ".cleanup-owners/e2e-test.json"
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(json.loads(path.read_text())["owner_token"], "test-owner-secret")
        self.calls.clear()
        with self.assertRaisesRegex(ValueError, "unarchived"):
            self.run_adopt()
        self.assertEqual(len(self.calls), 1)  # No second POST.

    def test_existing_registration_keeps_protection(self):
        self.listed = [{"id": "e2e-test", "port": 5341, "origin": "user"}]
        self.run_adopt()
        self.assertEqual(len(self.calls), 1)
        self.assertFalse((self.directory.parent / ".cleanup-owners/e2e-test.json").exists())

    def test_unknown_observation_or_invalid_adoption_never_publishes_owner(self):
        self.listed = [{"port": 5341}]
        with self.assertRaises(ValueError):
            self.run_adopt()
        self.assertEqual(len(self.calls), 1)
        self.listed = []
        self.response["workspace"]["id"] = "another-slot"
        with self.assertRaises(ValueError):
            self.run_adopt()
        self.assertFalse((self.directory.parent / ".cleanup-owners/e2e-test.json").exists())

    def test_missing_token_refuses_and_collision_does_not_replace_a_secret(self):
        del self.response["owner_token"]
        with self.assertRaises(ValueError):
            self.run_adopt()
        path = self.directory.parent / ".cleanup-owners/e2e-test.json"
        self.assertFalse(path.exists())
        path.write_text("previous-generation")
        self.calls.clear()
        with self.assertRaisesRegex(ValueError, "unarchived"):
            self.run_adopt()
        self.assertEqual(len(self.calls), 1)
        self.assertEqual(path.read_text(), "previous-generation")

    def test_stable_owner_lock_refuses_concurrent_adoption_before_network(self):
        folder = self.directory.parent / ".cleanup-owners"
        folder.mkdir()
        with (folder / "e2e-test.lock").open("a+b") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaises(BlockingIOError):
                self.run_adopt()
        self.assertEqual(self.calls, [])


if __name__ == "__main__":
    unittest.main()
