import math
import unittest

from tests.mock_openai_embeddings import embedding


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


if __name__ == "__main__":
    unittest.main()
