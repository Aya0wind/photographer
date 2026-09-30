"""Offline protocol tests; no Apple credentials or network requests."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "notarize", Path(__file__).with_name("notarize-macos.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
ID = "01234567-89ab-4cde-8fab-0123456789ab"


class NotarizationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.dmg = self.root / "Photo Hub 1.2.3.dmg"
        self.dmg.write_bytes(b"test artifact")
        self.evidence = self.root / "evidence"
        self.env = {"APPLE_ID": "test@example.invalid",
                    "APPLE_PASSWORD": "test-secret", "APPLE_TEAM_ID": "ABCDE12345"}
        self.calls = []

    def runner(self, status="Accepted", code=0, staple_code=0, raw=None):
        def execute(args, **kwargs):
            self.calls.append(args)
            if args[2] == "submit":
                return subprocess.CompletedProcess(
                    args, code, raw if raw is not None else json.dumps({"id": ID, "status": status}), "")
            if args[1:3] == ["stapler", "staple"]:
                return subprocess.CompletedProcess(args, staple_code, "", "")
            return subprocess.CompletedProcess(args, 0, "", "")
        return execute

    def test_acceptance_precedes_stapling_and_records_identity(self):
        identity = module.notarize(self.dmg, self.evidence, self.env, self.runner())
        self.assertEqual(identity, ID)
        self.assertEqual([c[2] for c in self.calls], ["submit", "staple", "validate"])
        self.assertEqual((self.evidence / "submission-id.txt").read_text().strip(), ID)
        artifact = json.loads((self.evidence / "artifact.json").read_text())
        self.assertEqual(len(artifact["sha256_before_stapling"]), 64)

    def test_nonaccepted_states_never_staple_or_resubmit(self):
        for status in ("Invalid", "Rejected", "In Progress", None, "accepted"):
            with self.subTest(status=status):
                self.calls.clear()
                evidence = self.root / str(status)
                with self.assertRaises(RuntimeError):
                    module.notarize(self.dmg, evidence, self.env, self.runner(status))
                self.assertEqual([c[2] for c in self.calls], ["submit", "log"])

    def test_command_failure_even_with_accepted_text_does_not_staple(self):
        with self.assertRaises(RuntimeError):
            module.notarize(self.dmg, self.evidence, self.env, self.runner(code=1))
        self.assertEqual([c[2] for c in self.calls], ["submit", "log"])

    def test_missing_id_and_malformed_output_fail_closed(self):
        for index, raw in enumerate(('{"status":"Accepted"}', "broken", "[]")):
            with self.subTest(raw=raw):
                self.calls.clear()
                with self.assertRaises(RuntimeError):
                    module.notarize(self.dmg, self.root / str(index), self.env, self.runner(raw=raw))
                self.assertEqual([c[2] for c in self.calls], ["submit"])

    def test_staple_failure_prevents_validation(self):
        with self.assertRaises(RuntimeError):
            module.notarize(self.dmg, self.evidence, self.env, self.runner(staple_code=1))
        self.assertEqual([c[2] for c in self.calls], ["submit", "staple"])

    def test_previous_evidence_and_missing_credentials_prevent_submission(self):
        self.evidence.mkdir()
        with self.assertRaises(FileExistsError):
            module.notarize(self.dmg, self.evidence, self.env, self.runner())
        with self.assertRaises(ValueError):
            module.notarize(self.dmg, self.root / "new", {}, self.runner())
        self.assertEqual(self.calls, [])

    def test_failed_submission_evidence_redacts_credentials(self):
        def execute(args, **kwargs):
            self.calls.append(args)
            if args[2] == "submit":
                return subprocess.CompletedProcess(args, 1,
                    json.dumps({"id": ID, "status": "Invalid",
                                "message": self.env["APPLE_ID"]}),
                    f"Authentication failed: {self.env['APPLE_PASSWORD']}")
            Path(args[-1]).write_text(json.dumps({"message": self.env["APPLE_PASSWORD"]}))
            return subprocess.CompletedProcess(args, 0, "", self.env["APPLE_ID"])

        with self.assertRaises(RuntimeError):
            module.notarize(self.dmg, self.evidence, self.env, execute)
        for path in self.evidence.iterdir():
            content = path.read_text()
            self.assertNotIn(self.env["APPLE_ID"], content)
            self.assertNotIn(self.env["APPLE_PASSWORD"], content)


if __name__ == "__main__":
    unittest.main()
