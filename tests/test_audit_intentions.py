from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from eval.audit_intentions import (  # noqa: E402
    accepted_e08_runs,
    audit_runs,
    build_parser,
    load_manifest,
)


def intention_pointer(
    path="Intentions/Allowed.md",
    *,
    title_line="Review the pending obligation",
    due="2099-08-03",
    status_note="pending",
    matched_terms=None,
):
    return {
        "path": path,
        "title_line": title_line,
        "due": due,
        "status_note": status_note,
        "matched_terms": matched_terms or ["review"],
    }


def accepted_artifact(pending):
    rendered_chars = len(json.dumps(
        pending,
        separators=(",", ":"),
        ensure_ascii=False,
    ))
    return {
        "path": "/tmp/e08-flag.json",
        "sha256": "a" * 64,
        "suite": "prospective",
        "manifest": {
            "path": "/tmp/manifest.json",
            "sha256": "b" * 64,
            "benchmark_version": "e08-prospective-v0.1",
        },
        "oracle": {"case-a": ["Intentions/Allowed.md"]},
        "provenance": {
            "run_id": "e08-run1",
            "experiment_arm": "e08-flag",
            "paired_draw_id": "e08-prospective-draw1",
            "schema_sha256": "c" * 64,
            "harness_sha256": "d" * 64,
            "source_revision": "e" * 40,
            "build_revision": "e" * 40,
            "api_image_id": "sha256:" + "f" * 64,
            "api_image_revision": "e" * 40,
            "runtime_features": {
                "semantic_lane": False,
                "verbatim_spans": False,
                "supersession_demotion": False,
                "intention_ledger": True,
                "resume_deltas": False,
            },
            "service_retrieval_modes": ["exact", "lexical"],
            "reasoning_billing": {
                "route": "chatgpt_subscription",
                "api_fallback": "forbidden",
                "auth_status": "Logged in using ChatGPT",
            },
        },
        "run": {
            "records": [{
                "case_id": "case-a",
                "service_operations": [{
                    "operation": "open",
                    "at": "2026-07-28T12:00:00+00:00",
                    "pending_intentions": pending,
                    "pending_intention_chars": rendered_chars,
                }],
            }],
        },
    }


class AuditIntentionsTests(unittest.TestCase):
    def test_audit_uses_exact_saved_pointer_and_character_receipts(self):
        result = audit_runs([
            accepted_artifact([intention_pointer()]),
        ])
        self.assertTrue(result["pass"])
        self.assertEqual(result["opens"], 1)
        self.assertEqual(result["pointers"], 1)
        self.assertEqual(result["false_surfacing_rate"], 0.0)

    def test_done_pointer_fails_and_malformed_character_receipt_raises(self):
        result = audit_runs([
            accepted_artifact([intention_pointer(
                "Intentions/Completed renewal invoice.md",
                due="2026-07-18",
                status_note="overdue",
            )]),
        ])
        self.assertFalse(result["pass"])
        self.assertEqual(len(result["done_surfacings"]), 1)

        malformed = accepted_artifact([])
        malformed["run"]["records"][0]["service_operations"][0][
            "pending_intention_chars"
        ] = 99
        with self.assertRaisesRegex(ValueError, "malformed intention accounting"):
            audit_runs([malformed])

    def test_pointer_schema_cap_and_overdue_semantics_fail_closed(self):
        too_many = [
            intention_pointer(f"Intentions/Probe {index}.md")
            for index in range(6)
        ]
        with self.assertRaisesRegex(ValueError, "5-pointer"):
            audit_runs([accepted_artifact(too_many)])

        overlong = intention_pointer(title_line="x" * 81)
        with self.assertRaisesRegex(ValueError, "title_line"):
            audit_runs([accepted_artifact([overlong])])

        missing_terms = intention_pointer()
        missing_terms["matched_terms"] = []
        with self.assertRaisesRegex(ValueError, "matched_terms"):
            audit_runs([accepted_artifact([missing_terms])])

        wrong_overdue_status = intention_pointer(
            due="2026-07-20",
            status_note="pending",
        )
        with self.assertRaisesRegex(
            ValueError,
            "expected 'overdue'",
        ):
            audit_runs([accepted_artifact([wrong_overdue_status])])

        historical_observation = accepted_artifact([intention_pointer(
            due="2025-01-02",
            status_note="pending",
        )])
        historical_observation["run"]["records"][0]["service_operations"][0][
            "at"
        ] = "2025-01-01T23:30:00-08:00"
        self.assertTrue(audit_runs([historical_observation])["pass"])

        missing_due = intention_pointer()
        missing_due["due"] = None
        with self.assertRaisesRegex(ValueError, "invalid due date"):
            audit_runs([accepted_artifact([missing_due])])

        naive_timestamp = accepted_artifact([intention_pointer()])
        naive_timestamp["run"]["records"][0]["service_operations"][0][
            "at"
        ] = "2026-07-28T12:00:00"
        with self.assertRaisesRegex(ValueError, "include a timezone"):
            audit_runs([naive_timestamp])

        extra_field = intention_pointer()
        extra_field["text"] = "must remain pointer-only"
        with self.assertRaisesRegex(ValueError, "bounded D05 pointer schema"):
            audit_runs([accepted_artifact([extra_field])])

    def test_cli_and_manifest_selection_are_exact(self):
        args = build_parser().parse_args([
            "full-1.json",
            "full-2.json",
            "full-3.json",
            "prospective-1.json",
            "prospective-2.json",
            "prospective-3.json",
            "--full-manifest",
            "eval/recent_work_cases.json",
            "--prospective-manifest",
            "eval/e08_prospective_cases.json",
            "--expected-full-draw",
            "e08-full-draw1",
            "--expected-full-draw",
            "e08-full-draw2",
            "--expected-full-draw",
            "e08-full-draw3",
            "--expected-prospective-draw",
            "e08-prospective-draw1",
            "--expected-prospective-draw",
            "e08-prospective-draw2",
            "--expected-prospective-draw",
            "e08-prospective-draw3",
            "--out",
            "audit.json",
        ])
        self.assertEqual(len(args.inputs), 6)
        full = load_manifest(
            ROOT / "eval" / "recent_work_cases.json",
            expected_benchmark="recent-work-v0.3",
        )
        self.assertIn("recent-aether-gmail-actions", full["oracle"])
        self.assertNotIn(
            "coord-deadline-readiness",
            full["embedded_oracle"],
        )
        prospective = load_manifest(
            ROOT / "eval" / "e08_prospective_cases.json",
            expected_benchmark="e08-prospective-v0.1",
        )
        self.assertIn(
            "coord-deadline-readiness",
            prospective["embedded_oracle"],
        )
        with self.assertRaisesRegex(ValueError, "exactly six"):
            accepted_e08_runs(
                [],
                full_manifest_path=ROOT / "eval" / "recent_work_cases.json",
                prospective_manifest_path=(
                    ROOT / "eval" / "e08_prospective_cases.json"
                ),
                expected_full_draws=(
                    "e08-full-draw1",
                    "e08-full-draw2",
                    "e08-full-draw3",
                ),
                expected_prospective_draws=(
                    "e08-prospective-draw1",
                    "e08-prospective-draw2",
                    "e08-prospective-draw3",
                ),
            )


if __name__ == "__main__":
    unittest.main()
