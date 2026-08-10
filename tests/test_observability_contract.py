from __future__ import annotations

import json
import re
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
DASHBOARD = REPO / "infra/datadog/straylight-production-dashboard.json"
PERCENTILES = REPO / "infra/datadog/distribution-percentiles.json"
MONITORS = REPO / "infra/datadog/straylight-production-monitors.json"
HTTP_CHECK = REPO / "deploy/railway/datadog-agent/http_check.yaml"
DATADOG_DOCKERFILE = REPO / "deploy/railway/datadog-agent/Dockerfile"
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
        self.http_check = HTTP_CHECK.read_text()
        self.datadog_dockerfile = DATADOG_DOCKERFILE.read_text()
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
        self.assertEqual(9, len(groups))
        self.assertGreaterEqual(child_count, 53)
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
                r"((?:straylight|datadog\.dogstatsd\.client|network\.http)\.[a-z0-9_.]+)",
                monitor["query"],
            )
            service_check = re.search(r'\"(http\.can_connect)\"', monitor["query"])
            self.assertTrue(metric or service_check, monitor["name"])
            if service_check:
                self.assertEqual("service check", monitor["type"])
                continue
            self.assertIsNotNone(metric, monitor["name"])
            if metric.group(1).startswith("straylight."):
                emitter_name = metric.group(1).removeprefix("straylight.")
                self.assertIn(f'"{emitter_name}"', self.rust, monitor["name"])

    def test_public_edge_http_check_is_bounded_and_outside_in(self):
        self.assertIn("gcr.io/datadoghq/agent:7.81.2@sha256:", self.datadog_dockerfile)
        self.assertIn(
            "COPY deploy/railway/datadog-agent/http_check.yaml "
            "/etc/datadog-agent/conf.d/http_check.d/conf.yaml",
            self.datadog_dockerfile,
        )
        required = [
            "url: https://straylight.rourkem.com/healthz",
            "url: https://straylight.rourkem.com/api/ready",
            "http_response_status_code: 200",
            "content_match: '^ok\\s*$'",
            "content_match: '\"status\"\\s*:\\s*\"ready\"'",
            "allow_redirects: false",
            "tls_verify: true",
            "collect_response_time: true",
            "timeout: 3",
            "connect_timeout: 3",
            "read_timeout: 3",
            "min_collection_interval: 15",
            "service: straylight",
            "component:public-edge",
            "probe:public-edge",
            "platform:railway",
            "vantage:railway-agent",
        ]
        for setting in required:
            self.assertIn(setting, self.http_check)
        self.assertEqual(2, self.http_check.count("  - name: straylight-public-"))

    def test_public_edge_monitors_use_agent_7812_http_check_signals(self):
        by_name = {monitor["name"]: monitor for monitor in self.monitors}
        connectivity = by_name["[Straylight] Public edge connectivity"]
        self.assertEqual("service check", connectivity["type"])
        self.assertIn('"http.can_connect"', connectivity["query"])
        self.assertIn("probe:public-edge", connectivity["query"])
        self.assertIn("platform:railway", connectivity["query"])
        self.assertIn(
            '.by("host", "instance", "url").last(3).count_by_status()',
            connectivity["query"],
        )
        self.assertNotIn(".last(3).by(", connectivity["query"])
        self.assertEqual(
            {"ok": 2, "warning": 1, "critical": 2},
            connectivity["options"]["thresholds"],
        )
        self.assertTrue(connectivity["options"]["notify_no_data"])

        latency = by_name["[Straylight] Public edge response is slow"]
        self.assertEqual("metric alert", latency["type"])
        self.assertIn("network.http.response_time", latency["query"])
        self.assertEqual(1, latency["options"]["thresholds"]["critical"])

    def test_open_and_search_critical_alerts_match_performance_gates(self):
        by_name = {monitor["name"]: monitor for monitor in self.monitors}
        gates = {
            "[Straylight] Simplified workspace open is slow": 5000,
            "[Straylight] Simplified workspace search is slow": 3000,
        }
        for name, gate_ms in gates.items():
            monitor = by_name[name]
            self.assertEqual(gate_ms, monitor["options"]["thresholds"]["critical"])
            self.assertTrue(monitor["query"].endswith(f"> {gate_ms}"))

    def test_functional_monitor_filters_do_not_mix_symbolic_commas(self):
        for monitor in self.monitors:
            query = monitor["query"]
            if " IN (" not in query and " NOT IN (" not in query:
                continue
            scope = query.split("{", 1)[1].split("}", 1)[0]
            self.assertIn(
                "env:__DD_ENV__ AND service:__DD_SERVICE__ AND ",
                scope,
            )
            self.assertNotIn("env:__DD_ENV__,", scope)
            self.assertNotIn("service:__DD_SERVICE__,", scope)


if __name__ == "__main__":
    unittest.main()
