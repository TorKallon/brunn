from __future__ import annotations

import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from eval.compare_query_counts import compare_query_count_artifacts, main


REVISION = "a" * 40
IMAGE_ID = "sha256:" + "b" * 64
FEATURE = "lexical_single_scan"
EXPECTED_MODES = ("exact", "lexical")
SCALE = 640_000
SAMPLES = 2
CONCURRENT_ROUNDS = 2
CONCURRENT_SEARCHES = 5
SAMPLE_OPERATIONS = {
    "open": "open",
    "search": "search",
    "broad_search": "search",
    "bounded_overflow_search": "search",
    "old_source_search": "search",
    "verbatim_identifier_search": "search",
    "concurrent_search": "search",
    "read": "read",
    "max_batch_read": "read",
    "write": "write",
    "checkpoint": "checkpoint",
    "max_checkpoint_sources": "checkpoint",
    "resume": "resume",
}


def expected_catalog() -> dict[str, int]:
    return dict(sorted({
        "open": SAMPLES,
        "search": SAMPLES,
        "broad_search": SAMPLES,
        "bounded_overflow_search": SAMPLES,
        "old_source_search": SAMPLES,
        "verbatim_identifier_search": 3,
        "concurrent_search": (
            CONCURRENT_ROUNDS * CONCURRENT_SEARCHES
        ),
        "read": SAMPLES,
        "max_batch_read": 1,
        "write": CONCURRENT_ROUNDS,
        "checkpoint": 1,
        "max_checkpoint_sources": 1,
        "resume": SAMPLES,
    }.items()))


def count_summary(
    operation: str,
    values: list[int],
    expected_samples: int,
) -> dict:
    return {
        "operation": operation,
        "expected_samples": expected_samples,
        "observed_samples": expected_samples,
        "missing_query_counts": 0,
        "samples": len(values),
        "min": min(values) if values else None,
        "max": max(values) if values else None,
        "counts": values,
    }


