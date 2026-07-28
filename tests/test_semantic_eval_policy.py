import unittest

from semantic_eval_policy import (
    enforced_retrieval_modes,
    response_has_candidates,
    semantic_counter_delta,
    semantic_rates,
    validate_e09_runtime,
)


class SemanticEvalPolicyTests(unittest.TestCase):
    def test_coverage_requires_an_actual_candidate(self):
        self.assertTrue(response_has_candidates({
            "data": {
                "results": [{
                    "candidates": [{"entry_ref": "entry:1"}],
                }],
            },
        }))
        self.assertFalse(response_has_candidates({
            "data": {"results": [{"candidates": []}]},
        }))

    def test_no_semantic_arm_requires_server_kill_switch_and_request_modes(self):
        result = validate_e09_runtime(
            {
                "build_revision": "abc123",
                "runtime_features": {
                    "semantic_lane": False,
                    "embed_cache": True,
                    "semantic_deadline_ms": 300,
                    "embedding_backfill_guard": True,
                },
            },
            "no_semantic",
        )
        self.assertEqual(
            result["enforced_request_modes"],
            ["exact", "lexical"],
        )
        self.assertEqual(
            enforced_retrieval_modes("no_semantic"),
            ("exact", "lexical"),
        )

    def test_semantic_arms_fail_closed_on_flag_drift(self):
        with self.assertRaisesRegex(ValueError, "runtime flag mismatch"):
            validate_e09_runtime(
                {
                    "build_revision": "abc123",
                    "runtime_features": {
                        "semantic_lane": True,
                        "embed_cache": True,
                        "semantic_deadline_ms": 600,
                        "embedding_backfill_guard": True,
                    },
                },
                "deadline_cache",
            )

    def test_authorized_step_arm_has_a_distinct_600ms_runtime_identity(self):
        status = {
            "build_revision": "abc123",
            "runtime_features": {
                "semantic_lane": True,
                "embed_cache": True,
                "semantic_deadline_ms": 600,
                "embedding_backfill_guard": True,
            },
        }
        with self.assertRaisesRegex(ValueError, "checked step-policy"):
            validate_e09_runtime(status, "deadline_cache_600")
        binding = {
            "schema": "straylight-e09-step-authorization-binding@v1",
            "authorization_id": "a" * 64,
            "artifact_sha256": "b" * 64,
            "deadline_before_ms": 300,
            "deadline_after_ms": 600,
            "automatic_1000ms_step_allowed": False,
        }
        result = validate_e09_runtime(
            status,
            "deadline_cache_600",
            step_authorization=binding,
        )
        self.assertEqual(result["arm"], "deadline_cache_600")
        self.assertEqual(result["step_authorization_id"], "a" * 64)
        self.assertEqual(
            result["expected_features"]["semantic_deadline_ms"],
            600,
        )

    def test_counter_delta_and_rates_are_exact(self):
        before = {
            "requested": 10,
            "cache_hits": 2,
            "cache_misses": 8,
            "deferrals": 1,
        }
        after = {
            "requested": 20,
            "cache_hits": 7,
            "cache_misses": 13,
            "deferrals": 3,
        }
        delta = semantic_counter_delta(before, after)
        self.assertEqual(delta["requested"], 10)
        self.assertEqual(delta["cache_hits"], 5)
        self.assertEqual(delta["cache_misses"], 5)
        self.assertEqual(
            semantic_rates(delta),
            {"cache_hit_rate": 0.5, "deferral_rate": 0.2},
        )

    def test_counter_reset_invalidates_the_measurement(self):
        with self.assertRaisesRegex(ValueError, "moved backwards"):
            semantic_counter_delta(
                {"requested": 2},
                {"requested": 1},
            )


if __name__ == "__main__":
    unittest.main()
