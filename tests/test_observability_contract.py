from __future__ import annotations

import json
import re
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
DASHBOARD = REPO / "infra/datadog/straylight-production-dashboard.json"
PERCENTILES = REPO / "infra/datadog/distribution-percentiles.json"
MONITORS = REPO / "infra/datadog/straylight-production-monitors.json"
RUST_SOURCE = REPO / "apps/api/src"


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)


def metric_macro_bodies(source: str):
    for match in re.finditer(r"(?:counter|gauge|histogram)!\s*\(", source):
        start = match.end()
        depth = 1
        for index in range(start, len(source)):
            if source[index] == "(":
                depth += 1
            elif source[index] == ")":
                depth -= 1
                if depth == 0:
                    yield source[start:index]
                    break


class ObservabilityContractTests(unittest.TestCase):
    def setUp(self):
        self.dashboard = json.loads(DASHBOARD.read_text())
        self.percentiles = json.loads(PERCENTILES.read_text())
        self.monitors = json.loads(MONITORS.read_text())
        self.rust = "\n".join(
            path.read_text() for path in sorted(RUST_SOURCE.glob("*.rs"))
        )

    def test_dashboard_is_operationally_complete_and_uses_unified_filters(self):
        groups = [
            widget
            for widget in self.dashboard["widgets"]
            if widget["definition"]["type"] == "group"
        ]
        child_count = sum(
            len(group["definition"]["widgets"]) for group in groups
        )
        self.assertEqual(7, len(groups))
        self.assertGreaterEqual(child_count, 40)
        variables = {
            variable["name"]: variable["prefix"]
            for variable in self.dashboard["template_variables"]
        }
        self.assertEqual(
            {
                "env": "env",
                "service": "service",
                "version": "version",
                "component": "component",
            },
            variables,
        )

    def test_every_straylight_dashboard_metric_has_a_rust_emitter(self):
        metric_names = {
            match
            for value in strings(self.dashboard)
            for match in re.findall(r"straylight\.([a-z0-9_.]+)", value)
        }
        self.assertGreaterEqual(len(metric_names), 45)
        missing = sorted(
            metric for metric in metric_names if f'"{metric}"' not in self.rust
        )
        self.assertEqual([], missing)

    def test_metric_labels_do_not_use_content_or_identifiers(self):
        forbidden_labels = {
            "user",
            "user_id",
            "tenant",
            "tenant_id",
            "credential",
            "credential_id",
            "scope",
            "scope_id",
            "session",
            "session_id",
            "record",
            "record_id",
            "source",
            "source_ref",
            "path",
            "query",
            "title",
            "task",
            "request_id",
            "error",
            "error_message",
            "model_output",
            "object_key",
        }
        metric_source = "\n".join(metric_macro_bodies(self.rust))
        offenders = sorted(
            label
            for label in forbidden_labels
            if re.search(rf'"{re.escape(label)}"\s*=>', metric_source)
        )
        self.assertEqual([], offenders)

    def test_dashboard_only_queries_emitted_tag_keys(self):
        emitted_tags: dict[str, set[str]] = {}
        for body in metric_macro_bodies(self.rust):
            metric = re.search(r'"([a-z][a-z0-9_.]+)"', body)
            if metric is None:
                continue
            emitted_tags.setdefault(metric.group(1), set()).update(
                re.findall(r'"([a-z][a-z0-9_]*)"\s*=>', body)
            )
        emitted_tags.update(
            {
                "datadog.dogstatsd.client.packets_sent": {"client_transport"},
                "datadog.dogstatsd.client.packets_dropped": {"client_transport"},
                "datadog.dogstatsd.client.metrics_by_type": {
                    "client_transport",
                    "metrics_type",
                },
                "datadog.dogstatsd.client.bytes_dropped": {"client_transport"},
            }
        )
        global_tags = {"env", "service", "version", "component"}
        invalid: list[str] = []
        for query in strings(self.dashboard):
            metric_match = re.search(
                r"(?:straylight|datadog\.dogstatsd\.client)\.[a-z0-9_.]+",
                query,
            )
            if metric_match is None:
                continue
            metric = metric_match.group(0)
            scope = re.search(re.escape(metric) + r"\{([^}]*)\}", query)
            query_tags = (
                set(re.findall(r"(?:^|,)([a-z][a-z0-9_]*):", scope.group(1)))
                if scope
                else set()
            )
            group = re.search(r"\bby \{([^}]+)\}", query)
            if group:
                query_tags.update(tag.strip() for tag in group.group(1).split(","))
            emitter_name = metric.removeprefix("straylight.")
            unsupported = query_tags - emitted_tags.get(emitter_name, set()) - global_tags
            if unsupported:
                invalid.append(f"{metric}: {sorted(unsupported)}")
        self.assertEqual([], invalid)

    def test_every_percentile_widget_has_a_managed_distribution(self):
        percentile_metrics = {
            metric
            for query in strings(self.dashboard)
            for metric in re.findall(
                r"\bp[0-9.]+:(straylight\.[a-z0-9_.]+)",
                query,
            )
        }
        self.assertEqual(percentile_metrics, set(self.percentiles))

    def test_monitors_are_complete_and_query_emitted_metrics(self):
        self.assertGreaterEqual(len(self.monitors), 10)
        names = [monitor["name"] for monitor in self.monitors]
        self.assertEqual(len(names), len(set(names)))
        for monitor in self.monitors:
            self.assertTrue(monitor["name"].startswith("[Straylight] "))
            self.assertIn("__DD_ENV__", monitor["query"])
            self.assertIn("__DD_SERVICE__", monitor["query"])
            self.assertIn("__NOTIFY__", monitor["message"])
            self.assertTrue(monitor["options"]["notify_audit"])
            metric = re.search(
                r"((?:straylight|datadog\.dogstatsd\.client)\.[a-z0-9_.]+)",
                monitor["query"],
            )
            self.assertIsNotNone(metric, monitor["name"])
            if metric.group(1).startswith("straylight."):
                emitter_name = metric.group(1).removeprefix("straylight.")
                self.assertIn(f'"{emitter_name}"', self.rust, monitor["name"])


if __name__ == "__main__":
    unittest.main()
