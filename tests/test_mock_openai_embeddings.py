import math
import tempfile
import unittest
from pathlib import Path

from tests.mock_openai_embeddings import (
    embedding,
    read_behavior,
    validate_behavior,
    write_behavior,
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


if __name__ == "__main__":
    unittest.main()
