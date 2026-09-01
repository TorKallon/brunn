import asyncio
import copy
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import unittest
import urllib.error
import urllib.request
from pathlib import Path
from unittest.mock import patch

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from agent_work_eval import grade_answer  # noqa: E402
from interface_eval import (  # noqa: E402
    DEFAULT_CODEX,
    FILESYSTEM_INTERFACE,
    OPENCLAW,
    OPENCLAW_NPM,
    Job,
    LocalModelGateway,
    ServiceBroker,
    TrustedInspectionServer,
    assert_resume_compatible,
    binary_asset_specs,
    build_run_manifest,
    cleanup_openclaw_state,
    codex_command,
    create_jobs,
    agent_environment,
    atomic_write_bytes,
    grade_binary_access,
    group_summary,
    load_cases,
    load_provisioning,
    normalize_mcp_trace,
    openclaw_config,
    paired_comparisons,
    parent_model_credentials,
    prepare_openclaw,
    prepare_run_dir,
    render_prompt,
    save_provisioning,
    quality_publishability,
    load_existing_record,
    canonical_record_path,
    model_gateway_contract,
    record_seal_path,
    response_api_usage,
    render_report,
    trusted_model_usage,
    verify_evidence_sandbox_profile,
    write_record_seal,
    write_evidence_sandbox_profile,
    write_run_manifest,
)
from transition_eval import grade_transition_answer  # noqa: E402


class InterfaceEvalTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cases, cls.validation = load_cases()

    def test_complete_protocol_shape(self):
        self.assertEqual(self.validation["errors"], [])
        self.assertEqual(len(self.cases), 52)
        self.assertEqual(
            sum(len(case.case["rubric"]) for case in self.cases),
            201,
        )
        self.assertEqual(len(create_jobs(self.cases, None, None)), 416)
        self.assertEqual(
            {
                case.case["id"]
                for case in self.cases
                if case.suite == "binary"
            },
            {
                "receipt-expense-evidence",
                "factory-sqlite-current-state",
                "factory-map-route",
                "switzerland-booking-state",
                "kids-ski-schedule-change",
                "brunn-state-alpha-artifact",
                "office-image-layout",
            },
        )

    def test_model_gateway_allows_only_models_and_responses(self):
        for direct_openai, prefix in (
            (False, "/backend-api/codex"),
            (True, "/v1"),
        ):
            gateway = LocalModelGateway(direct_openai=direct_openai)
            self.assertTrue(gateway.request_allowed("GET", f"{prefix}/models"))
            self.assertTrue(
                gateway.request_allowed("GET", f"{prefix}/models?client=codex")
            )
            self.assertTrue(gateway.request_allowed("POST", f"{prefix}/responses"))
            self.assertTrue(
                gateway.request_allowed("POST", f"{prefix}/responses/compact")
            )
            for method, path in (
                ("DELETE", f"{prefix}/responses"),
                ("POST", f"{prefix}/files"),
                ("GET", f"{prefix}/responses"),
                ("POST", f"{prefix}/responses/anything"),
                ("POST", f"{prefix}/../v1/files"),
                ("POST", f"{prefix}%2fresponses"),
            ):
                self.assertFalse(gateway.request_allowed(method, path))

    def test_model_gateway_requires_capability_and_exact_model_contract(self):
        gateway = LocalModelGateway(
            direct_openai=True,
            expected_model="gpt-eval",
            expected_reasoning_effort="xhigh",
        )
        headers = {"authorization": f"Bearer {gateway.capability}"}
        valid = json.dumps({
            "model": "gpt-eval",
            "reasoning": {"effort": "xhigh"},
            "store": False,
            "tools": [{"type": "function", "name": "local_read"}],
        }).encode()
        receipt = gateway.validate_child_request(
            "POST",
            "/v1/responses",
            headers,
            valid,
        )
        self.assertEqual(receipt["model"], "gpt-eval")
        self.assertEqual(receipt["reasoning_effort"], "xhigh")
        self.assertEqual(receipt["tool_types"], ["function"])
        self.assertTrue(receipt["request_sha256"].startswith("sha256:"))

        with self.assertRaisesRegex(PermissionError, "capability"):
            gateway.validate_child_request(
                "POST",
                "/v1/responses",
                {"authorization": "Bearer forged"},
                valid,
            )
        for mutation, message in (
            ({"model": "gpt-other"}, "predeclared model"),
            ({"reasoning": {"effort": "low"}}, "reasoning effort"),
            ({"store": True}, "server storage"),
            ({"previous_response_id": "resp:history"}, "prior server-side"),
            ({"tools": [{"type": "web_search"}]}, "server-side evidence tool"),
        ):
            payload = json.loads(valid)
            payload.update(mutation)
            with self.assertRaisesRegex(PermissionError, message):
                gateway.validate_child_request(
                    "POST",
                    "/v1/responses",
                    headers,
                    json.dumps(payload).encode(),
                )

    def test_model_usage_is_trusted_only_when_every_generation_receipt_is_complete(self):
        complete = {
            "method": "POST",
            "path": "/v1/responses",
            "allowed": True,
            "contract_valid": True,
            "upstream_http_status": 200,
            "response_body_complete": True,
            "usage": {
                "input_tokens": 100,
                "cached_input_tokens": 25,
                "cache_write_input_tokens": 0,
                "output_tokens": 10,
                "uncached_input_tokens": 75,
            },
        }
        self.assertEqual(
            trusted_model_usage([complete]),
            complete["usage"],
        )
        partial = {**complete, "usage": None}
        self.assertIsNone(trusted_model_usage([complete, partial]))
        contract = model_gateway_contract([complete, partial])
        self.assertFalse(contract["complete"])
        self.assertEqual(contract["missing_usage_indexes"], [1])

    def test_model_gateway_dechunks_usage_before_accounting(self):
        payload = (
            b'data: {"type":"response.completed","response":{"usage":'
            b'{"input_tokens":12,"output_tokens":3,'
            b'"input_tokens_details":{"cached_tokens":4}}}}\n\n'
        )

        class CaptureWriter:
            def __init__(self):
                self.data = bytearray()

            def write(self, value):
                self.data.extend(value)

            async def drain(self):
                return None

        async def scenario():
            async def serve(_reader, writer):
                writer.write(
                    b"HTTP/1.1 200 OK\r\n"
                    b"Content-Type: text/event-stream\r\n"
                    b"Transfer-Encoding: chunked\r\n\r\n"
                )
                for chunk in (payload[:19], payload[19:]):
                    writer.write(f"{len(chunk):x}\r\n".encode() + chunk + b"\r\n")
                writer.write(b"0\r\n\r\n")
                await writer.drain()
                writer.close()
                await writer.wait_closed()

            server = await asyncio.start_server(serve, "127.0.0.1", 0)
            port = server.sockets[0].getsockname()[1]
            reader, upstream_writer = await asyncio.open_connection(
                "127.0.0.1",
                port,
            )
            downstream = CaptureWriter()
            gateway = LocalModelGateway(direct_openai=True)
            try:
                result = await gateway._relay_upstream_response(reader, downstream)
            finally:
                upstream_writer.close()
                await upstream_writer.wait_closed()
                server.close()
                await server.wait_closed()
            return result

        status, _, captured, truncated = asyncio.run(scenario())
        self.assertEqual(status, 200)
        self.assertEqual(captured, payload)
        self.assertFalse(truncated)
        self.assertEqual(
            response_api_usage(captured),
            {
                "input_tokens": 12,
                "cached_input_tokens": 4,
                "cache_write_input_tokens": 0,
                "output_tokens": 3,
                "uncached_input_tokens": 8,
            },
        )

    def test_interfaces_share_task_contract_but_have_distinct_access_rules(self):
        eval_case = next(
            case
            for case in self.cases
            if case.qualified_id == "work:warmind-parser-learning"
        )
        metadata = {
            "authorization_scope": "scope:test",
            "checkpoint_id": None,
        }
        prompts = {
            interface: render_prompt(
                Job(eval_case, "codex", interface),
                metadata,
            )
            for interface in ("cli", "mcp", "api")
        }
        for prompt in prompts.values():
            self.assertIn(eval_case.case["task"], prompt)
            self.assertIn("exactly one self-contained entry", prompt)
            self.assertIn("Build a checklist", prompt)
            self.assertIn("four Brunn State calls", prompt)
            self.assertIn("Every claim must cite", prompt)
            self.assertIn("stable or source ID", prompt)
        self.assertIn("./memory open", prompts["cli"])
        self.assertIn("`memory.open`", prompts["mcp"])
        self.assertIn("Omit `resume_checkpoint_ref`", prompts["mcp"])
        self.assertIn("must not invent one", prompts["mcp"])
        self.assertIn("`source_episode:...` returned by read", prompts["mcp"])
        self.assertIn("/v1/memory/open", prompts["api"])
        self.assertIn("checkpoint body shape exactly", prompts["api"])
        self.assertIn('"scope":"scope:test"', prompts["api"])
        self.assertIn("Never put a human display", prompts["mcp"])
        for prompt in prompts.values():
            self.assertNotIn("asset-fetch", prompt)
            self.assertNotIn("asset.fetch", prompt)
            self.assertNotIn("/v1/assets", prompt)

    def test_binary_interfaces_require_verified_native_fetches(self):
        eval_case = next(
            case
            for case in self.cases
            if case.qualified_id == "binary:receipt-expense-evidence"
        )
        metadata = {
            "authorization_scope": "scope:binary-test",
            "checkpoint_id": None,
        }
        prompts = {
            interface: render_prompt(
                Job(eval_case, "codex", interface),
                metadata,
            )
            for interface in ("cli", "mcp", "api")
        }
        for prompt in prompts.values():
            self.assertIn("eight Brunn State operations", prompt)
            self.assertIn("session-pinned size and SHA-256", prompt)
            self.assertIn("returned private", prompt)
        self.assertIn("./memory asset-fetch", prompts["cli"])
        self.assertIn("`asset.fetch`", prompts["mcp"])
        self.assertIn("./api download", prompts["api"])

    def test_binary_answer_fails_without_exact_access_and_inspection_trace(self):
        eval_case = next(
            case
            for case in self.cases
            if case.qualified_id == "binary:receipt-expense-evidence"
        )
        job = Job(eval_case, "codex", "cli")
        imported = [
            {
                **spec,
                "asset_ref": f"asset:{index}",
                "version": f"asset-version:{index}",
            }
            for index, spec in enumerate(binary_asset_specs(eval_case), start=1)
        ]
        metadata = {"binary_import": {"assets": imported}}
        with tempfile.TemporaryDirectory() as temporary:
            grade = grade_binary_access(
                job,
                metadata,
                {"service_operations": []},
                Path(temporary),
            )

        self.assertFalse(grade["pass"])
        self.assertTrue(grade["assets"])
        self.assertTrue(all(not item["metadata"] for item in grade["assets"]))
        self.assertTrue(all(not item["verified_fetch"] for item in grade["assets"]))
        self.assertTrue(all(not item["local_inspection"] for item in grade["assets"]))

    def test_binary_answer_passes_with_complete_exact_trace(self):
        eval_case = next(
            case
            for case in self.cases
            if case.qualified_id == "binary:receipt-expense-evidence"
        )
        job = Job(eval_case, "codex", "cli")
        imported = [
            {
                **spec,
                "asset_ref": f"asset:{index}",
                "version": f"asset-version:{index}",
            }
            for index, spec in enumerate(binary_asset_specs(eval_case), start=1)
        ]
        metadata = {"binary_import": {"assets": imported}}
        operations = []
        inspections = []
        for asset in imported:
            service_identity = {
                "http_status": 200,
                "service_status": "complete",
                "trusted_broker_receipt": True,
                "asset_ref": asset["asset_ref"],
                "asset_version": asset["version"],
                "asset_content_hash": asset["content_hash"],
                "asset_size_bytes": asset["size_bytes"],
            }
            operations.extend([
                {"operation": "asset.metadata", **service_identity},
                {
                    "operation": "asset.fetch",
                    "binary_bytes": asset["size_bytes"],
                    **service_identity,
                },
            ])
            inspections.append({
                "operation": "trusted_local_inspection",
                "logical_paths": [asset["path"]],
                "content_hash": asset["content_hash"],
                "size_bytes": asset["size_bytes"],
                "bytes_read": asset["size_bytes"],
                "local_path": f"/private/run/assets/{asset['path']}",
                "verified": True,
            })

        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            grade = grade_binary_access(
                job,
                metadata,
                {
                    "service_operations": operations,
                    "trusted_asset_inspections": inspections,
                },
                run_dir,
            )

        self.assertTrue(grade["pass"])
        self.assertTrue(all(item["pass"] for item in grade["assets"]))
        self.assertFalse(grade["semantic_evidence_complete"])
        self.assertTrue(
            all(not item["semantic_evidence_complete"] for item in grade["assets"])
        )

    def test_sqlite_access_requires_query_results_bound_to_hidden_values(self):
        eval_case = next(
            case
            for case in self.cases
            if case.qualified_id == "binary:factory-sqlite-current-state"
        )
        job = Job(eval_case, "codex", "cli")
        spec = binary_asset_specs(eval_case)[0]
        imported = {
            **spec,
            "asset_ref": "asset:factory",
            "version": "asset-version:factory",
        }
        identity = {
            "http_status": 200,
            "service_status": "complete",
            "trusted_broker_receipt": True,
            "asset_ref": imported["asset_ref"],
            "asset_version": imported["version"],
            "asset_content_hash": imported["content_hash"],
            "asset_size_bytes": imported["size_bytes"],
        }
        operations = [
            {"operation": "asset.metadata", **identity},
            {
                "operation": "asset.fetch",
                "binary_bytes": imported["size_bytes"],
                **identity,
            },
        ]

        def inspection(result):
            return {
                "operation": "trusted_sqlite_query",
                "logical_paths": [imported["path"]],
                "content_hash": imported["content_hash"],
                "snapshot_sha256": imported["content_hash"],
                "size_bytes": imported["size_bytes"],
                "bytes_read": imported["size_bytes"],
                "verified": True,
                "sqlite_result": result,
            }

        metadata = {"binary_import": {"assets": [imported]}}
        irrelevant = grade_binary_access(
            job,
            metadata,
            {
                "service_operations": operations,
                "trusted_asset_inspections": [
                    inspection({
                        "columns": ["count"],
                        "rows": [[4]],
                        "row_count": 1,
                    })
                ],
            },
            Path("/unused"),
        )
        self.assertFalse(irrelevant["pass"])
        self.assertFalse(irrelevant["assets"][0]["local_inspection"])

        relevant = grade_binary_access(
            job,
            metadata,
            {
                "service_operations": operations,
                "trusted_asset_inspections": [
                    inspection({
                        "columns": ["site", "ingots"],
                        "rows": [
                            ["All sites", 258],
                            ["Grand Basin", 45],
                            ["Oil Mesa", 18],
                        ],
                        "row_count": 3,
                    })
                ],
            },
            Path("/unused"),
        )
        self.assertTrue(relevant["pass"])
        self.assertTrue(relevant["semantic_evidence_complete"])

    def test_binary_case_roots_include_only_card_declared_native_assets(self):
        all_native_paths = {
            path
            for case in self.cases
            if case.suite == "binary"
            for path in case.binary_asset_paths
        }
        for eval_case in (case for case in self.cases if case.suite == "binary"):
            present = {
                path
                for path in eval_case.corpus_paths
                if path in all_native_paths
            }
            self.assertEqual(
                present,
                set(eval_case.binary_asset_paths),
                eval_case.qualified_id,
            )
            self.assertEqual(
                set(eval_case.provision_asset_paths),
                set(eval_case.binary_asset_paths),
                eval_case.qualified_id,
            )

    def test_resume_manifest_binds_model_runtime_harness_fixtures_and_helpers(self):
        eval_case = next(case for case in self.cases if case.suite == "binary")
        job = Job(eval_case, "codex", FILESYSTEM_INTERFACE)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            codex = root / "codex"
            codex.write_bytes(b"runtime-v1")
            manifest = build_run_manifest(
                "run-manifest-test",
                [job],
                model="gpt-5.6-sol",
                codex=codex,
                timeout_seconds=600,
                native_index_timeout=600.0,
                service_api_url="http://unused.example",
                direct_openai=False,
            )
            path = root / ".run-manifest.json"
            write_run_manifest(path, manifest)

            self.assertEqual(path.stat().st_mode & 0o777, 0o400)
            self.assertIn("interface_eval", manifest["harness"])
            self.assertIn(eval_case.qualified_id, manifest["fixtures"]["cases"])
            self.assertIn("filesystem_asset", manifest["helpers"])
            self.assertIn("trusted_asset_inspect", manifest["helpers"])
            self.assertIn("codex", manifest["runtime"]["agents"])
            assert_resume_compatible(path, manifest)

            mutations = {
                "model": ("model", "id"),
                "runtime": ("runtime", "agents", "codex", "executable", "sha256"),
                "harness": ("harness", "interface_eval", "sha256"),
                "fixture": (
                    "fixtures",
                    "cases",
                    eval_case.qualified_id,
                    "sha256",
                ),
                "helper": ("helpers", "filesystem_asset", "sha256"),
            }
            for label, keys in mutations.items():
                changed = copy.deepcopy(manifest)
                target = changed
                for key in keys[:-1]:
                    target = target[key]
                target[keys[-1]] = f"changed-{label}"
                with self.subTest(label=label), self.assertRaises(ValueError):
                    assert_resume_compatible(path, changed)

            path.chmod(0o600)
            with self.assertRaisesRegex(ValueError, "writable"):
                assert_resume_compatible(path, manifest)

    def test_filesystem_control_is_explicit_and_uses_only_frozen_sources(self):
        eval_case = next(
            case
            for case in self.cases
            if case.qualified_id == "work:warmind-parser-learning"
        )
        job = Job(eval_case, "openclaw", FILESYSTEM_INTERFACE)
        metadata = {
            "authorization_scope": "filesystem",
            "checkpoint_id": None,
        }
        prompt = render_prompt(job, metadata)
        self.assertIn("frozen evidence under `./corpus`", prompt)
        self.assertIn("Do not browse or read outside", prompt)
        self.assertNotIn("four Brunn State calls", prompt)
        self.assertNotIn("`memory.open`", prompt)
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = prepare_run_dir(
                job,
                {**metadata, "token": ""},
                run_id="filesystem-test",
                run_root=Path(temporary),
            )
            self.assertTrue((run_dir / "corpus").is_dir())
            self.assertFalse((run_dir / "corpus").is_symlink())
            self.assertEqual(
                (run_dir / "corpus" / next(iter(eval_case.corpus_paths))).stat().st_mode
                & 0o222,
                0,
            )
            self.assertFalse((run_dir / "checkpoint.json").exists())

    def test_filesystem_transition_receives_checkpoint_and_delta(self):
        eval_case = next(case for case in self.cases if case.transition)
        job = Job(eval_case, "codex", FILESYSTEM_INTERFACE)
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = prepare_run_dir(
                job,
                {
                    "token": "",
                    "authorization_scope": "filesystem",
                    "checkpoint_id": None,
                },
                run_id="filesystem-transition-test",
                run_root=Path(temporary),
            )
            self.assertTrue((run_dir / "corpus").is_dir())
            self.assertFalse((run_dir / "corpus").is_symlink())
            self.assertTrue((run_dir / "checkpoint.json").is_file())
            self.assertTrue((run_dir / "delta.json").is_file())
            self.assertTrue((run_dir / eval_case.case["delta_path"]).is_file())
            self.assertFalse(
                (run_dir / eval_case.case["delta_path"]).is_symlink()
            )

    def test_repetitions_have_distinct_paths_and_deterministic_random_order(self):
        eval_case = self.cases[0]
        first = create_jobs(
            [eval_case],
            ["codex"],
            ["cli", "filesystem"],
            repetitions=3,
            randomization_seed=27,
        )
        second = create_jobs(
            [eval_case],
            ["codex"],
            ["cli", "filesystem"],
            repetitions=3,
            randomization_seed=27,
        )
        self.assertEqual([job.key for job in first], [job.key for job in second])
        self.assertEqual(len({job.key for job in first}), 6)
        with tempfile.TemporaryDirectory() as temporary:
            self.assertEqual(
                len({job.run_dir(Path(temporary)) for job in first}),
                6,
            )

    def test_binary_hidden_answers_do_not_appear_in_rendered_prompts(self):
        metadata = {
            "authorization_scope": "scope:prompt-test",
            "checkpoint_id": None,
        }
        cards = json.loads(
            (
                Path(__file__).resolve().parents[1]
                / "eval"
                / "binary_asset_reasoning_cards.json"
            ).read_text(encoding="utf-8")
        )
        by_id = {card["id"]: card for card in cards["cards"]}
        for eval_case in (case for case in self.cases if case.suite == "binary"):
            rendered = "\n".join(
                render_prompt(Job(eval_case, agent, interface), metadata)
                for agent in ("codex", "openclaw")
                for interface in ("cli", "mcp", "api", "filesystem")
            ).casefold()
            for value in by_id[eval_case.case["id"]]["withheld_values"]:
                with self.subTest(case=eval_case.case["id"], value=value):
                    self.assertNotIn(value.casefold(), rendered)

    def test_agent_environment_does_not_forward_unrelated_secrets(self):
        eval_case = self.cases[0]
        with patch.dict(
            os.environ,
            {
                "PATH": "/usr/bin:/bin",
                "HOME": str(Path.home()),
                "BRUNN_API_URL": "http://127.0.0.1:18110",
                "AWS_SECRET_ACCESS_KEY": "must-not-leak",
                "OPENAI_API_KEY": "subscription-run-must-not-forward",
            },
            clear=True,
        ):
            filesystem = agent_environment(
                Job(eval_case, "codex", "filesystem"),
                {"token": ""},
                run_id="env-test",
                run_dir=Path("/tmp/env-test"),
            )
        self.assertNotIn("AWS_SECRET_ACCESS_KEY", filesystem)
        self.assertNotIn("OPENAI_API_KEY", filesystem)
        self.assertNotIn("BRUNN_API_URL", filesystem)
        self.assertEqual(filesystem["HOME"], "/tmp/env-test/.agent-home")
        self.assertEqual(filesystem["CODEX_HOME"], "/tmp/env-test/.agent-home/.codex")

    def test_outer_sandbox_blocks_benchmark_source_but_allows_run_copy(self):
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            local = run_dir / "local.txt"
            local.write_text("allowed", encoding="utf-8")
            agent_home = run_dir / ".agent-home"
            agent_home.mkdir()
            profile = write_evidence_sandbox_profile(run_dir)
            allowed = subprocess.run(
                [
                    "/usr/bin/sandbox-exec",
                    "-f",
                    str(profile),
                    "/bin/cat",
                    str(local),
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            denied = subprocess.run(
                [
                    "/usr/bin/sandbox-exec",
                    "-f",
                    str(profile),
                    "/bin/cat",
                    str(
                        Path(__file__).resolve().parents[1]
                        / "eval"
                        / "binary_asset_reasoning_cards.json"
                    ),
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            home_denied = subprocess.run(
                [
                    "/usr/bin/sandbox-exec",
                    "-f",
                    str(profile),
                    "/bin/cat",
                    str(Path.home() / ".codex" / "auth.json"),
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            openclaw_runtime_manifests = (
                OPENCLAW_NPM
                / "node_modules"
                / "@openclaw"
                / "codex"
                / "package.json",
                OPENCLAW_NPM / "node_modules" / "openclaw" / "package.json",
            )
            openclaw_runtime_manifest = next(
                path for path in openclaw_runtime_manifests if path.is_file()
            )
            openclaw_runtime_allowed = subprocess.run(
                [
                    "/usr/bin/sandbox-exec",
                    "-f",
                    str(profile),
                    "/bin/cat",
                    str(openclaw_runtime_manifest),
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            openclaw_state_denied = subprocess.run(
                [
                    "/usr/bin/sandbox-exec",
                    "-f",
                    str(profile),
                    "/bin/cat",
                    str(Path.home() / ".openclaw" / "openclaw.json"),
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            runtime_env = {
                "HOME": str(agent_home),
                "CODEX_HOME": str(agent_home / ".codex"),
                "TMPDIR": str(run_dir),
                "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
            }
            codex_runtime_allowed = subprocess.run(
                [
                    "/usr/bin/sandbox-exec",
                    "-f",
                    str(profile),
                    str(DEFAULT_CODEX.resolve()),
                    "--version",
                ],
                cwd=run_dir,
                env=runtime_env,
                capture_output=True,
                text=True,
                check=False,
            )
            openclaw_launcher_allowed = subprocess.run(
                [
                    "/usr/bin/sandbox-exec",
                    "-f",
                    str(profile),
                    str(OPENCLAW.resolve()),
                    "--version",
                ],
                cwd=run_dir,
                env=runtime_env,
                capture_output=True,
                text=True,
                check=False,
            )
        self.assertEqual(allowed.returncode, 0)
        self.assertEqual(allowed.stdout, "allowed")
        self.assertNotEqual(denied.returncode, 0)
        self.assertIn("Operation not permitted", denied.stderr)
        self.assertNotEqual(home_denied.returncode, 0)
        self.assertIn("Operation not permitted", home_denied.stderr)
        self.assertEqual(openclaw_runtime_allowed.returncode, 0)
        self.assertIn(
            json.loads(openclaw_runtime_allowed.stdout)["name"],
            {"@openclaw/codex", "openclaw"},
        )
        self.assertNotEqual(openclaw_state_denied.returncode, 0)
        self.assertIn("Operation not permitted", openclaw_state_denied.stderr)
        self.assertEqual(codex_runtime_allowed.returncode, 0)
        self.assertIn("codex-cli", codex_runtime_allowed.stdout)
        self.assertNotIn("Operation not permitted", openclaw_launcher_allowed.stderr)
        if openclaw_launcher_allowed.returncode == 0:
            self.assertIn("OpenClaw", openclaw_launcher_allowed.stdout)
        else:
            self.assertIn(
                "Node.js",
                openclaw_launcher_allowed.stdout + openclaw_launcher_allowed.stderr,
            )

    def test_report_handles_incomplete_parent_token_receipts(self):
        result = {
            "runs": 1,
            "workflow_cases_passed": 0,
            "claims_passed": 0,
            "claims_total": 4,
            "valid_answers": 0,
            "process_failures": 1,
            "mean_score": 0.0,
            "tokens_mean": {
                "input_tokens": 0.0,
                "uncached_input_tokens": 0.0,
                "output_tokens": 0.0,
            },
            "tokens_median": {
                "input_tokens": 0.0,
                "uncached_input_tokens": 0.0,
                "output_tokens": 0.0,
            },
            "tokens_p95": {
                "input_tokens": 0.0,
                "uncached_input_tokens": 0.0,
                "output_tokens": 0.0,
            },
            "mean_service_calls": 0.0,
            "median_service_calls": 0.0,
            "p95_service_calls": 0.0,
            "mean_elapsed_seconds": 0.0,
            "median_elapsed_seconds": 0.0,
            "p95_elapsed_seconds": 0.0,
        }
        report = render_report({
            "run_id": "failed-run",
            "records": [{}],
            "protocol": {
                "model": "gpt-5.6-sol",
                "case_count": 1,
                "claim_count": 4,
                "repetitions": 1,
                "randomization_seed": 7,
                "interfaces": ["filesystem"],
            },
            "evaluation_status": {
                "publishable_for_quality_comparison": False,
                "blockers": ["one or more agent processes failed"],
            },
            "summary": {
                "cells": {"codex/filesystem": result},
                "paired_comparisons": {},
                "by_suite": {"work": {"codex/filesystem": result}},
            },
            "pricing": {
                "complete": False,
                "basis": "unavailable: parent-owned token receipts are incomplete",
            },
            "output_path": "results/failed.json",
            "report_path": "results/failed.md",
        })
        self.assertIn("API-equivalent cost is unavailable", report)
        self.assertNotIn("## Distribution and API-equivalent cost", report)

    def test_sandbox_preflight_denies_current_run_custody_and_local_escape_tools(self):
        with tempfile.TemporaryDirectory(dir="/Users/Shared") as temporary:
            root = Path(temporary)
            protected = root / ".run-manifest.json"
            protected.write_text('{"answers":"hidden"}\n', encoding="utf-8")
            run_dir = root / "run"
            run_dir.mkdir()
            (run_dir / "prompt.txt").write_text("screening prompt\n", encoding="utf-8")
            profile = write_evidence_sandbox_profile(run_dir)
            result = verify_evidence_sandbox_profile(
                profile,
                run_dir,
                protected_paths=[protected],
            )
        self.assertTrue(result["pass"])
        self.assertFalse(result["publishable_containment"])
        checks = {item["name"]: item["pass"] for item in result["checks"]}
        self.assertTrue(checks["protected_path_0_denied"])
        self.assertTrue(checks["launchd_escape_denied"])
        self.assertTrue(checks["keychain_escape_denied"])
        if "container_daemon_escape_denied" in checks:
            self.assertTrue(checks["container_daemon_escape_denied"])

    def test_parent_atomic_write_replaces_symlink_without_following_it(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            outside = root / "outside.txt"
            outside.write_text("do not overwrite", encoding="utf-8")
            destination = root / "run" / "record.json"
            destination.parent.mkdir()
            destination.symlink_to(outside)
            atomic_write_bytes(destination, b'{"trusted":true}\n', mode=0o400)
            self.assertEqual(outside.read_text(encoding="utf-8"), "do not overwrite")
            self.assertFalse(destination.is_symlink())
            self.assertEqual(destination.read_bytes(), b'{"trusted":true}\n')

    def test_trusted_inspector_accepts_exact_run_copy_and_rejects_source_tree(self):
        eval_case = next(
            case
            for case in self.cases
            if case.qualified_id == "binary:factory-sqlite-current-state"
        )
        job = Job(eval_case, "codex", "filesystem")

        async def scenario(run_dir: Path):
            logical_path = eval_case.binary_asset_paths[0]
            target = run_dir / "corpus" / logical_path
            target.parent.mkdir(parents=True)
            shutil.copy2(eval_case.corpus_root / logical_path, target)
            server = TrustedInspectionServer(job, run_dir)
            await server.start()

            def inspect(path: Path, **extra):
                request = urllib.request.Request(
                    str(server.url),
                    data=json.dumps({
                        "capability": server.capability,
                        "path": str(path),
                        **extra,
                    }).encode(),
                    headers={"Content-Type": "application/json"},
                    method="POST",
                )
                with urllib.request.urlopen(request, timeout=5) as response:
                    return json.loads(response.read())

            try:
                accepted = await asyncio.to_thread(inspect, target.resolve())
                queried = await asyncio.to_thread(
                    inspect,
                    target.resolve(),
                    operation="sqlite_query",
                    query="SELECT count(*) AS sites, sum(ingots) AS ingots FROM sites",
                )
                with self.assertRaises(urllib.error.HTTPError) as rejected:
                    await asyncio.to_thread(
                        inspect,
                        eval_case.corpus_root / logical_path,
                    )
                rejected.exception.close()
            finally:
                await server.close()
            return accepted, queried, server.records

        with tempfile.TemporaryDirectory() as temporary:
            accepted, queried, records = asyncio.run(scenario(Path(temporary)))
        self.assertTrue(accepted["verified"])
        self.assertEqual(
            accepted["logical_paths"],
            [eval_case.binary_asset_paths[0]],
        )
        self.assertEqual(
            queried["sqlite_result"],
            {"columns": ["sites", "ingots"], "rows": [[4, 258]], "row_count": 1},
        )
        self.assertEqual(queried["snapshot_sha256"], queried["content_hash"])
        self.assertEqual(records, [accepted, queried])

    def test_service_broker_hides_real_token_and_blocks_read_only_writes(self):
        eval_case = next(
            case
            for case in self.cases
            if case.qualified_id == "personal:coord-read-only-capability-boundary"
        )
        job = Job(eval_case, "codex", "api")

        async def scenario():
            upstream_requests = []

            async def upstream(reader, writer):
                header = await reader.readuntil(b"\r\n\r\n")
                request_line, *header_lines = header.decode().split("\r\n")
                method, path, _ = request_line.split(" ", 2)
                headers = {
                    key.casefold(): value.strip()
                    for line in header_lines
                    if ":" in line
                    for key, value in [line.split(":", 1)]
                }
                length = int(headers.get("content-length", "0"))
                if length:
                    await reader.readexactly(length)
                upstream_requests.append((method, path, headers))
                body = json.dumps({
                    "status": "complete",
                    "data": {
                        "session_id": "session:trusted",
                        "corpus_revision": "revision:trusted",
                        "evidence": [{
                            "content_scope": "complete_source",
                            "path": "source.md",
                            "content": "trusted evidence",
                        }],
                    },
                }).encode()
                writer.write(
                    b"HTTP/1.1 200 OK\r\n"
                    b"Content-Type: application/json\r\n"
                    + f"Content-Length: {len(body)}\r\n\r\n".encode()
                    + body
                )
                await writer.drain()
                writer.close()
                await writer.wait_closed()

            upstream_server = await asyncio.start_server(
                upstream,
                "127.0.0.1",
                0,
            )
            upstream_port = upstream_server.sockets[0].getsockname()[1]
            broker = ServiceBroker(
                job,
                {"token": "real-service-token"},
                run_id="broker-test",
                service_api_url=f"http://127.0.0.1:{upstream_port}",
            )
            await broker.start()

            def request(path, token, data=b"{}"):
                call = urllib.request.Request(
                    str(broker.base_url) + path,
                    data=data,
                    headers={
                        "Authorization": f"Bearer {token}",
                        "Content-Type": "application/json",
                    },
                    method="POST",
                )
                with urllib.request.urlopen(call, timeout=5) as response:
                    return response.status, json.loads(response.read())

            try:
                opened = await asyncio.to_thread(
                    request,
                    "/v1/memory/open",
                    broker.capability,
                )
                with self.assertRaises(urllib.error.HTTPError) as denied:
                    await asyncio.to_thread(
                        request,
                        "/v1/memory/checkpoint",
                        broker.capability,
                    )
                denied.exception.close()
                with self.assertRaises(urllib.error.HTTPError) as wrong_token:
                    await asyncio.to_thread(
                        request,
                        "/v1/memory/open",
                        "wrong-token",
                    )
                wrong_token.exception.close()
            finally:
                await broker.close()
                upstream_server.close()
                await upstream_server.wait_closed()
            return opened, denied.exception.code, upstream_requests, broker.records

        opened, denied_status, upstream, records = asyncio.run(scenario())
        self.assertEqual(opened[0], 200)
        self.assertEqual(denied_status, 403)
        self.assertEqual(len(upstream), 1)
        self.assertEqual(
            upstream[0][2]["authorization"],
            "Bearer real-service-token",
        )
        self.assertNotIn("real-service-token", json.dumps(records))
        self.assertEqual(records[0]["source_paths"], ["source.md"])
        self.assertEqual(records[1]["route_class"], "write")
        self.assertFalse(records[1]["allowed"])

    def test_codex_mcp_command_forwards_secret_by_name_not_value(self):
        eval_case = self.cases[0]
        command = codex_command(
            Job(eval_case, "codex", "mcp"),
            Path("/tmp/interface-run"),
            codex=Path("/tmp/codex"),
            model="gpt-5.6-sol",
            model_base_url="http://127.0.0.1:12345/backend-api/codex",
        )
        rendered = " ".join(command)
        self.assertIn("BRUNN_API_TOKEN", rendered)
        self.assertNotIn("secret-token", rendered)
        self.assertIn('model_reasoning_effort="xhigh"', command)
        self.assertIn(
            "--dangerously-bypass-approvals-and-sandbox",
            command,
        )
        self.assertNotIn("sandbox_workspace_write.network_access=true", command)
        self.assertIn(
            'mcp_servers.brunn-state.default_tools_approval_mode="approve"',
            command,
        )
        self.assertIn("BRUNN_STATE_MODEL_GATEWAY_TOKEN", rendered)
        self.assertIn(
            "model_providers.evaluation_gateway.requires_openai_auth=true",
            command,
        )

    def test_openclaw_direct_api_override_cannot_change_codex_runtime(self):
        eval_case = self.cases[0]
        job = Job(eval_case, "openclaw", "cli")
        metadata = {
            "authorization_scope": "scope:test",
            "token": "secret-token",
        }
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.dict(
                "os.environ",
                {"BRUNN_STATE_EVAL_DIRECT_OPENAI": "1"},
            ):
                config = openclaw_config(
                    job,
                    metadata,
                    root,
                    root,
                    "direct-api-test",
                    model="gpt-5.6-terra",
                )
        self.assertEqual(
            config["models"]["providers"]["openai"]["agentRuntime"]["id"],
            "codex",
        )
        self.assertEqual(
            config["agents"]["defaults"]["models"][
                "openai/gpt-5.6-terra"
            ]["agentRuntime"]["id"],
            "codex",
        )
        with self.assertRaisesRegex(ValueError, "Direct OpenAI reasoning is forbidden"):
            parent_model_credentials(direct_openai=True)

    def test_openclaw_parent_gateway_has_a_non_openai_provider_identity(self):
        eval_case = self.cases[0]
        job = Job(eval_case, "openclaw", "cli")
        metadata = {
            "authorization_scope": "scope:test",
            "token": "secret-token",
        }
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = openclaw_config(
                job,
                metadata,
                root,
                root,
                "gateway-test",
                model_base_url="http://127.0.0.1:12345/v1",
                model_gateway_token="ephemeral-model-capability",
                model="gpt-5.6-terra",
            )
        providers = config["models"]["providers"]
        self.assertIn("evaluation_gateway", providers)
        self.assertNotIn("openai", providers)
        self.assertEqual(
            config["agents"]["defaults"]["model"]["primary"],
            "evaluation_gateway/gpt-5.6-terra",
        )
        self.assertEqual(
            providers["evaluation_gateway"]["agentRuntime"]["id"],
            "codex",
        )
        self.assertEqual(
            providers["evaluation_gateway"]["apiKey"],
            "ephemeral-model-capability",
        )

    def test_prepare_openclaw_never_persists_direct_api_credentials(self):
        eval_case = self.cases[0]
        job = Job(eval_case, "openclaw", "cli")
        metadata = {
            "authorization_scope": "scope:test",
            "token": "secret-token",
        }
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            run_dir = root / "run"
            run_dir.mkdir()
            with patch.dict(
                "os.environ",
                {
                    "BRUNN_STATE_EVAL_DIRECT_OPENAI": "1",
                    "OPENAI_API_KEY": "must-not-be-persisted",
                },
            ):
                state_dir, config_path = prepare_openclaw(
                    job,
                    metadata,
                    run_dir,
                    "direct-api-test",
                )
            forbidden_auth_path = (
                state_dir
                / "agents"
                / "main"
                / "agent"
                / "auth-profiles.json"
            )
            rendered = config_path.read_text(encoding="utf-8")
            config = json.loads(rendered)
        self.assertFalse(forbidden_auth_path.exists())
        self.assertNotIn("must-not-be-persisted", rendered)
        self.assertEqual(
            config["models"]["providers"]["openai"]["agentRuntime"]["id"],
            "codex",
        )

    def test_person_resolution_grader_accepts_equivalent_matching_language(self):
        eval_case = next(
            case
            for case in self.cases
            if case.qualified_id == "personal:coord-person-resolution"
        )
        answer = {
            "answer": "",
            "claims": [
                {
                    "id": "c1",
                    "value": (
                        "Keep person:p-101 and person:p-102 distinct; "
                        "source:club-intake-r1 and source:volunteer-roster-r1 "
                        "remain source-bearing claims."
                    ),
                    "source_paths": ["Identity/source-identity-claims.csv"],
                },
                {
                    "id": "c2",
                    "value": (
                        "Both have the name Avery Lin and the email "
                        "avery.lin@example.com, but this did not trigger a merge."
                    ),
                    "source_paths": ["Identity/resolution-review.md"],
                },
                {
                    "id": "c3",
                    "value": (
                        "The possibly_same_as edge is unreviewed ambiguous "
                        "candidate linkage."
                    ),
                    "source_paths": ["Identity/resolution-review.md"],
                },
                {
                    "id": "c4",
                    "value": (
                        "person:p-103 had a same_as relation confirmed after "
                        "explicit owner review."
                    ),
                    "source_paths": ["Identity/resolution-review.md"],
                },
            ],
            "checkpoint": {
                "objective": "Resolve identities.",
                "current_state": ["Distinct people retained."],
                "next_actions": ["Preserve provenance."],
                "artifacts": ["Identity/resolution-review.md"],
            },
        }
        grade = grade_answer(
            eval_case.case,
            answer,
            set(eval_case.corpus_paths),
        )
        self.assertTrue(grade["pass"])

    def test_deadline_readiness_accepts_explicit_pending_and_incomplete_wording(self):
        eval_case = next(
            case
            for case in self.cases
            if case.qualified_id == "personal:coord-deadline-readiness"
        )
        answer = {
            "answer": "",
            "claims": [
                {
                    "id": "c1",
                    "value": (
                        "work-item:readiness-601 belongs to person:p-601 and is due "
                        "2026-08-14T17:00:00-07:00 in America/Los_Angeles."
                    ),
                    "source_paths": ["Work/readiness-plan.md"],
                },
                {
                    "id": "c2",
                    "value": (
                        "The prerequisites are work-item:guardian-consent-602, "
                        "work-item:equipment-inspection-603, work-item:registration-604, "
                        "and official schedule revision r2."
                    ),
                    "source_paths": [
                        "Work/readiness-plan.md",
                        "Work/completion-evidence.json",
                        "Events/schedule-snapshot-r2.md",
                    ],
                },
                {
                    "id": "c3",
                    "value": (
                        "gate:field-lab-ready is blocked because the equipment "
                        "inspection is pending."
                    ),
                    "source_paths": [
                        "Work/readiness-plan.md",
                        "Work/completion-evidence.json",
                    ],
                },
                {
                    "id": "c4",
                    "value": (
                        "Completed evidence includes artifact:guardian-consent-602 "
                        "and artifact:registration-ledger-604. The inspection remains "
                        "pending, and the packet itself remains incomplete."
                    ),
                    "source_paths": [
                        "Work/completion-evidence.json",
                        "Work/readiness-plan.md",
                    ],
                },
            ],
            "checkpoint": {
                "objective": "Assess readiness.",
                "current_state": ["The gate is blocked."],
                "next_actions": ["Complete the inspection."],
                "artifacts": ["Work/readiness-plan.md"],
            },
        }
        grade = grade_answer(
            eval_case.case,
            answer,
            set(eval_case.corpus_paths),
        )
        self.assertTrue(grade["pass"])

    def test_transition_grader_does_not_borrow_from_adjacent_claims(self):
        case = {
            "rubric": [
                {
                    "id": "c1",
                    "checks": [{"any": ["alpha"]}],
                    "sources_any": ["alpha.md"],
                },
                {
                    "id": "c2",
                    "checks": [{"any": ["beta"]}],
                    "sources_any": ["beta.md"],
                },
            ],
        }
        answer = {
            "answer": "",
            "claims": [
                {
                    "id": "c1",
                    "value": "alpha and beta",
                    "source_paths": ["alpha.md"],
                },
                {
                    "id": "c2",
                    "value": "missing its required detail",
                    "source_paths": ["beta.md"],
                },
            ],
            "checkpoint": {
                "objective": "Test.",
                "current_state": ["Test."],
                "next_actions": ["Test."],
                "artifacts": ["alpha.md", "beta.md"],
            },
        }
        grade = grade_transition_answer(
            case,
            answer,
            {"alpha.md", "beta.md"},
        )
        self.assertFalse(grade["pass"])
        self.assertFalse(grade["claims"][1]["pass"])

    def test_openclaw_runtime_state_is_discarded_unless_explicitly_kept(self):
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            state_dir = run_dir / ".openclaw-state"
            nested = state_dir / "agents" / "main"
            nested.mkdir(parents=True)
            (nested / "state.json").write_text("{}\n", encoding="utf-8")
            cleanup_openclaw_state(run_dir)
            self.assertFalse(state_dir.exists())

            state_dir.mkdir()
            with patch.dict(
                "os.environ",
                {"BRUNN_STATE_EVAL_KEEP_AGENT_STATE": "1"},
            ):
                cleanup_openclaw_state(run_dir)
            self.assertTrue(state_dir.is_dir())

    def test_mcp_trace_normalizes_to_shared_service_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            operations = [
                {
                    "operation": "memory.open",
                    "http_status": 200,
                    "elapsed_ms": 12.5,
                    "result_chars": 100,
                    "source_text_chars": 75,
                    "metadata_chars": 25,
                    "session_id": "session:one",
                    "corpus_revision": "revision:one",
                },
                {
                    "operation": "memory.checkpoint",
                    "http_status": 201,
                    "elapsed_ms": 3.5,
                    "result_chars": 30,
                    "source_text_chars": 0,
                    "metadata_chars": 30,
                    "checkpoint_id": "checkpoint:child",
                },
            ]
            (root / "mcp-trace.jsonl").write_text(
                "\n".join(json.dumps(item) for item in operations) + "\n",
                encoding="utf-8",
            )
            normalize_mcp_trace(root)
            state = json.loads(
                (root / "native-session.json").read_text(encoding="utf-8")
            )
            self.assertEqual(state["session_id"], "session:one")
            self.assertEqual(state["corpus_revision"], "revision:one")
            self.assertEqual(state["checkpoint_id"], "checkpoint:child")
            self.assertEqual(len(state["operations"]), 2)

    def test_provisioning_state_is_private_and_run_scoped(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / ".interface-provisioning.json"
            jobs = {"job": {"token": "secret-token"}}
            save_provisioning(path, "run-one", jobs)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(load_provisioning(path, "run-one"), jobs)
            with self.assertRaises(ValueError):
                load_provisioning(path, "run-two")

    def test_resumed_record_uses_keyed_parent_custody_not_public_run_copy(self):
        job = Job(self.cases[0], "codex", "filesystem")
        with tempfile.TemporaryDirectory() as temporary:
            run_root = Path(temporary)
            run_dir = job.run_dir(run_root)
            run_dir.mkdir(parents=True)
            record_path = run_dir / "record.json"
            record_path.write_text(
                json.dumps({"run_id": "sealed-run", "job_key": job.key}),
                encoding="utf-8",
            )
            record_path.chmod(0o400)
            write_record_seal(
                run_root,
                job,
                run_id="sealed-run",
                record_path=record_path,
            )
            loaded = load_existing_record(
                job,
                {"token": ""},
                run_id="sealed-run",
                run_root=run_root,
                run_dir=run_dir,
            )
            self.assertEqual(loaded["job_key"], job.key)

            # A same-user child may rewrite the diagnostic run copy after exit.
            # Resume authority stays with the parent-custodied canonical record.
            record_path.chmod(0o600)
            record_path.write_text(
                json.dumps({
                    "run_id": "sealed-run",
                    "job_key": job.key,
                    "forged": True,
                }),
                encoding="utf-8",
            )
            record_path.chmod(0o400)
            loaded = load_existing_record(
                job,
                {"token": ""},
                run_id="sealed-run",
                run_root=run_root,
                run_dir=run_dir,
            )
            self.assertNotIn("forged", loaded)

            canonical = canonical_record_path(run_root, job)
            canonical.chmod(0o600)
            canonical.write_text(
                json.dumps({
                    "run_id": "sealed-run",
                    "job_key": job.key,
                    "forged": True,
                }),
                encoding="utf-8",
            )
            canonical.chmod(0o400)
            seal_path = record_seal_path(run_root, job)
            seal = json.loads(seal_path.read_text(encoding="utf-8"))
            seal["record_sha256"] = (
                "sha256:" + hashlib.sha256(canonical.read_bytes()).hexdigest()
            )
            seal_path.chmod(0o600)
            seal_path.write_text(json.dumps(seal), encoding="utf-8")
            seal_path.chmod(0o400)
            with self.assertRaisesRegex(ValueError, "seal mismatch"):
                load_existing_record(
                    job,
                    {"token": ""},
                    run_id="sealed-run",
                    run_root=run_root,
                    run_dir=run_dir,
                )

    def test_summary_counts_failed_runs_in_quality_denominators(self):
        rows = [
            {
                "expected_claims": 4,
                "exit_code": 0,
                "timed_out": False,
                "grade": {
                    "claims_passed": 4,
                    "score": 1.0,
                },
                "answer_pass": True,
                "workflow_pass": True,
                "events": {
                    "tokens": {
                        "input_tokens": 100,
                        "cached_input_tokens": 20,
                        "output_tokens": 10,
                    },
                },
            },
            {
                "expected_claims": 4,
                "exit_code": 1,
                "timed_out": False,
                "grade": None,
                "answer_pass": False,
                "workflow_pass": False,
                "events": {"tokens": {}},
            },
        ]
        summary = group_summary(rows)
        self.assertEqual(summary["claims_passed"], 4)
        self.assertEqual(summary["claims_total"], 8)
        self.assertEqual(summary["evaluated_claims_total"], 4)
        self.assertEqual(summary["evaluated_claim_pass_rate"], 1.0)
        self.assertEqual(summary["process_failures"], 1)
        self.assertEqual(summary["evaluated_workflow_case_pass_rate"], 1.0)
        self.assertEqual(summary["workflow_cases_passed"], 1)
        self.assertEqual(summary["tokens_mean"]["uncached_input_tokens"], 40.0)
        self.assertEqual(summary["tokens_median"]["input_tokens"], 50.0)
        self.assertEqual(summary["tokens_p95"]["input_tokens"], 100.0)
        self.assertEqual(summary["median_service_calls"], 0)

    def test_paired_comparison_uses_same_case_only(self):
        def row(interface, case_id, passed, score, tokens):
            return {
                "agent": "codex",
                "interface": interface,
                "qualified_case_id": case_id,
                "workflow_pass": passed,
                "grade": {"score": score},
                "events": {
                    "tokens": {
                        "input_tokens": tokens,
                        "cached_input_tokens": 0,
                        "output_tokens": 10,
                    },
                },
            }

        records = [
            row("cli", "work:a", True, 1.0, 100),
            row("mcp", "work:a", False, 0.5, 120),
            row("cli", "work:b", True, 0.9, 80),
            row("mcp", "work:b", True, 1.0, 70),
        ]
        result = paired_comparisons(records)["codex:mcp-minus-cli"]
        self.assertEqual(result["paired_cases"], 2)
        self.assertEqual(result["workflow_wins"], 0)
        self.assertEqual(result["workflow_losses"], 1)
        self.assertEqual(result["mean_token_delta"]["input_tokens"], 5.0)

    def test_smoke_run_cannot_be_published_as_quality_evidence(self):
        run = {
            "protocol": {
                "agents": ["codex"],
                "interfaces": ["filesystem"],
                "suites": ["work"],
                "case_count": 1,
                "repetitions": 1,
                "fresh_agent_runs": 1,
            },
            "records": [{
                "interface": "filesystem",
                "service_operations": [],
            }],
            "integrity": {"pass": True},
            "summary": {"paired_comparisons": {}},
        }
        status = quality_publishability(
            run,
            all_case_count=len(self.cases),
            process_failures=0,
        )
        self.assertFalse(status["publishable_for_quality_comparison"])
        self.assertIn(
            "run is not the full predeclared matrix",
            status["blockers"],
        )
        self.assertIn(
            "deterministic keyword rubrics are screening evidence; blind semantic "
            "or human adjudication is required for a quality claim",
            status["blockers"],
        )
        self.assertIn(
            "one or more model request/response contracts is incomplete",
            status["blockers"],
        )
        self.assertIn(
            "parent-owned token accounting is incomplete",
            status["blockers"],
        )
        self.assertFalse(status["cross_agent_quality_comparable"])


if __name__ == "__main__":
    unittest.main()
