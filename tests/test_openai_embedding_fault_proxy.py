import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from eval.hook_provenance import (
    ATTESTATION_SCHEMA,
    hook_target_matches,
    parse_hook_attestation,
    run_hook,
)
from eval.openai_embedding_fault_proxy import (
    forwarded_headers,
    parse_upstream_usage,
    read_behavior,
    read_token,
    validate_behavior,
    validate_upstream_base_url,
    write_behavior,
)


class OpenAiEmbeddingFaultProxyTests(unittest.TestCase):
    def test_upstream_rejects_credentials_and_plain_http_by_default(self):
        self.assertEqual(
            validate_upstream_base_url(
                "https://api.openai.com/v1/",
                allow_http=False,
            ),
            "https://api.openai.com/v1",
        )
        with self.assertRaises(ValueError):
            validate_upstream_base_url(
                "https://token@api.openai.com/v1",
                allow_http=False,
            )
        with self.assertRaises(ValueError):
            validate_upstream_base_url(
                "http://127.0.0.1:9000/v1",
                allow_http=False,
            )

    def test_fault_modes_are_mutually_exclusive_and_fail_closed(self):
        self.assertEqual(
            validate_behavior("forward", 0, 0)["mode"],
            "forward",
        )
        self.assertEqual(
            validate_behavior("slow", 800, 0)["delay_ms"],
            800,
        )
        self.assertEqual(
            validate_behavior("error", 0, 503)["error_status"],
            503,
        )
        for invalid in (
            ("forward", 1, 0),
            ("slow", 0, 0),
            ("slow", 800, 503),
            ("error", 1, 503),
            ("error", 0, 0),
        ):
            with self.assertRaises(ValueError):
                validate_behavior(*invalid)

    def test_config_and_control_token_are_owner_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = root / "proxy.json"
            token = root / "control.token"
            token.write_text("x" * 64, encoding="ascii")
            token.chmod(0o600)
            write_behavior(
                config,
                instance_id="stack-one",
                mode="forward",
                delay_ms=0,
                error_status=0,
                revision=1,
            )
            self.assertEqual(read_behavior(config)["mode"], "forward")
            self.assertEqual(config.stat().st_mode & 0o777, 0o600)
            self.assertEqual(read_token(token), b"x" * 64)
            token.chmod(0o644)
            with self.assertRaises(PermissionError):
                read_token(token)

    def test_forwarded_headers_do_not_copy_control_or_transport_headers(self):
        headers = {
            "Authorization": "Bearer provider-secret",
            "Content-Type": "application/json",
            "X-Brunn-Control": "never-forward",
            "Cookie": "never-forward",
            "Host": "proxy",
        }
        forwarded = forwarded_headers(headers)
        self.assertEqual(
            set(key.casefold() for key in forwarded),
            {"authorization", "content-type"},
        )

    def test_usage_receipt_extracts_only_valid_provider_token_totals(self):
        self.assertEqual(
            parse_upstream_usage(json.dumps({
                "data": [{"embedding": [0.1]}],
                "usage": {"prompt_tokens": 41, "total_tokens": 41},
            }).encode()),
            (41, 41),
        )
        self.assertIsNone(parse_upstream_usage(b'{"data":[]}'))
        self.assertIsNone(parse_upstream_usage(json.dumps({
            "usage": {"prompt_tokens": True, "total_tokens": 1},
        }).encode()))

    def test_hook_attestation_drops_arbitrary_stdout_fields(self):
        attestation = {
            "schema": ATTESTATION_SCHEMA,
            "pass": True,
            "action": "configure",
            "mode": "error",
            "instance_id": "stack-one",
            "implementation_sha256": "a" * 64,
            "upstream_base_url_sha256": "b" * 64,
            "configuration_revision": 2,
            "control": "local",
            "authorization": "Bearer must-not-survive",
            "request_body": {"input": "must-not-survive"},
        }
        parsed = parse_hook_attestation(
            (json.dumps(attestation) + "\n").encode("utf-8")
        )
        self.assertIsNotNone(parsed)
        self.assertNotIn("authorization", parsed)
        self.assertNotIn("request_body", parsed)

    def test_required_hook_attestation_is_mode_bound(self):
        attestation = {
            "schema": ATTESTATION_SCHEMA,
            "pass": True,
            "action": "configure",
            "mode": "forward",
            "instance_id": "stack-one",
            "implementation_sha256": "a" * 64,
            "upstream_base_url_sha256": "b" * 64,
            "configuration_revision": 3,
            "control": "local",
        }

        class Completed:
            returncode = 0
            stdout = (json.dumps(attestation) + "\n").encode("utf-8")

        with patch(
            "eval.hook_provenance.subprocess.run",
            return_value=Completed(),
        ):
            passed = run_hook(
                "/usr/bin/true",
                timeout_seconds=1,
                require_attestation=True,
                expected_mode="forward",
            )
            failed = run_hook(
                "/usr/bin/true",
                timeout_seconds=1,
                require_attestation=True,
                expected_mode="error",
            )
        self.assertTrue(passed["pass"])
        self.assertFalse(failed["pass"])
        self.assertEqual(
            failed["error"],
            "missing_or_mismatched_hook_attestation",
        )

    def test_restore_must_bind_the_same_proxy_and_newer_revision(self):
        injected = {
            "pass": True,
            "attestation": {
                "instance_id": "stack-one",
                "implementation_sha256": "a" * 64,
                "upstream_base_url_sha256": "b" * 64,
                "configuration_revision": 2,
            },
        }
        restored = {
            "pass": True,
            "attestation": {
                **injected["attestation"],
                "configuration_revision": 3,
            },
        }
        self.assertTrue(hook_target_matches(injected, restored))
        restored["attestation"]["instance_id"] = "stack-two"
        self.assertFalse(hook_target_matches(injected, restored))
        restored["attestation"]["instance_id"] = "stack-one"
        restored["attestation"]["configuration_revision"] = 2
        self.assertFalse(hook_target_matches(injected, restored))


if __name__ == "__main__":
    unittest.main()
