from __future__ import annotations

import argparse
from copy import deepcopy
import importlib.util
import json
import os
from pathlib import Path
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("dreamer_canary", ROOT / "scripts/dreamer-production-canary.py")
canary = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(canary)

FIXTURE = {"user_id": "user:00000000-0000-4000-8000-000000000001",
           "external_ref": "brunn-dreamer-canary:00000000-0000-4000-8000-000000000002"}
OWNER_ID = "user:00000000-0000-4000-8000-000000000099"


def identity():
    return {"user": {"id": FIXTURE["user_id"], "external_ref": FIXTURE["external_ref"], "created_at": canary.stamp()},
            "read_only": False, "capabilities": ["credential:manage", "save", "status"]}


class StubClient:
    def __init__(self, responses):
        self.responses, self.requests = responses, []

    def request(self, method, path, body=None, **kwargs):
        self.requests.append((method, path, body))
        value = self.responses[path]
        return deepcopy(value)


class DreamerProductionCanaryTests(unittest.TestCase):
    def test_owner_keychain_origin_and_redirects_are_bounded(self):
        self.assertEqual(canary.safe_base("https://brunn.ai/api/"), "https://brunn.ai/api")
        self.assertEqual(canary.safe_base("http://127.0.0.1:18110", admin=True), "http://127.0.0.1:18110")
        for url in ("https://evil.example/api", "http://brunn.ai/api", "https://brunn.ai:8443/api",
                    "https://brunn.ai/api?token=secret", "https://user:secret@brunn.ai/api", "http://nyx:18110"):
            with self.subTest(url=url), self.assertRaises(canary.CanaryError):
                canary.safe_base(url, admin=True)
        with self.assertRaises(canary.CanaryError):
            canary.NoRedirect().redirect_request(None, None, None, None, None, None)

    def test_preflight_is_get_only_and_omits_personal_content_and_identity(self):
        client = StubClient({
            "/v1/me": {"user": {"id": OWNER_ID, "display_name": "PRIVATE_NAME"}, "capabilities": ["admin"], "read_only": False},
            "/v1/status": {"status": "ready", "build_revision": "old", "feature_flags": {}},
            "/ready": {"status": "ready", "dependencies": {"database": "ready"}},
            "/v1/workspace/dashboard?timezone=America%2FLos_Angeles": {"data": {"storage": {"text": {"count": 12}}, "access": [{"name": "PRIVATE_NAME"}]}},
            "/v1/workspace/notifications?limit=1": {"unread_count": 4, "items": [{"body": "PRIVATE_BODY"}]},
            "/v1/credentials": {"total": 3, "items": [{"name": "PRIVATE_NAME", "token": "NEVER_PRINT"}]},
            "/v1/dreamer/review": {"http_status": 404},
        })
        result, owner_id = canary.preflight(client)
        self.assertEqual(owner_id, OWNER_ID)
        self.assertEqual(result["storage"]["text"]["count"], 12)
        self.assertEqual(result["notification_unread_count"], 4)
        self.assertEqual(result["review_http_status"], 404)
        self.assertTrue(all(method == "GET" and body is None for method, _, body in client.requests))
        for forbidden in (OWNER_ID, "PRIVATE_NAME", "PRIVATE_BODY", "NEVER_PRINT"):
            self.assertNotIn(forbidden, json.dumps(result))

    def test_fixture_guard_rejects_real_owner_other_user_and_wrong_external_ref(self):
        for invalid in (
            {**identity(), "user": {**identity()["user"], "id": OWNER_ID}},
            {**identity(), "user": {**identity()["user"], "id": "user:other"}},
            {**identity(), "user": {**identity()["user"], "external_ref": "personal-owner"}},
        ):
            with self.subTest(identity=invalid), self.assertRaises(canary.CanaryError):
                canary.fixture_identity(invalid, FIXTURE, OWNER_ID)
        stale = identity()
        stale["user"]["created_at"] = "2020-01-01T00:00:00Z"
        with self.assertRaises(canary.CanaryError):
            canary.fixture_identity(stale, FIXTURE, OWNER_ID, fresh=True)

    def test_cleanup_checks_identity_before_any_destructive_request(self):
        client = StubClient({"/v1/me": {**identity(), "user": {**identity()["user"], "id": OWNER_ID}}})
        with self.assertRaises(canary.CanaryError):
            canary.cleanup(client, FIXTURE, OWNER_ID, 1)
        self.assertEqual(client.requests, [("GET", "/v1/me", None)])

    def test_cleanup_uses_account_lifecycle_and_reports_backup_retention(self):
        deletion = {"id": "account_deletion:fixture", "status": "awaiting_backup_expiry",
                    "records_total": 17, "records_completed": 17, "backup_expiry_due_at": "2099-01-01T00:00:00Z"}
        client = StubClient({"/v1/me": identity(), "/v1/account/deletion": deletion,
                             "/v1/account/deletions/account_deletion:fixture": deletion})
        result = canary.cleanup(client, FIXTURE, OWNER_ID, 1)
        self.assertTrue(result["canonical_purge_verified"])
        self.assertFalse(result["backup_erasure_verified"])
        mutations = [(method, path, body) for method, path, body in client.requests if method != "GET"]
        self.assertEqual(len(mutations), 1)
        self.assertEqual(mutations[0][1], "/v1/account/deletion")
        self.assertEqual(mutations[0][2]["confirmation"], "DELETE " + FIXTURE["external_ref"])

    def test_cleanup_resumes_status_only_credential_without_repeating_delete(self):
        status_only = identity()
        status_only.update(read_only=True, capabilities=["status"])
        deletion = {"id": "account_deletion:fixture", "status": "completed", "records_total": 17, "records_completed": 17}
        client = StubClient({"/v1/me": status_only, "/v1/account/deletion": deletion,
                             "/v1/account/deletions/account_deletion:fixture": deletion})
        self.assertTrue(canary.cleanup(client, FIXTURE, OWNER_ID, 1)["backup_erasure_verified"])
        self.assertTrue(all(method == "GET" for method, _, _ in client.requests))

    def test_revision_or_undeployed_review_blocks_before_fixture_provisioning(self):
        args = argparse.Namespace(expected_revision="new", fixture_fd=None, admin_base="http://127.0.0.1:18110")
        for preflight in ({"build_revision": "old", "review_http_status": 200},
                          {"build_revision": "new", "review_http_status": 404}):
            with self.subTest(preflight=preflight), patch.object(canary, "Client") as client:
                with self.assertRaises(canary.CanaryError):
                    canary.execute(None, OWNER_ID, args, {"preflight": preflight}, lambda: None)
                client.assert_not_called()

    def test_failure_after_fixture_authentication_still_requests_scoped_cleanup(self):
        read_fd, write_fd = os.pipe()
        provisioned = {"user": identity()["user"], "credential": {"token": "FIXTURE_SECRET_NEVER_PERSIST"}}
        os.write(write_fd, json.dumps(provisioned).encode())
        os.close(write_fd)
        args = argparse.Namespace(expected_revision="new", fixture_fd=read_fd, admin_base=None, cleanup_timeout=1,
                                  summary_preference="enabled")
        report = {"preflight": {"build_revision": "new", "review_http_status": 200,
                                "feature_flags": {"dreamer_summary_reads_enabled": True}}, "calls": []}
        fixture_client = StubClient({"/v1/me": identity(), "/v1/workspace/dashboard": {"data": {"storage": {"text": {"count": 1}}}}})
        owner = type("Owner", (), {"base": "https://brunn.ai/api"})()
        try:
            with patch.object(canary, "Client", return_value=fixture_client), patch.object(canary, "cleanup", return_value={"canonical_purge_verified": True}) as cleanup:
                with self.assertRaisesRegex(canary.CanaryError, "start empty"):
                    canary.execute(owner, OWNER_ID, args, report, lambda: None)
                cleanup.assert_called_once_with(fixture_client, FIXTURE, OWNER_ID, 1)
            self.assertNotIn("FIXTURE_SECRET_NEVER_PERSIST", json.dumps(report))
        finally:
            os.close(read_fd)

    def test_canonical_receipt_rejects_extra_fields_and_wrong_pins(self):
        text = (ROOT / "apps/api/tests/fixtures/dreamer/latest_receipt_v2.md").read_text()
        value = json.loads(text.split("```json\n", 1)[1].split("\n```", 1)[0])
        # Golden compatibility payload is report-only; the canary requires full.
        value["mode"] = "full"
        canary.receipt(value, value["receipt_ref"], value["receipt_version"])
        with self.assertRaises(canary.CanaryError):
            canary.receipt({**value, "new_lifecycle_field": "forbidden"}, value["receipt_ref"], value["receipt_version"])
        with self.assertRaises(canary.CanaryError):
            canary.receipt(value, value["receipt_ref"], value["receipt_version"] + 1)

    def test_phased_flag_rollout_retries_only_gets_and_exposes_milestone(self):
        client = StubClient({})
        replies = [canary.CanaryError("restart"), {"feature_flags": {"dreamer_summary_reads_enabled": False}},
                   {"feature_flags": {"dreamer_summary_reads_enabled": True}}]
        report = {}
        with patch.object(client, "request", side_effect=replies) as request, \
                patch.object(canary.time, "sleep") as sleep, patch("builtins.print") as output:
            canary.await_summary_preference(client, report, lambda: None, 60)
        self.assertEqual(report["phase"], "summary_preference_enabled")
        self.assertEqual(request.call_count, 3)
        self.assertTrue(all(call.args == ("GET", "/v1/status") for call in request.call_args_list))
        self.assertEqual(sleep.call_count, 2)
        self.assertIn("awaiting_summary_preference_enable", output.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
