import hashlib
import json
import os
import re
import subprocess
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from unittest.mock import patch

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from agent_work_eval import (  # noqa: E402
    build_codex_command,
    candidate_matches,
    forbidden_is_asserted,
    grade_answer,
    normalize,
    parse_event_metrics,
    require_codex_subscription,
    render_fixed_context,
    render_prompt,
    resolve_codex_path,
    select_cases,
    subscription_reasoning_environment,
    load_native_provisioning_state,
    validate,
    write_native_provisioning_state,
)
from straylight_eval import BM25Index  # noqa: E402
from workspace_cli import corpus_hash, load_corpus, safe_compute  # noqa: E402


ROOT = Path(__file__).resolve().parents[1]


class AgentWorkEvalTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest = json.loads((ROOT / "eval" / "work_cases.json").read_text())
        cls.documents, cls.chunks = load_corpus(ROOT / cls.manifest["corpus_root"])

    def test_frozen_corpus_shape_and_hash(self):
        self.assertEqual(len(self.documents), 73)
        self.assertGreater(len(self.chunks), 1500)
        self.assertEqual(
            corpus_hash(self.documents),
            "b08ded20cdc2f1437da8cc0db5b217de0f84e89a33814995307fd81681be0bc2",
        )

    def test_codex_path_resolution_skips_a_broken_or_missing_install(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fallback = root / "current-codex"
            fallback.write_text("#!/bin/sh\nexit 0\n")
            fallback.chmod(0o755)
            self.assertEqual(
                resolve_codex_path([root / "missing-codex", fallback]),
                fallback,
            )

    def test_reasoning_environment_never_forwards_paid_api_credentials(self):
        env = subscription_reasoning_environment({
            "PATH": "/usr/bin:/bin",
            "OPENAI_API_KEY": "embedding-key",
            "OPENAI_BASE_URL": "https://paid-api.example.test/v1",
            "AZURE_OPENAI_API_KEY": "alternate-paid-key",
            "CODEX_API_KEY": "paid-reasoning-key",
            "CARRYSTATE_EVAL_DIRECT_OPENAI": "1",
        })
        self.assertEqual(env["PATH"], "/usr/bin:/bin")
        self.assertNotIn("OPENAI_API_KEY", env)
        self.assertNotIn("OPENAI_BASE_URL", env)
        self.assertNotIn("AZURE_OPENAI_API_KEY", env)
        self.assertNotIn("CODEX_API_KEY", env)
        self.assertNotIn("CARRYSTATE_EVAL_DIRECT_OPENAI", env)

    def test_reasoning_preflight_requires_chatgpt_authentication(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            chatgpt = root / "chatgpt-codex"
            chatgpt.write_text(
                "#!/bin/sh\necho 'Logged in using ChatGPT'\n",
                encoding="utf-8",
            )
            chatgpt.chmod(0o755)
            api_key = root / "api-key-codex"
            api_key.write_text(
                "#!/bin/sh\necho 'Logged in using an API key'\n",
                encoding="utf-8",
            )
            api_key.chmod(0o755)

            self.assertEqual(
                require_codex_subscription(chatgpt),
                {
                    "route": "chatgpt_subscription",
                    "api_fallback": "forbidden",
                },
            )
            with self.assertRaisesRegex(ValueError, "require Codex logged in"):
                require_codex_subscription(api_key)

    def test_reasoning_preflight_rejects_direct_api_override(self):
        with patch.dict(
            os.environ,
            {"CARRYSTATE_EVAL_DIRECT_OPENAI": "1"},
            clear=False,
        ):
            with self.assertRaisesRegex(ValueError, "Direct OpenAI reasoning is forbidden"):
                require_codex_subscription(Path("/unused/codex"))

    def test_native_provisioning_state_is_private_resumable_and_run_scoped(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / ".native-provisioning.json"
            cases = {
                "case-a": {
                    "authorization_scope": "eval:run-a/case-a",
                    "token": "one-time-secret",
                }
            }
            write_native_provisioning_state(path, "run-a", cases)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(load_native_provisioning_state(path, "run-a"), cases)
            with self.assertRaises(ValueError):
                load_native_provisioning_state(path, "run-b")

    def test_native_service_enables_network_without_relaxing_filesystem_sandbox(self):
        common = {
            "codex": Path("/tmp/codex"),
            "model": "gpt-5.6",
            "schema": Path("/tmp/schema.json"),
            "run_dir": Path("/tmp/run"),
        }
        native = build_codex_command(condition="service_api", **common)
        filesystem = build_codex_command(condition="filesystem", **common)

        self.assertIn("sandbox_workspace_write.network_access=true", native)
        self.assertNotIn("sandbox_workspace_write.network_access=true", filesystem)

        for command in (native, filesystem):
            sandbox_index = command.index("--sandbox")
            self.assertEqual(command[sandbox_index + 1], "workspace-write")

    def test_rupture_ops_suite_shape_hash_and_code_artifacts(self):
        manifest_path = ROOT / "eval" / "rupture_ops_cases.json"
        validated = validate(manifest_path, ROOT / "eval" / "work_answer_schema.json")
        self.assertEqual(validated["errors"], [])
        self.assertEqual(len(validated["manifest"]["cases"]), 12)
        self.assertEqual(
            sum(len(case["rubric"]) for case in validated["manifest"]["cases"]),
            48,
        )
        self.assertEqual(len(validated["documents"]), 65)
        self.assertEqual(len(validated["chunks"]), 587)
        self.assertEqual(
            validated["corpus_sha256"],
            "aa9c33e39777f0899dacecee61a94f5fd11ec1315f7a6e820a41bc217e1a9803",
        )
        self.assertEqual(
            validated["artifact_tree_sha256"],
            "aa434eb3ffd5f4b6b9c766000d34ec6c30fd14b5ec6f94a4ff625306396b7b50",
        )
        paths = {document.path for document in validated["documents"]}
        self.assertIn(
            "Projects/RuptureOps/Repository/ios/RuptureOps/Domain/RuptureCycle.swift",
            paths,
        )

        case = next(
            case for case in validated["manifest"]["cases"]
            if case["id"] == "ruptureops-interrupted-ios-continuation"
        )
        self.assertIn(
            './memory open --scope "RuptureOps"',
            render_prompt(case, "workspace"),
        )
        service_prompt = render_prompt(case, "service_api", "simple")
        self.assertIn("treat its initial evidence", service_prompt)
        self.assertIn("repeat `--path` or `--ref`", service_prompt)
        self.assertIn("complete entry", service_prompt)
        self.assertIn("intentionally has no compute or verify operation", service_prompt)
        self.assertNotIn("--neighbors", service_prompt)
        self.assertIn("exactly from authoritative evidence", service_prompt)
        self.assertIn("Build a facet checklist", service_prompt)
        self.assertIn("repeat a fact when it is needed in more than one slot", service_prompt)

    def test_retired_cases_are_excluded_by_default_but_remain_reproducible(self):
        active = select_cases(self.manifest, None, include_retired=False)
        all_cases = select_cases(self.manifest, None, include_retired=True)
        explicit = select_cases(
            self.manifest,
            ["straylight-trust-handoff"],
            include_retired=False,
        )

        self.assertEqual(len(all_cases), len(active) + 1)
        self.assertNotIn("straylight-trust-handoff", {case["id"] for case in active})
        self.assertEqual([case["id"] for case in explicit], ["straylight-trust-handoff"])

    def test_rupture_ops_rubrics_are_satisfiable_and_fixed_packs_are_fair(self):
        manifest = json.loads((ROOT / "eval" / "rupture_ops_cases.json").read_text())
        documents, chunks = load_corpus(ROOT / manifest["corpus_root"])
        paths = {document.path for document in documents}
        index = BM25Index(chunks)
        for case in manifest["cases"]:
            context = render_fixed_context(case, index, manifest)
            claims = []
            for rubric in case["rubric"]:
                self.assertTrue(
                    any(path in context for path in rubric["sources_any"]),
                    f"{case['id']}:{rubric['id']} has no expected source in its fixed pack",
                )
                claims.append({
                    "id": rubric["id"],
                    "value": " ".join(check["any"][0] for check in rubric["checks"]),
                    "source_paths": [rubric["sources_any"][0]],
                    "confidence": "high",
                })
            answer = {
                "answer": "Evidence-backed continuation.",
                "claims": claims,
                "checkpoint": {
                    "objective": "Continue the work safely.",
                    "current_state": ["Recovered evidence"],
                    "decisions": ["Preserve source boundaries"],
                    "open_questions": ["Refresh live state"],
                    "next_actions": ["Advance from the verified checkpoint"],
                    "artifacts": ["Source-bearing corpus artifacts"],
                },
            }
            self.assertTrue(grade_answer(case, answer, paths)["pass"], case["id"])

    def test_personal_coordination_suite_shape_scope_and_hashes(self):
        manifest_path = ROOT / "eval" / "personal_coordination_cases.json"
        validated = validate(manifest_path, ROOT / "eval" / "work_answer_schema.json")
        manifest = validated["manifest"]
        cases = manifest["cases"]

        self.assertEqual(validated["errors"], [])
        self.assertEqual(manifest["benchmark_version"], "personal-coordination-v0.1")
        self.assertEqual(manifest["grading_mode"], "concept_tokens_v1")
        self.assertEqual(
            manifest["conditions"],
            ["fixed_pack", "filesystem", "workspace"],
        )
        self.assertEqual(manifest["fixed_pack_chars"], 24000)
        self.assertEqual(manifest["fixed_pack_chunks"], 16)
        self.assertEqual(
            manifest["generalization"]["profile_types"],
            [
                "person", "organization", "group", "place", "event",
                "arrangement", "resource", "work-item", "artifact",
            ],
        )
        self.assertEqual(
            [case["id"] for case in cases],
            [
                "coord-person-resolution",
                "coord-identity-equivalence-reversal",
                "coord-person-dossier",
                "coord-role-relationship-provenance",
                "coord-canonical-contract-normalization",
                "coord-series-exceptions",
                "coord-schedule-supersession",
                "coord-participation-independent-state",
                "coord-deadline-readiness",
                "coord-handoff-logistics",
                "coord-arrangement-independent-state",
                "coord-vacation-game-continuity",
                "coord-weekly-brief-change-impact",
                "coord-read-only-capability-boundary",
                "coord-minor-export-policy",
            ],
        )
        self.assertTrue(all(case["scope"] == "Personal Coordination" for case in cases))
        self.assertTrue(all(case["workload"] == "Personal Coordination" for case in cases))
        self.assertTrue(all(set(case["claim_slots"]) == {"c1", "c2", "c3", "c4"} for case in cases))
        self.assertTrue(all(len(case["rubric"]) == 4 for case in cases))
        self.assertTrue(all(case["forbidden"] for case in cases))
        self.assertEqual(sum(len(case["rubric"]) for case in cases), 60)

        corpus_files = [path for path in validated["corpus_root"].rglob("*") if path.is_file()]
        self.assertEqual(len(corpus_files), 29)
        self.assertTrue(all(path.suffix in {".md", ".json", ".csv"} for path in corpus_files))
        self.assertEqual(len(validated["documents"]), 29)
        self.assertEqual(len(validated["chunks"]), 36)
        self.assertEqual(sum(len(document.text) for document in validated["documents"]), 29352)
        self.assertEqual(
            validated["corpus_sha256"],
            "1f2d62e8f27d2309bdb9353ff349277e038a58b4753c6f3199fd608e9c97ff18",
        )
        self.assertEqual(
            validated["artifact_tree_sha256"],
            "1f2d62e8f27d2309bdb9353ff349277e038a58b4753c6f3199fd608e9c97ff18",
        )
        self.assertEqual(
            hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
            "ad3a20cd4019d9ba574241b78924b51fcf69261d3cab802037ed9dff09abb7a2",
        )

    def test_personal_coordination_rubrics_are_satisfiable_and_fixed_packs_are_fair(self):
        manifest = json.loads(
            (ROOT / "eval" / "personal_coordination_cases.json").read_text()
        )
        documents, chunks = load_corpus(ROOT / manifest["corpus_root"])
        documents_by_path = {document.path: document for document in documents}
        paths = set(documents_by_path)
        index = BM25Index(chunks)

        for case in manifest["cases"]:
            context = render_fixed_context(case, index, manifest)
            self.assertLessEqual(len(context), manifest["fixed_pack_chars"] + 2000)
            claims = []
            for rubric in case["rubric"]:
                self.assertGreaterEqual(len(rubric["sources_any"]), 1)
                self.assertLessEqual(len(rubric["sources_any"]), 6)
                source_text = "\n".join(
                    documents_by_path[path].text for path in rubric["sources_any"]
                )
                for check in rubric["checks"]:
                    self.assertTrue(
                        any(normalize(candidate) in normalize(source_text) for candidate in check["any"]),
                        f"{case['id']}:{rubric['id']} has an unsatisfied source check {check}",
                    )
                self.assertTrue(
                    any(path in context for path in rubric["sources_any"]),
                    f"{case['id']}:{rubric['id']} has no expected source in its fixed pack",
                )
                claims.append({
                    "id": rubric["id"],
                    "value": " ".join(check["any"][0] for check in rubric["checks"]),
                    "source_paths": [rubric["sources_any"][0]],
                    "confidence": "high",
                })
            answer = {
                "answer": "Source-preserving personal coordination checkpoint.",
                "claims": claims,
                "checkpoint": {
                    "objective": "Continue coordination from frozen evidence.",
                    "current_state": ["Recovered source-bearing state"],
                    "decisions": ["Keep independent facts independent"],
                    "open_questions": ["Await new authoritative evidence"],
                    "next_actions": ["Advance the cited work item"],
                    "artifacts": ["Frozen corpus sources"],
                },
            }
            self.assertTrue(grade_answer(case, answer, paths)["pass"], case["id"])

    def test_recent_work_suite_shape_and_source_boundaries(self):
        manifest_path = ROOT / "eval" / "recent_work_cases.json"
        validated = validate(manifest_path, ROOT / "eval" / "work_answer_schema.json")
        manifest = validated["manifest"]

        self.assertEqual(validated["errors"], [])
        self.assertEqual(manifest["benchmark_version"], "recent-work-v0.2")
        self.assertEqual(len(manifest["cases"]), 12)
        self.assertEqual(
            sum(len(case["rubric"]) for case in manifest["cases"]),
            48,
        )
        self.assertEqual(len(validated["documents"]), 17)
        self.assertEqual(len(validated["chunks"]), 20)
        self.assertEqual(
            validated["corpus_sha256"],
            "1e4f1a1da7c87189d20d8ed01b228eee08058bb5c45afffca62d1dd40518528c",
        )
        self.assertTrue(
            {
                "recent-europe-source-authority",
                "recent-europe-calendar-dedup",
                "recent-europe-rail-resume",
                "recent-tracker-no-delta",
                "recent-aether-heartbeat-healthy",
                "recent-aether-gmail-actions",
                "recent-aether-morning-brief",
                "recent-current-over-history",
            }
            <= {case["id"] for case in manifest["cases"]}
        )

        frozen_text = manifest_path.read_text() + "\n" + "\n".join(
            document.text for document in validated["documents"]
        )
        self.assertNotIn("/Users/aether/", frozen_text)
        self.assertNotIn("@rourkem.com", frozen_text)
        self.assertNotRegex(frozen_text, r"\b[A-Z0-9]{6}\b")
        emails = re.findall(
            r"[A-Za-z0-9._%+-]+@([A-Za-z0-9.-]+\.[A-Za-z]{2,})",
            frozen_text,
        )
        self.assertEqual(set(emails), {"example.com"})

    def test_recent_work_rubrics_are_satisfiable_and_fixed_packs_are_fair(self):
        manifest = json.loads(
            (ROOT / "eval" / "recent_work_cases.json").read_text()
        )
        documents, chunks = load_corpus(ROOT / manifest["corpus_root"])
        documents_by_path = {document.path: document for document in documents}
        paths = set(documents_by_path)
        index = BM25Index(chunks)

        for case in manifest["cases"]:
            context = render_fixed_context(case, index, manifest)
            self.assertLessEqual(len(context), manifest["fixed_pack_chars"] + 2000)
            claims = []
            for rubric in case["rubric"]:
                source_text = "\n".join(
                    documents_by_path[path].text for path in rubric["sources_any"]
                )
                for check in rubric["checks"]:
                    self.assertTrue(
                        any(
                            candidate_matches(
                                candidate,
                                source_text,
                                manifest["grading_mode"],
                            )
                            for candidate in check["any"]
                        ),
                        f"{case['id']}:{rubric['id']} has an unsatisfied source check {check}",
                    )
                self.assertTrue(
                    any(path in context for path in rubric["sources_any"]),
                    f"{case['id']}:{rubric['id']} has no expected source in its fixed pack",
                )
                claims.append({
                    "id": rubric["id"],
                    "value": " ".join(check["any"][0] for check in rubric["checks"]),
                    "source_paths": [rubric["sources_any"][0]],
                    "confidence": "high",
                })
            answer = {
                "answer": "Source-backed recent-work continuation.",
                "claims": claims,
                "checkpoint": {
                    "objective": "Continue from current evidence.",
                    "current_state": ["Recovered the latest source-backed state"],
                    "decisions": ["Respect authority and action boundaries"],
                    "open_questions": ["Await new evidence where needed"],
                    "next_actions": ["Advance only the cited current work"],
                    "artifacts": ["Frozen recent-work corpus"],
                },
            }
            self.assertTrue(grade_answer(case, answer, paths)["pass"], case["id"])

    def test_personal_coordination_fixture_has_no_live_paths_or_contact_domains(self):
        manifest_path = ROOT / "eval" / "personal_coordination_cases.json"
        manifest = json.loads(manifest_path.read_text())
        documents, _ = load_corpus(ROOT / manifest["corpus_root"])
        frozen_text = manifest_path.read_text() + "\n" + "\n".join(
            document.text for document in documents
        )

        self.assertNotRegex(frozen_text, r"/(?:Users|home|Volumes)/")
        self.assertNotIn("obsidian", frozen_text.casefold())
        emails = re.findall(
            r"[A-Za-z0-9._%+-]+@([A-Za-z0-9.-]+\.[A-Za-z]{2,})",
            frozen_text,
        )
        self.assertGreater(len(emails), 0)
        self.assertEqual(set(emails), {"example.com"})

    def test_personal_coordination_canonical_contract_and_projection_receipt(self):
        corpus = ROOT / "eval" / "corpus-personal-coordination-v0.1"
        contract = json.loads(
            (corpus / "Contracts" / "canonical-contract-snapshot.json").read_text()
        )
        self.assertEqual(contract["schema"], "straylight-context-contract-fixture@v1")
        self.assertEqual(contract["object"]["object_id"], "object:person-p301")
        self.assertGreaterEqual(len(contract["object"]["type_profiles"]), 2)
        self.assertEqual(contract["qualified_relation"]["version"], "v2")
        self.assertEqual(contract["qualified_relation"]["previous_version"], "v1")
        self.assertEqual(len(contract["qualified_relation"]["endpoints"]), 2)
        recurrence = contract["recurring_event"]["recurrence"]
        self.assertEqual(contract["recurring_event"]["temporal_schema"], "temporal-spec@v1")
        self.assertEqual(recurrence["start"]["kind"], "local_datetime")
        self.assertEqual(recurrence["start"]["time_zone"], "America/Los_Angeles")
        self.assertEqual(
            {claim["predicate"] for claim in contract["state_assignments"]},
            {
                "state:arrangement.booking@v1",
                "state:arrangement.payment@v1",
                "state:arrangement.allocation@v1",
                "state:resource.availability@v1",
                "state:arrangement.use@v1",
            },
        )
        self.assertTrue(all(claim["claim_mode"] == "state_assignment" for claim in contract["state_assignments"]))
        claims_by_predicate = {
            claim["predicate"]: claim for claim in contract["state_assignments"]
        }
        self.assertEqual(
            claims_by_predicate["state:resource.availability@v1"]["about_refs"],
            [contract["resource"]["object_id"]],
        )
        self.assertTrue(
            all(
                claim["about_refs"] == [contract["arrangement"]["object_id"]]
                for predicate, claim in claims_by_predicate.items()
                if predicate != "state:resource.availability@v1"
            )
        )

        projection = json.loads(
            (corpus / "Policy" / "redacted-minor-projection.json").read_text()
        )
        for field in {
            "source_revision", "policy_version", "audience", "purpose",
            "path_basis", "included_paths", "withheld", "transforms", "generated_at",
            "audit_receipt",
        }:
            self.assertIn(field, projection)
        self.assertTrue(all(item.get("path") and item.get("reason") for item in projection["withheld"]))
        self.assertEqual(projection["path_basis"]["kind"], "source_revision")
        self.assertEqual(
            projection["path_basis"]["revision_ref"],
            projection["source_revision"],
        )
        self.assertTrue(all(path.startswith("/") for path in projection["included_paths"]))

    def test_concept_token_grading_accepts_paraphrase_and_preserves_negation(self):
        self.assertTrue(candidate_matches(
            "Each row retains its source ID",
            "The rebuilt view keeps provenance by retaining each row's source ID.",
            "concept_tokens_v1",
        ))
        self.assertTrue(candidate_matches(
            "schedule_authority: none",
            "The family copy has no schedule authority.",
            "concept_tokens_v1",
        ))
        self.assertTrue(candidate_matches(
            "no references are rewritten to person:p-101",
            "Do not rewrite references; relation:participation-103 stays on person:p-103, not person:p-101.",
            "concept_tokens_v1",
        ))
        self.assertTrue(candidate_matches(
            "240/min",
            "Each extractor produces 240 items/min.",
            "concept_tokens_v1",
        ))
        self.assertFalse(candidate_matches(
            "does not change the tentative response",
            "The update changes the tentative response.",
            "concept_tokens_v1",
        ))
        self.assertTrue(candidate_matches(
            "exclude",
            "Remove all dependencies on private source data.",
            "concept_tokens_v1",
        ))

    def test_forbidden_conclusions_ignore_explicit_negation(self):
        forbidden = "rewrite relation:participation-103 to person:p-101"
        self.assertFalse(forbidden_is_asserted(
            forbidden,
            normalize(
                "Do not delete or merge either person, select a survivor, "
                "or rewrite relation:participation-103 to person:p-101."
            ),
        ))
        self.assertTrue(forbidden_is_asserted(
            forbidden,
            normalize("Rewrite relation:participation-103 to person:p-101."),
        ))

    def test_read_only_workspace_allows_reasoning_and_denies_mutation(self):
        corpus = ROOT / "eval" / "corpus-personal-coordination-v0.1"
        documents, _ = load_corpus(corpus)
        before = corpus_hash(documents)
        with tempfile.TemporaryDirectory() as temporary:
            session = Path(temporary) / "session.json"
            base = [
                sys.executable,
                str(ROOT / "workspace_cli.py"),
                "--corpus", str(corpus),
                "--session", str(session),
                "--access-mode", "read_only",
            ]
            opened = subprocess.run(
                [*base, "open", "--scope", "Personal Coordination"],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(opened.returncode, 0, opened.stderr)

            allowed_commands = [
                ["query", "read-only capability", "--scope", "Personal Coordination"],
                ["read", "--path", "Authorization/read-only-capability-contract.md"],
                ["compute", "1 + 1"],
                ["verify", "read-only credentials cannot mutate corpus state", "--scope", "Personal Coordination"],
            ]
            for command in allowed_commands:
                allowed = subprocess.run(
                    [*base, *command],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(allowed.returncode, 0, allowed.stderr)

            denied = subprocess.run(
                [*base, "checkpoint", "--objective", "must not persist"],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(denied.returncode, 77)
            self.assertEqual(json.loads(denied.stdout)["error"]["code"], "capability_denied")
            state = json.loads(session.read_text())
            self.assertIsNone(state["checkpoint"])
            self.assertEqual(state["operations"][-1]["operation"], "denied:checkpoint")

            for operation in ["save", "stage", "correct", "delete", "dream"]:
                unavailable = subprocess.run(
                    [*base, operation],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(unavailable.returncode, 77, operation)
                self.assertEqual(
                    json.loads(unavailable.stdout)["error"]["code"],
                    "capability_denied",
                )

        after = corpus_hash(load_corpus(corpus)[0])
        self.assertEqual(after, before)

        manifest = json.loads(
            (ROOT / "eval" / "personal_coordination_cases.json").read_text()
        )
        case = next(
            item for item in manifest["cases"]
            if item["id"] == "coord-read-only-capability-boundary"
        )
        prompt = render_prompt(case, "workspace")
        self.assertIn("cannot checkpoint or mutate corpus or staged state", prompt)
        self.assertIn("output proposal only", prompt)

    def test_workspace_compute(self):
        self.assertEqual(safe_compute("45.67 + 32.31 + 32.31"), 110.29)
        with self.assertRaises(ValueError):
            safe_compute("__import__('os').getcwd()")

    def test_fixed_pack_is_bounded_and_source_bearing(self):
        case = next(case for case in self.manifest["cases"] if case["id"] == "star-rupture-rail-and-heat-plan")
        context = render_fixed_context(case, BM25Index(self.chunks), self.manifest)
        self.assertLessEqual(len(context), self.manifest["fixed_pack_chars"] + 2000)
        self.assertIn("Topics/Star Rupture/Star Rupture production.md", context)
        self.assertIn("480", context)

    def test_structured_grader_accepts_corpus_prefix(self):
        case = next(case for case in self.manifest["cases"] if case["id"] == "star-rupture-rail-and-heat-plan")
        claims = []
        for rubric in case["rubric"]:
            value = " ".join(check["any"][0] for check in rubric["checks"])
            claims.append({
                "id": rubric["id"],
                "value": value,
                "source_paths": ["corpus/" + rubric["sources_any"][0]],
                "confidence": "high",
            })
        answer = {
            "answer": "Evidence-backed plan.",
            "claims": claims,
            "checkpoint": {
                "objective": "Plan the factory.",
                "current_state": ["Known state"],
                "decisions": ["Safe decision"],
                "open_questions": ["Verify in game"],
                "next_actions": ["Build the first line"],
                "artifacts": ["Source notes"],
            },
        }
        grade = grade_answer(case, answer, {document.path for document in self.documents})
        self.assertTrue(grade["pass"])
        self.assertEqual(grade["citation_validity"], 1.0)

    def test_structured_grader_rejects_duplicate_missing_and_extra_claim_ids(self):
        case = {
            "rubric": [
                {
                    "id": "c1",
                    "checks": [{"any": ["alpha"]}],
                    "sources_any": ["Evidence/source.md"],
                },
                {
                    "id": "c2",
                    "checks": [{"any": ["beta"]}],
                    "sources_any": ["Evidence/source.md"],
                },
            ],
        }
        checkpoint = {
            "objective": "Test exact claim sets.",
            "current_state": ["Evidence is available."],
            "next_actions": ["Reject malformed claim sets."],
            "artifacts": ["Evidence/source.md"],
        }
        valid_claims = [
            {
                "id": "c1",
                "value": "alpha",
                "source_paths": ["Evidence/source.md"],
            },
            {
                "id": "c2",
                "value": "beta",
                "source_paths": ["Evidence/source.md"],
            },
        ]
        corpus = {"Evidence/source.md"}
        valid = grade_answer(
            case,
            {"answer": "", "claims": valid_claims, "checkpoint": checkpoint},
            corpus,
        )
        duplicate = grade_answer(
            case,
            {
                "answer": "",
                "claims": [valid_claims[0], valid_claims[0], valid_claims[1]],
                "checkpoint": checkpoint,
            },
            corpus,
        )
        missing = grade_answer(
            case,
            {
                "answer": "",
                "claims": [valid_claims[0]],
                "checkpoint": checkpoint,
            },
            corpus,
        )
        extra = grade_answer(
            case,
            {
                "answer": "",
                "claims": [
                    *valid_claims,
                    {
                        "id": "c3",
                        "value": "gamma",
                        "source_paths": ["Evidence/source.md"],
                    },
                ],
                "checkpoint": checkpoint,
            },
            corpus,
        )

        self.assertTrue(valid["pass"])
        self.assertTrue(valid["claim_set_valid"])
        self.assertFalse(duplicate["pass"])
        self.assertEqual(duplicate["duplicate_claim_ids"], ["c1"])
        self.assertFalse(missing["pass"])
        self.assertEqual(missing["missing_claim_ids"], ["c2"])
        self.assertFalse(extra["pass"])
        self.assertEqual(extra["extra_claim_ids"], ["c3"])

    def test_claim_cannot_borrow_required_content_from_global_answer(self):
        case = {
            "rubric": [{
                "id": "c1",
                "checks": [{"any": ["required claim detail"]}],
                "sources_any": ["Evidence/source.md"],
            }],
        }
        answer = {
            "answer": "The required claim detail appears only in the global answer.",
            "claims": [{
                "id": "c1",
                "value": "This claim omits it.",
                "source_paths": ["Evidence/source.md"],
            }],
            "checkpoint": {
                "objective": "Test claim isolation.",
                "current_state": ["A claim is incomplete."],
                "next_actions": ["Keep grading claim-local."],
                "artifacts": ["Evidence/source.md"],
            },
        }

        grade = grade_answer(case, answer, {"Evidence/source.md"})

        self.assertFalse(grade["pass"])
        self.assertFalse(grade["claims"][0]["pass"])
        self.assertIsNone(grade["claims"][0]["checks"][0]["matched"])

    def test_claim_with_only_two_of_three_required_checks_fails(self):
        case = {
            "rubric": [{
                "id": "c1",
                "checks": [
                    {"any": ["alpha"]},
                    {"any": ["beta"]},
                    {"any": ["gamma"]},
                ],
                "sources_any": ["Evidence/source.md"],
            }],
        }
        answer = {
            "answer": "",
            "claims": [{
                "id": "c1",
                "value": "alpha beta",
                "source_paths": ["Evidence/source.md"],
            }],
            "checkpoint": {
                "objective": "Test complete checks.",
                "current_state": ["Two details are present."],
                "next_actions": ["Require the third detail."],
                "artifacts": ["Evidence/source.md"],
            },
        }

        grade = grade_answer(case, answer, {"Evidence/source.md"})

        self.assertFalse(grade["pass"])
        self.assertEqual(grade["claims"][0]["content_fraction"], 0.6667)
        self.assertFalse(grade["claims"][0]["pass"])

    def test_fabricated_suffix_citation_does_not_match_expected_path(self):
        case = {
            "rubric": [{
                "id": "c1",
                "checks": [{"any": ["supported fact"]}],
                "sources_any": ["Authority/source.md"],
            }],
        }
        answer = {
            "answer": "",
            "claims": [{
                "id": "c1",
                "value": "supported fact",
                "source_paths": ["fabricated/Authority/source.md"],
            }],
            "checkpoint": {
                "objective": "Test exact citations.",
                "current_state": ["The claim has a fabricated path."],
                "next_actions": ["Reject suffix matching."],
                "artifacts": ["Authority/source.md"],
            },
        }

        grade = grade_answer(case, answer, {"Authority/source.md"})

        self.assertFalse(grade["pass"])
        self.assertFalse(grade["claims"][0]["source_hit"])
        self.assertEqual(grade["citation_validity"], 0.0)

    def test_sources_all_requires_every_exact_path(self):
        case = {
            "rubric": [{
                "id": "c1",
                "checks": [{"any": ["joined conclusion"]}],
                "sources_any": ["Evidence/alpha.md", "Evidence/beta.md"],
                "sources_all": ["Evidence/alpha.md", "Evidence/beta.md"],
            }],
        }
        answer = {
            "answer": "",
            "claims": [{
                "id": "c1",
                "value": "joined conclusion",
                "source_paths": ["Evidence/alpha.md"],
            }],
            "checkpoint": {
                "objective": "Test all-source evidence.",
                "current_state": ["Only one source is cited."],
                "next_actions": ["Require both exact paths."],
                "artifacts": ["Evidence/alpha.md", "Evidence/beta.md"],
            },
        }
        corpus_paths = {"Evidence/alpha.md", "Evidence/beta.md"}

        incomplete = grade_answer(case, answer, corpus_paths)
        answer["claims"][0]["source_paths"] = [
            "Evidence/alpha.md",
            "fabricated/Evidence/beta.md",
        ]
        fabricated = grade_answer(case, answer, corpus_paths)
        answer["claims"][0]["source_paths"] = [
            "Evidence/alpha.md",
            "Evidence/beta.md",
        ]
        complete = grade_answer(case, answer, corpus_paths)

        self.assertFalse(incomplete["pass"])
        self.assertFalse(incomplete["claims"][0]["source_hit"])
        self.assertFalse(fabricated["pass"])
        self.assertFalse(fabricated["claims"][0]["source_hit"])
        self.assertTrue(complete["pass"])

    def test_event_metrics_count_completed_calls_and_cached_input(self):
        with tempfile.TemporaryDirectory() as temp:
            events = Path(temp) / "events.jsonl"
            rows = [
                {"type": "item.started", "item": {"type": "command_execution"}},
                {
                    "type": "item.completed",
                    "item": {"type": "command_execution", "aggregated_output": "hello"},
                },
                {
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": 100,
                        "cached_input_tokens": 80,
                        "output_tokens": 10,
                    },
                },
            ]
            events.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
            metrics = parse_event_metrics(events)
            self.assertEqual(metrics["commands"], 1)
            self.assertEqual(metrics["command_output_chars"], 5)
            self.assertEqual(metrics["tokens"]["input_tokens"], 100)
            self.assertEqual(metrics["tokens"]["cached_input_tokens"], 80)

    def test_workspace_session_commits_are_atomic_and_reopenable(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            corpus = root / "corpus"
            corpus.mkdir()
            (corpus / "Alpha.md").write_text("# Alpha\n\nDurable state survives concurrent operations.\n")
            session = root / "session.json"
            command = [
                sys.executable,
                str(ROOT / "workspace_cli.py"),
                "--corpus",
                str(corpus),
                "--session",
                str(session),
                "compute",
            ]
            with ThreadPoolExecutor(max_workers=6) as executor:
                results = list(executor.map(
                    lambda value: subprocess.run(
                        [*command, f"{value} + 1"],
                        check=False,
                        capture_output=True,
                        text=True,
                    ),
                    range(6),
                ))
            self.assertTrue(all(result.returncode == 0 for result in results))
            state = json.loads(session.read_text())
            self.assertEqual(len(state["operations"]), 6)

            checkpoint = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "workspace_cli.py"),
                    "--corpus",
                    str(corpus),
                    "--session",
                    str(session),
                    "checkpoint",
                    "--objective",
                    "Resume safely",
                    "--current-state",
                    "Concurrent writes passed",
                    "--next-action",
                    "Reopen the checkpoint",
                    "--artifact",
                    "Alpha.md",
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            self.assertIn("Resume safely", checkpoint.stdout)
            status = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "workspace_cli.py"),
                    "--corpus",
                    str(corpus),
                    "--session",
                    str(session),
                    "status",
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            reopened = json.loads(status.stdout)
            self.assertEqual(reopened["checkpoint"]["objective"], "Resume safely")
            self.assertEqual(len(reopened["operations"]), 7)


if __name__ == "__main__":
    unittest.main()