def artifact(
    *,
    feature_state: bool,
    search_counts: list[int],
    read_counts: list[int] | None = None,
) -> dict:
    features = {
        "lexical_single_scan": feature_state,
        "semantic_lane": False,
        "verbatim_spans": False,
    }
    runtime = {
        "schema": "brunn-service-runtime-snapshot@v1",
        "status": "ready",
        "build_revision": REVISION,
        "read_only": False,
        "runtime_features": features,
        "embeddings": {
            "provider": "hashing",
            "model": "hashing",
            "dimensions": 32,
            "status": "ready",
        },
    }
    read_values = read_counts or [11, 11]
    catalog = expected_catalog()
    values_by_name = {
        sample_name: [17] * cardinality
        for sample_name, cardinality in catalog.items()
    }
    values_by_name["search"] = search_counts
    values_by_name["read"] = read_values
    by_sample_name = {
        sample_name: count_summary(
            SAMPLE_OPERATIONS[sample_name],
            values,
            catalog[sample_name],
        )
        for sample_name, values in values_by_name.items()
    }
    by_operation: dict[str, list[int]] = {}
    for sample_name, values in values_by_name.items():
        by_operation.setdefault(
            SAMPLE_OPERATIONS[sample_name],
            [],
        ).extend(values)
    return {
        "schema": "brunn-performance-eval@v2",
        "protocol": "simple",
        "pass": True,
        "errors": [],
        "retrieval_modes": ["exact", "lexical"],
        "gate_profile": "e05-lexical-consolidation",
        "semantic_failure_probe_posture": "not-applicable",
        "verbatim_feature_acceptance_posture": {
            "posture": "not-applicable",
            "eligible": True,
            "reason": "verbatim_spans is an explicitly disabled nuisance feature",
            "observed": {"verbatim_spans": False},
        },
        "production_reference_records": 64_000,
        "future_reference_records": 640_000,
        "query_budget_contract": None,
        "expected_build_revision": REVISION,
        "expected_runtime_features": features,
        "runtime_configuration": {
            "before": copy.deepcopy(runtime),
            "after": copy.deepcopy(runtime),
            "stable": True,
        },
        "implementation_fingerprint": {
            "source_revision": REVISION,
            "tracked_source_clean": True,
            "untracked_source_files": [],
            "api_image_id": IMAGE_ID,
            "api_image_revision": REVISION,
            "reproducible": True,
        },
        "run_profile": {
            "mode": "definitive",
            "definitive": True,
            "samples_per_retrieval": SAMPLES,
            "requested_scales": [SCALE],
            "required_scales": [SCALE],
            "future_soak": {
                "requested": True,
                "records": SCALE,
                "status": "complete",
            },
            "semantic_failure_probe": "not-applicable",
            "semantic_failure_hooks_configured": False,
            "semantic_failure_hook_attestation_required": False,
            "wait_for_semantic": False,
            "unique_queries": False,
            "import_timeout_seconds": 7_200.0,
        },
        "scales": [{
            "scale": SCALE,
            "protocol": "simple",
            "documents": SCALE,
            "target_path": "Synthetic/records/0639999.md",
            "marker": "PERF-MARKER-640000",
            "samples": SAMPLES,
            "discovery_query": "perf-discovery-key-640000",
            "discovery_task": "Find the terminal synthetic corpus answer.",
            "discovery_path_was_provided": False,
            "fixture_manifest": {
                "schema": "brunn-synthetic-fixture@v2",
                "scale": SCALE,
                "verbatim_identifiers": [
                    {
                        "path": f"Synthetic/records/{index:07d}.md",
                        "identifier": f"STRAYID-640000-{index}-deadbeef",
                        "byte_offset": 2_500,
                    }
                    for index in range(3)
                ],
            },
            "concurrent_probe": {
                "rounds": CONCURRENT_ROUNDS,
                "searches_per_round": CONCURRENT_SEARCHES,
            },
            "query_counts": {
                "by_operation": {
                    operation: {
                        "samples": len(values),
                        "min": min(values),
                        "max": max(values),
                        "counts": values,
                    }
                    for operation, values in by_operation.items()
                },
                "by_sample_name": by_sample_name,
                "sample_cardinality": {
                    "schema": (
                        "brunn-query-count-sample-cardinality@v1"
                    ),
                    "authoritative": True,
                    "expected_by_sample_name": dict(catalog),
                    "observed_by_sample_name": dict(catalog),
                    "counted_by_sample_name": dict(catalog),
                    "missing_response_samples": {},
                    "extra_response_samples": {},
                    "missing_query_count_samples": {},
                    "pass": True,
                },
                "missing_by_operation": {},
                "missing_by_sample_name": {},
            },
        }],
    }


