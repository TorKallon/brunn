import math
import os
import tempfile
import unittest
from pathlib import Path

from tests.mock_openai_embeddings import (
    HEALTH_SCHEMA,
    STATE_SCHEMA,
    config_path_sha256,
    embedding,
    identity_matches,
    implementation_sha256,
    read_behavior,
    read_state,
    validate_expected_state,
    validate_behavior,
    write_behavior,
    write_state,
)


class MockOpenAiEmbeddingsTest(unittest.TestCase):
    def test_embeddings_are_stable_normalized_and_semantically_shared(self) -> None:
        first = embedding("terminal-corpus-100-current-answer cobalt", 64)
        replay = embedding("terminal-corpus-100-current-answer cobalt", 64)
        related = embedding(
            "Find terminal-corpus-100-current-answer without a path",
            64,
        )
        unrelated = embedding("weather forecast and ski wax", 64)

        self.assertEqual(first, replay)
        self.assertAlmostEqual(
            math.sqrt(sum(value * value for value in first)),
            1.0,
        )
        related_score = sum(left * right for left, right in zip(first, related))
        unrelated_score = sum(
            left * right for left, right in zip(first, unrelated)
        )
        self.assertGreater(related_score, unrelated_score)

    def test_runtime_behavior_is_machine_readable_and_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mock.json"
            write_behavior(path, 800, 0)
            self.assertEqual(
                read_behavior(path),
                {"delay_ms": 800, "error_status": 0},
            )
            path.write_text("{broken", encoding="utf-8")
            self.assertEqual(
                read_behavior(path),
                {"delay_ms": 0, "error_status": 503},
            )

    def test_invalid_failure_configuration_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            validate_behavior(-1, 0)
        with self.assertRaises(ValueError):
            validate_behavior(0, 302)

    def test_health_identity_rejects_generic_or_foreign_mock(self) -> None:
        state = {
            "instance_id": "mode2-owned",
            "implementation_sha256": implementation_sha256(),
            "host": "0.0.0.0",
            "port": 55200,
            "config_path_sha256": "a" * 64,
            "pid": 1234,
        }
        observed = {
            "schema": HEALTH_SCHEMA,
            "status": "ok",
            "behavior": {"delay_ms": 0, "error_status": 0},
            "identity": dict(state),
        }
        self.assertTrue(identity_matches(observed, state))
        self.assertFalse(identity_matches({
            "status": "ok",
            "behavior": {"delay_ms": 0, "error_status": 0},
        }, state))
        observed["identity"]["instance_id"] = "mode2-foreign"
        self.assertFalse(identity_matches(observed, state))

    def test_structured_state_is_private_and_binding_is_exact(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "mock.state"
            config = root / "mock.config"
            state = {
                "schema": STATE_SCHEMA,
                "pid": 1234,
                "instance_id": "mode2-owned",
                "implementation_sha256": implementation_sha256(),
                "host": "0.0.0.0",
                "port": 55200,
                "config_path_sha256": config_path_sha256(config),
                "started_at": "2026-07-27T00:00:00-07:00",
            }
            write_state(path, state)
            self.assertEqual(read_state(path), state)
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)
            validate_expected_state(
                state,
                instance_id="mode2-owned",
                expect_implementation_sha256=implementation_sha256(),
                port=55200,
                config=config,
            )
            with self.assertRaisesRegex(ValueError, "binding mismatch"):
                validate_expected_state(
                    state,
                    instance_id="mode2-foreign",
                    expect_implementation_sha256=implementation_sha256(),
                    port=55200,
                    config=config,
                )


if __name__ == "__main__":
    unittest.main()
