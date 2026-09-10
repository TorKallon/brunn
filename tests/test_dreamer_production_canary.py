from __future__ import annotations

import argparse
from copy import deepcopy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import unittest
from unittest.mock import Mock, patch


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
    def test_draft_receipt_requires_exact_typed_identity_and_canonical_candidate_hash(self):
        candidate = {"content": "A synthetic café observation.[^s1]", "kind": "summary",
                     "sources": [{"version": 2, "entry_ref": "entry:synthetic"}]}
        pointer = {"entry_ref": "entry:00000000-0000-4000-8000-000000000044", "version": 3,
                   "candidate_hash": hashlib.sha256(json.dumps(candidate, sort_keys=True,
                       ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()}
        ack = {"protocol": canary.DRAFT_PROTOCOL, "operation_id": "synthetic-operation",
               "recorded": True, "replayed": False, "retired": False, "pointer": pointer}
        response = {"draft_receipt": ack}
        self.assertEqual(canary.draft_receipt(response, "synthetic-operation", candidate,
                         replayed=False, retired=False), pointer)
        for replayed, retired in [(True, False), (False, True), (True, True)]:
            current = {"draft_receipt": {**ack, "replayed": replayed, "retired": retired}}
            self.assertEqual(canary.draft_receipt(current, "synthetic-operation", candidate,
                replayed=replayed, retired=retired, pointer=pointer), pointer)
        faults = [("missing", {}), ("null", {"draft_receipt": None}),
                  ("array", {"draft_receipt": [ack]})]
        for field, wrong in [("protocol", "unknown"), ("operation_id", "other-operation"),
                             ("recorded", 1), ("recorded", False), ("replayed", "false"),
                             ("replayed", True), ("retired", True), ("retired", 0),
                             ("pointer", []), ("pointer", None)]:
            faults.append((f"ack:{field}:{wrong}", {"draft_receipt": {**ack, field: wrong}}))
        for field, wrong in [("entry_ref", "entry:not-a-uuid"), ("entry_ref", "wrong:prefix"),
                             ("entry_ref", None), ("entry_ref", 17), ("version", True),
                             ("version", 0), ("version", "3"), ("candidate_hash", "0" * 64),
                             ("extra", "unexpected")]:
            faults.append((f"pointer:{field}:{wrong}",
                           {"draft_receipt": {**ack, "pointer": {**pointer, field: wrong}}}))
        for name, invalid in faults:
            with self.subTest(fault=name), self.assertRaisesRegex(canary.CanaryError, "draft custody receipt"):
                canary.draft_receipt(invalid, "synthetic-operation", candidate, replayed=False, retired=False)
        for wrong_pointer in [{**pointer, "version": 4},
                              {**pointer, "entry_ref": "entry:00000000-0000-4000-8000-000000000045"}]:
            with self.subTest(pointer=wrong_pointer), self.assertRaises(canary.CanaryError):
                canary.draft_receipt(response, "synthetic-operation", candidate,
                                     replayed=False, retired=False, pointer=wrong_pointer)
        with self.assertRaises(canary.CanaryError):
            canary.draft_receipt(response, "synthetic-operation", {**candidate, "content": "Changed content"},
                                 replayed=False, retired=False)

    def test_expected_http_error_retains_only_bounded_public_code(self):
        cases = [
            (json.dumps({"error": {"code": "research_refresh_required", "message": "PRIVATE_MESSAGE",
                                   "details": {"token": "PRIVATE_TOKEN"}}}),
             {"http_status": 400, "error": {"code": "research_refresh_required"}}),
            ("{malformed", {"http_status": 400}),
            ("null", {"http_status": 400}),
            ('{"error":[]}', {"http_status": 400}),
            ('{"error":{"code":["research_refresh_required"]}}', {"http_status": 400}),
            (json.dumps({"error": {"code": "x" * 97}}), {"http_status": 400}),
            (json.dumps({"error": {"code": "PRIVATE\nTOKEN"}}), {"http_status": 400}),
        ]
        for body, expected in cases:
            with self.subTest(body=body):
                calls = []
                client = canary.Client("https://brunn.ai/api", "SYNTHETIC_TOKEN", calls, "fixture")
                error = canary.urllib.error.HTTPError("https://brunn.ai/api/v1/workspace/dreamer/research-progress",
                    400, "synthetic rejection", None, io.BytesIO(body.encode()))
                with patch.object(client.opener, "open", side_effect=error):
                    result = client.request("POST", "/v1/workspace/dreamer/research-progress", {}, expected=(400,))
                self.assertEqual(result, expected)
                self.assertEqual(calls[0]["status"], 400)
                for forbidden in ("PRIVATE_MESSAGE", "PRIVATE_TOKEN", "SYNTHETIC_TOKEN"):
                    self.assertNotIn(forbidden, json.dumps({"result": result, "calls": calls}))

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

    def test_subject_cycle_requires_current_replay_full_dependencies_and_stale_fallback(self):
        for fault, error in ((None, None), ("protocol", "research_protocol 1"),
                             ("raw_input_missing", "four synthetic raw inputs"),
                             ("raw_input_extra", "four synthetic raw inputs"),
                             ("raw_input_wrong_version", "four synthetic raw inputs"),
                             ("source_origin_protocol", "source-origin follow-up protocol"),
                             ("draft_protocol", "draft custody protocol"),
                             ("draft_ack_missing", "draft custody receipt"),
                             ("draft_ack_hash", "draft custody receipt"),
                             ("draft_state_progress", "draft custody changed"),
                             ("draft_job_progress", "draft custody changed"),
                             ("draft_input_progress", "draft custody changed"),
                             ("draft_processed_progress", "draft custody changed"),
                             ("draft_raw_mutation", "draft custody changed"),
                             ("draft_replay_ack", "draft custody receipt"),
                             ("draft_replay_progress", "draft replay changed"),
                             ("draft_replay_inputs", "draft replay changed"),
                             ("draft_replay_retirement", "draft replay changed"),
                             ("draft_retirement_ack", "draft custody receipt"),
                             ("draft_not_retired", "did not retire"),
                             ("draft_retirement_missing_projection", "did not retire"),
                             ("discovery_audit", "current bounded discovery audit"),
                             ("excluded_index", "excluded search fixture was not lexically indexed"),
                             ("excluded_admitted", "ineligible search fixture entered"),
                             ("excluded_counted", "counted the indexed ineligible fixture"),
                             ("audit_target_refresh", "target-only discovery changed"),
                             ("audit_source_change", "changed evidence retained"),
                             ("checkpoint_eof", "exact end-of-document selectors"),
                             ("checkpoint_excerpt", "copied source excerpts"),
                             ("publication_eof", "publication accepted an out-of-range"),
                             ("link_receipt", "imported link resolution receipt"),
                             ("refresh_signal", "typed refresh-required rejection"),
                             ("additive_progress", "additive discovery lost checked progress"),
                             ("unreviewed_source", "marked new sources reviewed"),
                             ("changed_progress", "changed dependency did not invalidate"),
                             ("missing_revalidation", "lost or misidentified historical revalidation"),
                             ("wrong_revalidation_origin", "lost or misidentified historical revalidation"),
                             ("revalidation_not_cleared", "retained obsolete historical context"),
                             ("revalidation_replay", "resurrected historical research context"),
                             ("replay", "rewound"), ("manifest", "uncited research dependency"),
                             ("generic_heading", "unrelated section heading invalidated"),
                             ("generated_briefing_create", "generated briefing edition invalidated"),
                             ("generated_briefing_update", "generated briefing edition invalidated"),
                             ("generated_briefing_raw", "generated briefing exact read was changed"),
                             ("freshness", "did not invalidate"), ("exact", "mutated an exact source")):
            with self.subTest(fault=fault):
                canonical = {"entry_ref": "entry:canonical", "version": 1,
                             "path": "sources/Projects/Canary Aster/Canary Aster.md"}
                trail = {"entry_ref": "entry:trail", "version": 1, "path": "sources/CanaryResearch/Trail.md"}
                primary = {"entry_ref": "entry:primary", "version": 1, "path": "sources/CanaryResearch/Outcome.md"}
                changed = {**primary, "version": 2}
                excluded = {"entry_ref": "entry:excluded", "version": 1,
                            "path": "sources/CanaryResearch/Excluded Aster.md"}
                headers = [canonical, trail, primary]
                raw_inputs = [*headers, excluded]
                admitted = {"admitted": True, "mode": "full", "research_protocol": 0 if fault == "protocol" else 1,
                            "attempt_id": "fixture-attempt", "fence": "fixture-fence", "state_version": 1,
                            "inputs": raw_inputs, "frozen_generation": 3, "processed_generation": 0}
                if fault == "raw_input_missing":
                    admitted["inputs"] = headers
                elif fault == "raw_input_extra":
                    admitted["inputs"] = [*raw_inputs, {"entry_ref": "entry:unexpected", "version": 1}]
                elif fault == "raw_input_wrong_version":
                    admitted["inputs"] = [*headers, {**excluded, "version": 2}]
                job = {"subject_ref": canonical["entry_ref"], "subject_path": canonical["path"],
                       "output_path": "derived/entities/canary-aster.md", "output_version": 0,
                       "follow_up_protocol": None if fault == "source_origin_protocol" else "dream.research.follow_up.v1",
                       "draft_protocol": None if fault == "draft_protocol" else canary.DRAFT_PROTOCOL,
                       "unaccepted_draft": None,
                       "discovery_audit": {"validity": "legacy_or_unknown", "last_search": None},
                       "notes": "", "reviewed_sources": [], "coverage": {"unresolved_targets": []}}
                initial_selectors = [{"entry_ref": canonical["entry_ref"], "version": 1,
                                      "start_line": 3, "end_line": 203, "path": canonical["path"]}]
                admissions = [{**admitted, "state_version": 2 if index == 0 else index + 3,
                               "research": {**job, "version": 1 if index == 0 else index + 2, "sources": sources,
                                            "snapshot_generation": 4 if index == 3 else 3}}
                              for index, sources in enumerate(([canonical], headers[:2], headers, [canonical, trail, changed]))]
                search_audit = {"validity": "current", "last_search": {
                    "schema": "dream.research.discovery.v1", "retrieval_policy": 2, "searched_generation": 3,
                    "queries": ["canary aster"], "groups": [
                        {"query_index": 0, "sort": sort, "returned": 1, "limit": 8,
                         "execution_status": "bounded", "output_limit_reached": False}
                        for sort in ["best_match", "last_modified"]]}}
                for item in admissions[1:3]:
                    item["research"]["discovery_audit"] = deepcopy(search_audit)
                admissions[3]["research"]["discovery_audit"] = {"validity": "outdated", "last_search": None}
                if fault == "discovery_audit":
                    admissions[1]["research"]["discovery_audit"]["last_search"]["queries"] = ["unrelated query"]
                elif fault == "excluded_admitted":
                    admissions[1]["research"]["sources"].append(excluded)
                elif fault == "excluded_counted":
                    admissions[1]["research"]["discovery_audit"]["last_search"]["groups"][0]["returned"] = 2
                elif fault == "audit_target_refresh":
                    admissions[2]["research"]["discovery_audit"]["last_search"]["searched_generation"] = 4
                elif fault == "audit_source_change":
                    admissions[3]["research"]["discovery_audit"] = deepcopy(search_audit)
                initial_progress = deepcopy(admissions[0])
                initial_progress["state_version"] = 3
                initial_progress["research"].update(version=2, notes="Canary Aster has a project trail.",
                                                    reviewed_sources=initial_selectors)
                if fault == "checkpoint_eof":
                    initial_progress["research"]["reviewed_sources"] = [
                        {**initial_selectors[0], "end_line": 204}]
                elif fault == "checkpoint_excerpt":
                    initial_progress["research"]["reviewed_sources"] = [
                        {**initial_selectors[0], "excerpt": "Unnecessary copied source body."}]
                for item in admissions[1:3]:
                    item["research"].update(notes=initial_progress["research"]["notes"],
                                            reviewed_sources=deepcopy(initial_selectors))
                if fault == "additive_progress":
                    admissions[1]["research"]["notes"] = ""
                elif fault == "link_receipt":
                    admissions[1]["research"]["coverage"] = {"unresolved_targets": ["CanaryResearch/Trail"]}
                elif fault == "unreviewed_source":
                    admissions[1]["research"]["reviewed_sources"].append(trail)
                elif fault == "changed_progress":
                    admissions[3]["research"].update(notes=initial_progress["research"]["notes"],
                                                    reviewed_sources=initial_selectors)
                historical_context = {
                    "status": "historical_revalidation_only",
                    "origin": {"entry_ref": "entry:notebook", "version": admissions[2]["research"]["version"],
                               "snapshot_generation": admissions[2]["research"]["snapshot_generation"]},
                    "notes": initial_progress["research"]["notes"],
                    "prior_reviewed_sources": [
                        {key: row[key] for key in ("entry_ref", "version", "start_line", "end_line")}
                        for row in initial_selectors],
                    "prior_pending_targets": ["CanaryResearch/Trail"],
                    "prior_progress": {"admitted_source_count": 3},
                }
                admissions[3]["research"]["revalidation_context"] = deepcopy(historical_context)
                if fault == "missing_revalidation":
                    admissions[3]["research"]["revalidation_context"] = None
                elif fault == "wrong_revalidation_origin":
                    admissions[3]["research"]["revalidation_context"]["origin"]["version"] = 1
                checkpoint = deepcopy(admissions[-1])
                checkpoint["state_version"] = 7
                checkpoint["research"].update(version=6,
                    notes="The current outcome is complete; one equipment detail remains unresolved.",
                    reviewed_sources=[canonical, changed],
                    revalidation_context=historical_context if fault == "revalidation_not_cleared" else None)
                replay = deepcopy(checkpoint)
                if fault == "replay":
                    replay["research"]["version"] = 1
                elif fault == "revalidation_replay":
                    replay["research"]["revalidation_context"] = historical_context
                runner = Mock()
                before_custody = [admitted, {"data": admissions[0]}, {"data": initial_progress},
                    *({"data": item} for item in admissions[1:3]),
                    {"http_status": 400, "error": {"code": "invalid_request" if fault == "refresh_signal" else "research_refresh_required"}},
                    {"data": admissions[3]}, {"data": checkpoint}, {"no_op": True, "data": replay},
                    {"http_status": 200 if fault == "publication_eof" else 400}]
                retained_draft, custody_calls = None, []

                def draft_ack(body, ptr, replayed, retired):
                    return {"protocol": canary.DRAFT_PROTOCOL, "operation_id": body["operation_id"],
                            "recorded": True, "replayed": replayed, "retired": retired,
                            "pointer": deepcopy(ptr)}

                def runner_response(method, path, body=None, **kwargs):
                    nonlocal retained_draft
                    if before_custody:
                        return deepcopy(before_custody.pop(0))
                    self.assertEqual(method, "POST")
                    if path.endswith("/research-progress"):
                        self.assertEqual(body["draft_protocol"], canary.DRAFT_PROTOCOL)
                        self.assertEqual(body["processed_inputs"], [])
                        self.assertEqual(set(body), {"attempt_id", "fence", "expected_state_version",
                            "operation_id", "subject_ref", "research_version", "draft_protocol",
                            "draft_candidate", "processed_inputs", "findings"})
                        custody_calls.append(deepcopy(body))
                        is_replay = len(custody_calls) == 2
                        if is_replay:
                            self.assertEqual(body, custody_calls[0])
                        candidate = body["draft_candidate"]
                        ptr = {"entry_ref": "entry:00000000-0000-4000-8000-000000000044", "version": 1,
                               "candidate_hash": hashlib.sha256(json.dumps(candidate, sort_keys=True,
                                   ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()}
                        retained_draft = {"status": "unaccepted_revalidation_only", "pointer": ptr,
                                          "candidate": deepcopy(candidate)}
                        response = deepcopy(checkpoint)
                        response["research"]["unaccepted_draft"] = deepcopy(retained_draft)
                        response["draft_receipt"] = draft_ack(body, ptr, is_replay, False)
                        if not is_replay:
                            if fault == "draft_ack_missing":
                                response.pop("draft_receipt")
                            elif fault == "draft_ack_hash":
                                response["draft_receipt"]["pointer"]["candidate_hash"] = "0" * 64
                            elif fault == "draft_state_progress":
                                response["state_version"] += 1
                            elif fault == "draft_job_progress":
                                response["research"]["version"] += 1
                            elif fault == "draft_input_progress":
                                response["inputs"] = []
                            elif fault == "draft_processed_progress":
                                response["processed_generation"] += 1
                            elif fault == "draft_raw_mutation":
                                response["research"]["unaccepted_draft"]["candidate"]["content"] = "Changed draft"
                        elif fault == "draft_replay_ack":
                            response["draft_receipt"]["operation_id"] = "wrong-operation"
                        elif fault == "draft_replay_progress":
                            response["research"]["version"] += 1
                        elif fault == "draft_replay_inputs":
                            response["inputs"] = []
                        elif fault == "draft_replay_retirement":
                            response["research"]["unaccepted_draft"] = None
                        return {"data": response, **({"no_op": True} if is_replay else {})}
                    if path.endswith("/candidates"):
                        self.assertEqual(body["draft_protocol"], canary.DRAFT_PROTOCOL)
                        self.assertEqual(body["draft_pointer"], retained_draft["pointer"])
                        self.assertEqual(body["candidates"], [retained_draft["candidate"]])
                        response = {**deepcopy(checkpoint), "accepted_candidate_ids": ["fixture-item"],
                                    "state_version": 8,
                                    "draft_receipt": draft_ack(body, body["draft_pointer"], False, True)}
                        response["research"]["unaccepted_draft"] = None
                        if fault == "draft_retirement_ack":
                            response["draft_receipt"]["retired"] = False
                        elif fault == "draft_not_retired":
                            response["research"]["unaccepted_draft"] = deepcopy(retained_draft)
                        elif fault == "draft_retirement_missing_projection":
                            response.pop("research")
                        return response
                    self.assertTrue(path.endswith("/finish"), path)
                    return {"latest_receipt": {"status": "partial", "mode": "full"}}

                runner.request.side_effect = runner_response
                item = {"id": "fixture-item", "reviewable": True, "stale": False, "run_entry_ref": "entry:run",
                        "run_version": 1, "candidate_hash": "fixture-hash", "candidate": {"target_path": job["output_path"]}}
                owner = Mock()
                owner.request.side_effect = [{"items": [item], "decision_version": 7}, {"application_status": "applied"}]
                reader, report, documents, writes = Mock(), {}, {}, []
                briefing_path = "Briefings/2026/Canary edition.md"
                briefing = {"entry_ref": "entry:briefing", "path": briefing_path}

                def fixture_write(client, path, content, version, *, metadata=None):
                    self.assertIs(client, owner)
                    self.assertNotEqual(path, "dreams/CONTROL.md")
                    if path == briefing_path:
                        self.assertEqual(metadata, {"kind": "briefing_edition"} if version == 0 else
                                         {"kind": "briefing_edition", "briefing": {"schema": "briefing.v1"}})
                    if path == excluded["path"]:
                        self.assertEqual(metadata, {"evaluation_output": True})
                    writes.append(path)
                    documents[path, version + 1] = content
                    source = next((row for row in [*headers, briefing, excluded] if row["path"] == path),
                                  {"entry_ref": "entry:unrelated" if path.endswith("/Unrelated.md") else "entry:update", "path": path})
                    return {**source, "version": version + 1, "search_status":
                            "pending" if fault == "excluded_index" and source == excluded else
                            "lexical_ready_semantic_queued"}

                def fixture_read(client, **request):
                    self.assertIs(client, reader)
                    relevant_change = ("sources/Elsewhere/ResearchUpdate.md", 1) in documents
                    if request["view"] == "full":
                        source = next(row for row in [*headers, briefing] if row["entry_ref"] == request["ref"])
                        text = documents[source["path"], request["version"]]
                        if (fault == "exact" and relevant_change) or (fault == "generated_briefing_raw" and source == briefing):
                            text = "corrupted"
                        return {"reference": source["entry_ref"], "version": request["version"], "text": text}
                    if relevant_change:
                        return {"reference": canonical["entry_ref"], "text": documents[canonical["path"], 1],
                                "representation": "derived_summary" if fault == "freshness" else "current_source_fallback",
                                "freshness": {"reason": "subject_scope_changed"}}
                    if fault == "generic_heading" and ("sources/Elsewhere/Unrelated.md", 2) in documents:
                        return {"reference": canonical["entry_ref"], "text": documents[canonical["path"], 1],
                                "representation": "current_source_fallback", "freshness": {"reason": "subject_scope_changed"}}
                    if ((fault == "generated_briefing_create" and (briefing_path, 1) in documents)
                            or (fault == "generated_briefing_update" and (briefing_path, 2) in documents)):
                        return {"reference": canonical["entry_ref"], "text": documents[canonical["path"], 1],
                                "representation": "current_source_fallback", "freshness": {"reason": "subject_scope_changed"}}
                    candidate = runner.request.call_args_list[-2].args[2]["candidates"][0]
                    return {"reference": "entry:summary", "path": job["output_path"], "version": 1,
                            "representation": "derived_summary", "freshness": {"status": "fresh"}, "text": candidate["content"],
                            "metadata": {"dreamer_summary": {"subject_ref": canonical["entry_ref"], "sources": candidate["sources"],
                                "subject_scope": {"dependencies": [canonical, changed] if fault == "manifest" else [canonical, trail, changed]}}}}

                with patch.object(canary, "write", side_effect=fixture_write), \
                        patch.object(canary, "read_one", side_effect=fixture_read), \
                        patch.object(canary, "measure_subject_reads", return_value={"synthetic_measurement": True}):
                    if error:
                        with self.assertRaisesRegex(canary.CanaryError, error):
                            canary.run_subject_cycle(owner, runner, reader, report)
                        self.assertNotIn("subject_cycle", report)
                    else:
                        canary.run_subject_cycle(owner, runner, reader, report)
                        self.assertEqual(report["subject_cycle"]["research_dependencies"], 3)
                        self.assertEqual(report["subject_cycle"]["original_inputs_retained"], 4)
                        self.assertEqual(report["subject_cycle"]["claim_sources"], 2)
                        self.assertEqual(report["subject_cycle"]["uncited_cross_directory_invalidation"], "passed")
                        self.assertTrue(report["subject_cycle"]["saved_progress_survives_additive_discovery"])
                        self.assertTrue(report["subject_cycle"]["checkpoint_eof_normalized"])
                        self.assertTrue(report["subject_cycle"]["checkpoint_selectors_compact"])
                        self.assertTrue(report["subject_cycle"]["publication_eof_remains_strict"])
                        self.assertTrue(report["subject_cycle"]["imported_wiki_links_resolved"])
                        self.assertTrue(report["subject_cycle"]["subject_header_search_exercised"])
                        self.assertTrue(report["subject_cycle"]["static_ineligible_search_result_verified"])
                        self.assertTrue(report["subject_cycle"]["changed_scope_refresh_signal"])
                        self.assertTrue(report["subject_cycle"]["unaccepted_draft_custody_replay_and_retirement"])
                        self.assertTrue(report["subject_cycle"]["changed_dependency_invalidates_progress"])
                        self.assertTrue(report["subject_cycle"]["historical_revalidation_context_retained"])
                        self.assertTrue(report["subject_cycle"]["fresh_replacement_and_replay_clear_revalidation"])
                        self.assertTrue(report["subject_cycle"]["generic_section_heading_is_not_an_identity"])
                        self.assertTrue(report["subject_cycle"]["generated_briefing_editions_do_not_invalidate"])
                        self.assertTrue(report["subject_cycle"]["generated_briefing_exact_reads_preserved"])
                        self.assertTrue(documents[canonical["path"], 1].startswith("## Purpose\n"))
                        calls = runner.request.call_args_list
                        self.assertEqual(len(calls), 14, "custody and replay add exactly two runner requests")
                        self.assertEqual(calls[0].args[2]["requested_subject_refs"], [canonical["entry_ref"]])
                        self.assertEqual(calls[3].args[2]["targets"], ["CanaryResearch/Trail"])
                        self.assertEqual(calls[3].args[2]["queries"], ["Canary Aster"])
                        self.assertEqual(calls[4].args[2]["targets"], ["CanaryResearch/Outcome.md"])
                        self.assertEqual(calls[3].args[2], calls[8].args[2])
                        self.assertNotEqual(calls[3].args[2]["operation_id"], calls[4].args[2]["operation_id"])
                        self.assertEqual(calls[2].args[1], "/v1/workspace/dreamer/research-progress")
                        self.assertEqual(calls[5].args[1], "/v1/workspace/dreamer/research-progress")
                        self.assertEqual(calls[5].kwargs["expected"], (400,))
                        self.assertEqual(calls[7].args[1], "/v1/workspace/dreamer/research-progress")
                        self.assertEqual(calls[2].args[2]["reviewed_sources"][0]["end_line"], 204)
                        rejected, accepted = calls[-5], calls[-2]
                        self.assertEqual(rejected.kwargs["expected"], (400,))
                        self.assertEqual(rejected.args[2]["candidates"][0]["sources"][1]["end_line"], 4)
                        self.assertEqual(accepted.args[2]["candidates"][0]["sources"][1]["end_line"], 3)
                        self.assertNotEqual(rejected.args[2]["operation_id"], accepted.args[2]["operation_id"])
                        self.assertEqual(len(custody_calls), 2)
                        self.assertTrue(all(call.args[1] == "/v1/workspace/dreamer/research-progress"
                                            for call in calls[-4:-2]))
                        self.assertNotEqual(custody_calls[0]["operation_id"], accepted.args[2]["operation_id"])
                        self.assertEqual(accepted.args[2]["research_version"], 6)
                        self.assertEqual(accepted.args[2]["processed_inputs"], [])
                        self.assertNotIn("subject_scope", accepted.args[2]["candidates"][0])
                        self.assertTrue(all("notifications" not in call.args[1] for call in calls))

    def test_subject_read_measurement_is_warmed_alternating_and_content_free(self):
        canonical, summary = {"entry_ref": "entry:canonical", "version": 1}, {"reference": "entry:summary"}
        reader = Mock(calls=[])

        def read(client, **request):
            self.assertIs(client, reader)
            current = request["view"] == "current_state"
            reader.calls.append({"path": "/v1/workspace/read", "status": 200,
                                 "elapsed_ms": 20 if current else 10, "response_bytes": 100 if current else 1000,
                                 "private": "DO_NOT_REPORT"})
            return {"reference": summary["reference"] if current else canonical["entry_ref"],
                    "representation": "derived_summary" if current else "source", "version": 1,
                    "freshness": {"status": "fresh"}, "text": "DO_NOT_REPORT"}

        with patch.object(canary, "read_one", side_effect=read) as reads:
            result = canary.measure_subject_reads(reader, canonical, summary)
        self.assertEqual(reads.call_count, 12)
        for name in result:
            self.assertEqual(len(result[name]["samples"]), 5)
        self.assertEqual(result["canonical_current_state"]["median_ms"], 20)
        self.assertEqual(result["exact_raw_source"]["median_response_bytes"], 1000)
        self.assertNotIn("DO_NOT_REPORT", json.dumps(result))
        self.assertEqual([call.kwargs["view"] for call in reads.call_args_list], ["current_state", "full"] * 6)
        # A slower current_state still passes: this is measurement, not a speed gate.

    def test_supplemental_failure_after_both_legacy_cycles_still_cleans_same_fixture(self):
        read_fd, write_fd = os.pipe()
        os.write(write_fd, json.dumps({"user": identity()["user"], "credential": {"token": "FIXTURE_SECRET"}}).encode())
        os.close(write_fd)
        args = argparse.Namespace(expected_revision="new", fixture_fd=read_fd, admin_base=None,
                                  cleanup_timeout=1, summary_preference="enabled")
        report = {"preflight": {"build_revision": "new", "review_http_status": 200,
                                "feature_flags": {"dreamer_summary_reads_enabled": True}}, "calls": []}
        client = StubClient({"/v1/me": identity(), "/v1/workspace/dashboard": {"storage": {"text": {"count": 0}}},
            "/v1/workspace/notifications?limit=1": {"items": []},
            "/v1/credentials": {"capabilities": list(canary.RUNNER_CAPS), "token": "FIXTURE_SECRET"},
            "/v1/workspace/write": {}, "/v1/workspace/notifications": {}, "/v1/workspace/read": {}})
        first = "The synthetic fixture valve is amber; its state is uncertain."
        second = "The synthetic fixture valve is violet; its state remains uncertain."
        fallback = {"representation": "current_source_fallback", "text": second}
        owner = type("Owner", (), {"base": "https://brunn.ai/api"})()
        try:
            with patch.object(canary, "Client", return_value=client), \
                    patch.object(canary, "write", side_effect=[{}, {"entry_ref": "entry:source", "version": 1},
                                                               {"entry_ref": "entry:source", "version": 2}]), \
                    patch.object(canary, "read_one", side_effect=[fallback, fallback, {"text": first}]), \
                    patch.object(canary, "run_cycle", return_value={"reference": "entry:summary", "version": 1}) as legacy, \
                    patch.object(canary, "run_subject_cycle", side_effect=canary.CanaryError("supplemental failed")) as subject, \
                    patch.object(canary, "cleanup", return_value={"canonical_purge_verified": True}) as cleanup:
                with self.assertRaisesRegex(canary.CanaryError, "supplemental failed"):
                    canary.execute(owner, OWNER_ID, args, report, lambda: None)
                self.assertEqual(legacy.call_count, 2)
                subject.assert_called_once_with(client, client, client, report)
                cleanup.assert_called_once_with(client, FIXTURE, OWNER_ID, 1)
            self.assertEqual(report["source_edit_fallback"], "passed")
            self.assertNotIn("FIXTURE_SECRET", json.dumps(report))
        finally:
            os.close(read_fd)


if __name__ == "__main__":
    unittest.main()