class PairedQueryCountComparisonTests(unittest.TestCase):
    def test_accepts_hydration_transaction_deltas_of_zero_or_five(self):
        result = compare_query_count_artifacts(
            artifact(feature_state=False, search_counts=[20, 20]),
            artifact(feature_state=True, search_counts=[25, 20]),
            feature=FEATURE,
            operation="search",
            min_delta=0,
            max_delta=5,
            expected_retrieval_modes=EXPECTED_MODES,
            require_strict_increase=True,
            allowed_deltas=[0, 5],
        )

        self.assertTrue(result["pass"])
        search = next(
            item
            for item in result["comparisons"]
            if item["sample_name"] == "search"
        )
        self.assertEqual(search["paired_deltas"], [5, 0])
        self.assertEqual(
            result["query_count_contract"]["allowed_deltas"],
            [0, 5],
        )
        self.assertEqual(
            result["sample_catalogs"][0]["expected_by_sample_name"],
            expected_catalog(),
        )

    def test_accepts_nonincrease_with_a_strict_reduction(self):
        result = compare_query_count_artifacts(
            artifact(feature_state=False, search_counts=[20, 20]),
            artifact(feature_state=True, search_counts=[18, 20]),
            feature=FEATURE,
            operation="search",
            min_delta=-2,
            max_delta=0,
            expected_retrieval_modes=EXPECTED_MODES,
            require_strict_improvement=True,
        )

        self.assertTrue(result["pass"])
        search = next(
            item
            for item in result["comparisons"]
            if item["sample_name"] == "search"
        )
        self.assertEqual(search["paired_deltas"], [-2, 0])

    def test_rejects_unstructured_or_absent_strict_increase(self):
        control = artifact(feature_state=False, search_counts=[20, 20])
        with self.assertRaisesRegex(ValueError, "allowed set"):
            compare_query_count_artifacts(
                control,
                artifact(feature_state=True, search_counts=[23, 20]),
                feature=FEATURE,
                operation="search",
                min_delta=0,
                max_delta=5,
                expected_retrieval_modes=EXPECTED_MODES,
                require_strict_increase=True,
                allowed_deltas=[0, 5],
            )
        with self.assertRaisesRegex(ValueError, "strict increase"):
            compare_query_count_artifacts(
                control,
                artifact(feature_state=True, search_counts=[20, 20]),
                feature=FEATURE,
                operation="search",
                min_delta=0,
                max_delta=5,
                expected_retrieval_modes=EXPECTED_MODES,
                require_strict_increase=True,
                allowed_deltas=[0, 5],
            )

    def test_rejects_dirty_unpaired_missing_or_out_of_range_inputs(self):
        control = artifact(feature_state=False, search_counts=[20, 20])
        treatment = artifact(feature_state=True, search_counts=[18, 20])

        dirty = copy.deepcopy(control)
        dirty["implementation_fingerprint"]["reproducible"] = False
        with self.assertRaisesRegex(ValueError, "clean source/build/image"):
            compare_query_count_artifacts(
                dirty,
                treatment,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

        missing = copy.deepcopy(control)
        missing["scales"][0]["query_counts"]["missing_by_operation"] = {
            "search": 1,
        }
        with self.assertRaisesRegex(ValueError, "missing query counts"):
            compare_query_count_artifacts(
                missing,
                treatment,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

        unpaired = copy.deepcopy(treatment)
        unpaired["scales"][0]["query_counts"]["by_sample_name"][
            "search"
        ]["counts"] = [18]
        unpaired["scales"][0]["query_counts"]["by_sample_name"][
            "search"
        ]["samples"] = 1
        unpaired["scales"][0]["query_counts"]["by_sample_name"][
            "search"
        ]["min"] = 18
        unpaired["scales"][0]["query_counts"]["by_sample_name"][
            "search"
        ]["max"] = 18
        with self.assertRaisesRegex(ValueError, "malformed"):
            compare_query_count_artifacts(
                control,
                unpaired,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

        out_of_range = artifact(
            feature_state=True,
            search_counts=[17, 20],
        )
        with self.assertRaisesRegex(ValueError, "outside"):
            compare_query_count_artifacts(
                control,
                out_of_range,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

        no_improvement = artifact(
            feature_state=True,
            search_counts=[20, 20],
        )
        with self.assertRaisesRegex(ValueError, "strict improvement"):
            compare_query_count_artifacts(
                control,
                no_improvement,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
                require_strict_improvement=True,
            )

    def test_rejects_wrong_modes_profile_runtime_and_query_shape(self):
        control = artifact(feature_state=False, search_counts=[20, 20])
        treatment = artifact(feature_state=True, search_counts=[18, 20])

        missing_posture = copy.deepcopy(control)
        missing_posture.pop("verbatim_feature_acceptance_posture")
        with self.assertRaisesRegex(
            ValueError,
            "lacks verbatim feature-acceptance posture",
        ):
            compare_query_count_artifacts(
                missing_posture,
                treatment,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

        wrong_modes = copy.deepcopy(control)
        wrong_modes["retrieval_modes"] = ["lexical"]
        with self.assertRaisesRegex(ValueError, "explicit expected modes"):
            compare_query_count_artifacts(
                wrong_modes,
                treatment,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

        profile_mismatch = copy.deepcopy(treatment)
        profile_mismatch["run_profile"]["unique_queries"] = True
        with self.assertRaisesRegex(ValueError, "run profiles differ"):
            compare_query_count_artifacts(
                control,
                profile_mismatch,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

        posture_mismatch = copy.deepcopy(treatment)
        posture_mismatch["verbatim_feature_acceptance_posture"] = {
            "posture": "required",
            "eligible": True,
            "reason": "feature acceptance required",
            "observed": {"verbatim_spans": False},
        }
        with self.assertRaisesRegex(
            ValueError,
            "verbatim_feature_acceptance_posture differs",
        ):
            compare_query_count_artifacts(
                control,
                posture_mismatch,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

        runtime_mismatch = copy.deepcopy(treatment)
        runtime_mismatch["runtime_configuration"]["before"]["read_only"] = True
        runtime_mismatch["runtime_configuration"]["after"]["read_only"] = True
        with self.assertRaisesRegex(ValueError, "beyond the feature"):
            compare_query_count_artifacts(
                control,
                runtime_mismatch,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

        query_mismatch = copy.deepcopy(treatment)
        query_mismatch["scales"][0]["discovery_query"] = "different query"
        with self.assertRaisesRegex(ValueError, "fixture shape differs"):
            compare_query_count_artifacts(
                control,
                query_mismatch,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

    def test_rejects_identically_incomplete_or_inexact_sample_catalogs(self):
        control = artifact(feature_state=False, search_counts=[20, 20])
        treatment = artifact(feature_state=True, search_counts=[18, 20])
        for candidate in (control, treatment):
            counts = candidate["scales"][0]["query_counts"]
            counts["by_sample_name"].pop("max_batch_read")
            cardinality = counts["sample_cardinality"]
            for field in (
                "expected_by_sample_name",
                "observed_by_sample_name",
                "counted_by_sample_name",
            ):
                cardinality[field].pop("max_batch_read")
        with self.assertRaisesRegex(ValueError, "authoritative and exact"):
            compare_query_count_artifacts(
                control,
                treatment,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

        control = artifact(feature_state=False, search_counts=[20, 20])
        treatment = artifact(feature_state=True, search_counts=[18, 20])
        treatment["scales"][0]["query_counts"]["by_sample_name"][
            "search"
        ]["observed_samples"] = 1
        with self.assertRaisesRegex(ValueError, "malformed"):
            compare_query_count_artifacts(
                control,
                treatment,
                feature=FEATURE,
                operation="search",
                min_delta=-2,
                max_delta=0,
                expected_retrieval_modes=EXPECTED_MODES,
            )

    def test_failure_artifact_hashes_both_readable_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            control_path = root / "control.json"
            treatment_path = root / "treatment.json"
            output_path = root / "comparison.json"
            control_path.write_text("{}\n", encoding="utf-8")
            treatment_path.write_text(
                json.dumps(
                    artifact(feature_state=True, search_counts=[18, 20])
                )
                + "\n",
                encoding="utf-8",
            )

            exit_code = main([
                "--control",
                str(control_path),
                "--treatment",
                str(treatment_path),
                "--feature",
                FEATURE,
                "--operation",
                "search",
                "--expected-retrieval-modes",
                "exact",
                "lexical",
                "--min-delta",
                "-2",
                "--max-delta",
                "0",
                "--out",
                str(output_path),
            ])

            self.assertEqual(exit_code, 2)
            result = json.loads(output_path.read_text(encoding="utf-8"))
            self.assertFalse(result["pass"])
            self.assertEqual(
                result["inputs"]["control"]["sha256"],
                hashlib.sha256(control_path.read_bytes()).hexdigest(),
            )
            self.assertEqual(
                result["inputs"]["treatment"]["sha256"],
                hashlib.sha256(treatment_path.read_bytes()).hexdigest(),
            )


if __name__ == "__main__":
    unittest.main()
