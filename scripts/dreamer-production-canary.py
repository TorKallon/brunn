#!/usr/bin/env python3
"""HTTP-only Dreamer release canary. Default: read-only production preflight.

Execution is explicit, uses a new isolated account, and always requests account
deletion afterward. The real owner's token is used only for GETs/provisioning.
No model, secret mutation, installation registration, SQL, or service startup.
See docs/Dreamer Canary.md for the private provisioning transport and cleanup
limitations. Credentials stay in memory; output contains only whitelisted facts.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from zoneinfo import ZoneInfo

PREFIX = "brunn-dreamer-canary:"
SOURCE_PATH = "sources/Canary/Observation.md"
SUMMARY_PATH = "derived/entities/canary.md"
RUNNER_CAPS = {"secret:read", "secret:write", "notification:publish", "dreamer:run"}
RECEIPT_FIELDS = {
    "schema", "run_id", "receipt_ref", "receipt_version", "receipt_path", "status",
    "completed_at", "mode", "runner", "mode_flip", "probe_monitoring", "applied_writes",
    "entering_veto_window_today", "pending_owner", "pending_review_surfaces", "next_run_at",
}
MAX_RESPONSE = 4 * 1024 * 1024


class CanaryError(Exception):
    """Only locally authored, credential-free errors may escape to the report."""


def require(condition, message):
    if not condition:
        raise CanaryError(message)


def unwrap(value):
    return value.get("data", value)


def encoded(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()


def stamp():
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def parse_time(value):
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    require(parsed.tzinfo is not None, "timestamp must have an explicit offset")
    return parsed


def safe_base(value, *, admin=False):
    parts = urllib.parse.urlsplit(value)
    require(not (parts.username or parts.password or parts.query or parts.fragment),
            "API base must not contain credentials, a query, or a fragment")
    public = (parts.scheme == "https" and parts.hostname == "brunn.ai"
              and parts.port in {None, 443} and parts.path.rstrip("/") == "/api")
    private = (parts.scheme == "http" and parts.hostname in {"127.0.0.1", "localhost", "::1"}
               and parts.port is not None and parts.path in {"", "/"})
    require(public or (admin and private),
            "use the verified https://brunn.ai/api origin; admin transport may use explicit loopback HTTP")
    return value.rstrip("/")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        raise CanaryError("HTTP redirect refused; credentials were not forwarded")


class Client:
    def __init__(self, base, token, calls, actor):
        self.base, self.token, self.calls, self.actor = base, token, calls, actor
        # Ignore environment proxies so Keychain custody cannot be redirected.
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def request(self, method, path, body=None, expected=(200,), *, public=False):
        require(path.startswith("/v1/") or path == "/ready", "unexpected canary API path")
        headers = {"Content-Type": "application/json", "User-Agent": "Brunn-Dreamer-Canary/1.0"}
        if not public:
            headers["Authorization"] = "Bearer " + self.token
        request = urllib.request.Request(self.base + path, method=method, headers=headers,
                                         data=encoded(body) if body is not None else None)
        started = time.monotonic()
        try:
            with self.opener.open(request, timeout=30) as response:
                status, data = response.status, response.read(MAX_RESPONSE + 1)
        except urllib.error.HTTPError as error:
            with error:
                status, data = error.code, error.read(MAX_RESPONSE + 1)
        except (urllib.error.URLError, TimeoutError, OSError):
            self.calls.append({"actor": self.actor, "method": method, "path": path,
                               "status": "transport_uncertain"})
            raise CanaryError(f"{self.actor} {method} {path}: transport result uncertain; no automatic retry") from None
        self.calls.append({"actor": self.actor, "method": method, "path": path, "status": status,
                           "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
                           "response_bytes": len(data)})
        require(status in expected, f"{self.actor} {method} {path}: HTTP {status}")
        require(len(data) <= MAX_RESPONSE, "HTTP response exceeds canary byte bound")
        if status not in range(200, 300):
            result = {"http_status": status}
            # Typed rejection checks need the public code, but never the error
            # message, details, credentials or arbitrary response content.
            try:
                payload = json.loads(data)
            except (ValueError, UnicodeDecodeError):
                payload = None
            error = payload.get("error") if isinstance(payload, dict) else None
            code = error.get("code") if isinstance(error, dict) else None
            if isinstance(code, str) and re.fullmatch(r"[a-z][a-z0-9_]{0,95}", code):
                result["error"] = {"code": code}
            return result
        try:
            return json.loads(data)
        except (ValueError, UnicodeDecodeError):
            raise CanaryError("successful HTTP response was not JSON") from None


def owner_token():
    result = subprocess.run(
        ["security", "find-generic-password", "-s", "straylight.rourkem.com",
         "-a", "owner-api-token", "-w"], capture_output=True, text=True, timeout=15,
    )
    require(result.returncode == 0 and result.stdout.strip(), "verified owner Keychain item unavailable")
    return result.stdout.strip()


def preflight(owner):
    me = owner.request("GET", "/v1/me")
    status = owner.request("GET", "/v1/status")
    ready = owner.request("GET", "/ready", public=True)
    dashboard = unwrap(owner.request("GET", "/v1/workspace/dashboard?timezone=America%2FLos_Angeles"))
    notifications = owner.request("GET", "/v1/workspace/notifications?limit=1")
    credentials = owner.request("GET", "/v1/credentials")
    review = unwrap(owner.request("GET", "/v1/dreamer/review", expected=(200, 404)))
    # Retain no titles, bodies, source paths, credential labels, or user identity.
    result = {
        "observed_at": stamp(), "status": status.get("status"),
        "build_revision": status.get("build_revision"),
        "read_only": me.get("read_only"), "owner_admin_capability": "admin" in me.get("capabilities", []),
        "readiness": {"status": ready.get("status"), "dependencies": ready.get("dependencies")},
        "feature_flags": status.get("feature_flags", {}),
        "storage": dashboard.get("storage"),
        "credential_count": credentials.get("total"),
        "notification_unread_count": notifications.get("unread_count"),
        "review_http_status": review.get("http_status", 200),
    }
    if result["review_http_status"] == 200:
        result["review"] = {key: review.get(key) for key in ("available", "mode", "paused", "counts", "decision_version")}
        result["review"]["last_attempt"] = {
            key: review.get("last_attempt", {}).get(key)
            for key in ("outcome", "started_at", "finished_at")
        } if isinstance(review.get("last_attempt"), dict) else None
    return result, me["user"]["id"]


def fixture_identity(identity, fixture, real_owner_id, *, fresh=False):
    user = identity["user"]
    require(user["id"] == fixture["user_id"] and user["id"] != real_owner_id,
            "fixture identity mismatch; no fixture writes or cleanup permitted")
    external = fixture["external_ref"]
    suffix = external.removeprefix(PREFIX)
    require(external.startswith(PREFIX) and str(uuid.UUID(suffix)) == suffix
            and user["external_ref"] == external,
            "fixture external reference mismatch; no cleanup permitted")
    require(not identity.get("read_only") or identity.get("capabilities") == ["status"],
            "fixture owner credential is read-only")
    if fresh:
        age = (datetime.now(timezone.utc) - parse_time(user["created_at"])).total_seconds()
        require(-60 <= age <= 3600, "fixture must have been provisioned within the last hour")
        require("credential:manage" in identity.get("capabilities", []), "fixture requires its own owner credential")


def read_one(client, **request):
    envelope = client.request("POST", "/v1/workspace/read", {"requests": [request]})
    items = unwrap(envelope).get("items", [])
    require(len(items) == 1, "read did not return exactly one fixture item")
    return items[0]


def write(client, path, content, version, *, metadata=None):
    return unwrap(client.request("POST", "/v1/workspace/write", {
        "path": path, "content": content, "expected_version": version, "metadata": metadata or {},
    }))


def receipt(value, run_ref, run_version):
    require(set(value) == RECEIPT_FIELDS, "latest receipt must have exactly the v2 field names")
    require(value["schema"] == "dream.latest-receipt.v2" and value["status"] == "completed"
            and value["mode"] == "full", "latest receipt outcome/schema/mode mismatch")
    require(value["receipt_ref"] == run_ref and value["receipt_version"] == run_version,
            "latest receipt must pin the actual terminal run version")
    run_uuid = uuid.UUID(run_ref.removeprefix("entry:"))
    require(run_ref == "entry:" + str(run_uuid) and run_uuid.version == 7,
            "receipt requires a canonical UUIDv7 entry reference")
    require(re.fullmatch(r"\d{4}-\d{2}-\d{2}", value["run_id"])
            and value["receipt_path"] == "dreams/runs/" + value["run_id"] + ".md",
            "receipt run identity mismatch")
    require(parse_time(value["next_run_at"]) > parse_time(value["completed_at"]), "receipt next run must follow completion")
    require(value["mode_flip"] is False and value["probe_monitoring"] is None,
            "retired monitoring fields must remain explicit")
    row_fields = {
        "applied_writes": {"path", "version", "summary"},
        "entering_veto_window_today": {"recommendation_id", "summary", "apply_at"},
        "pending_owner": {"recommendation_id", "summary", "reason", "published_at", "age_days"},
        "pending_review_surfaces": {"review_id", "summary", "reason", "path", "ref", "version", "published_at"},
    }
    for category, fields in row_fields.items():
        require(isinstance(value[category], list) and all(set(row) == fields for row in value[category]),
                "receipt recommendation row fields mismatch")


def terminal_receipt(owner, finished):
    expected = finished["latest_receipt"]
    receipt(expected, finished["run_entry_ref"], finished["run_version"])
    latest = read_one(owner, path="dreams/latest-receipt.md", view="full", max_chars=200_000)
    blocks = re.findall(r"```json\n([^\n]+)\n```", latest.get("text", ""))
    require(len(blocks) == 1, "latest receipt must contain one canonical JSON block")
    parsed = json.loads(blocks[0])
    receipt(parsed, finished["run_entry_ref"], finished["run_version"])
    require(encoded(parsed).decode() == blocks[0] and parsed == expected,
            "persisted latest receipt differs from canonical terminal payload")
    document = ("---\nschema: dream.latest-receipt.v2\nrun_id: " + parsed["run_id"]
        + "\nstatus: completed\n---\n# Latest Dreaming receipt\n\n"
        + "This is a stable briefing projection. The immutable receipt is authoritative.\n\n```json\n"
        + blocks[0] + "\n```\n")
    require(latest["text"] == document, "persisted receipt document is not the strict canonical projection")
    return expected


def await_summary_preference(reader, report, persist, timeout):
    report["phase"] = "awaiting_summary_preference_enable"
    persist()
    print(json.dumps({"phase": report["phase"], "timeout_seconds": timeout}), flush=True)
    deadline = time.monotonic() + timeout
    while True:
        try:
            status = reader.request("GET", "/v1/status")
            if status.get("feature_flags", {}).get("dreamer_summary_reads_enabled") is True:
                report["phase"] = "summary_preference_enabled"
                persist()
                return
        except CanaryError:
            # A flag rollout may restart the API. Retry only this bounded GET;
            # no credential provisioning, approval, or other write is repeated.
            pass
        require(time.monotonic() < deadline, "timed out waiting for summary preference flag rollout")
        time.sleep(5)


def run_cycle(owner, runner, reader, source, text, target_version, report, enable_gate=None):
    date = datetime.now(ZoneInfo("America/Los_Angeles")).date().isoformat()
    admit_body = {"attempt_id": str(uuid.uuid4()), "date": date, "kind": "manual", "lease_seconds": 120}
    admitted = runner.request("POST", "/v1/workspace/dreamer/admit", admit_body)
    require(admitted.get("admitted") is True and admitted["mode"] == "full", "fixture run was not admitted in full mode")
    require(any(row["entry_ref"] == source["entry_ref"] and row["version"] == source["version"]
                for row in admitted["inputs"]), "source was not durably admitted")
    require(not admitted.get("location_work"), "synthetic canary must not admit personal location work")
    require(len(admitted["inputs"]) == 1, "disposable fixture admission must contain only its synthetic source")
    fence = {key: admitted[key] for key in ("attempt_id", "fence")}
    discovery_body = {**fence, "expected_state_version": admitted["state_version"], "queries": ["Canary source"]}
    reader.request("POST", "/v1/workspace/dreamer/narrative-discover", discovery_body, expected=(403,))
    discovered = unwrap(runner.request("POST", "/v1/workspace/dreamer/narrative-discover", discovery_body))
    require(discovered["inputs"] == admitted["inputs"]
            and discovered["processed_generation"] == admitted["processed_generation"],
            "context discovery consumed pending work")
    context = discovered.get("narrative_context", [])
    require(len(context) == 1 and context[0]["entry_ref"] == source["entry_ref"]
            and context[0]["version"] == source["version"], "context discovery escaped the exact synthetic source")
    replay = unwrap(runner.request("POST", "/v1/workspace/dreamer/narrative-discover", discovery_body))
    require(replay == discovered, "context replay changed its frozen admission")
    admitted = discovered
    submitted_body = {**fence, "expected_state_version": admitted["state_version"],
        "candidates": [{"kind": "summary", "title": "Canary summary", "summary": "Synthetic release canary.",
            "reason": "Verify exact-source summary publication.", "path": SUMMARY_PATH,
            "content": "# Canary summary\n\n" + text + "[^s1]\n", "expected_version": target_version,
            "sources": [{"entry_ref": source["entry_ref"], "version": source["version"], "start_line": 3, "end_line": 3}]}],
        "processed_inputs": admitted["inputs"], "findings": ["Synthetic source inspected without a model call."]}
    submitted = runner.request("POST", "/v1/workspace/dreamer/candidates", submitted_body)
    require(runner.request("POST", "/v1/workspace/dreamer/candidates", submitted_body) == submitted,
            "candidate retry changed immutable identity")
    terminal_body = {**fence, "expected_state_version": submitted["state_version"], "outcome": "completed",
        "detail": "Synthetic HTTP canary completed without model execution.",
        "auth_persistence": {"status": "not_needed"}, "notification": {"status": "not_needed"}}
    finished = runner.request("POST", "/v1/workspace/dreamer/finish", terminal_body)
    require(runner.request("POST", "/v1/workspace/dreamer/finish", terminal_body) == finished,
            "terminal retry changed receipt")
    previous_receipt = terminal_receipt(owner, finished)
    run_ref, run_version = finished["run_entry_ref"], finished["run_version"]
    notification = {"event_key": "dreamer:canary:" + admitted["attempt_id"], "correlation_id": admitted["attempt_id"],
        "kind": "operational", "importance": "important", "title": "Canary review ready",
        "body": "Synthetic fixture only.", "target": {"type": "entry", "entry_ref": run_ref},
        "source": {"type": "dreamer_run", "ref": run_ref, "version_ref": f"{run_ref}@{run_version}"}}
    published = runner.request("POST", "/v1/workspace/notifications/publish", notification)
    require(published.get("delivery_count") == 0 and published.get("delivery_status") == "no_installations"
            and "notification" not in published, "restricted fixture publisher must expose no content or delivery")
    replayed = runner.request("POST", "/v1/workspace/notifications/publish", notification)
    require(replayed.get("replayed") is True and replayed["notification_ref"] == published["notification_ref"],
            "notification retry created a different event")
    detail = owner.request("GET", "/v1/workspace/notifications/" + published["notification_ref"])["notification"]
    require(detail["deliveries"] == [] and detail["source"] == notification["source"], "notification source pin or empty delivery mismatch")
    view = unwrap(owner.request("GET", "/v1/dreamer/review"))
    matches = [item for item in view["items"] if item["id"] in submitted["accepted_candidate_ids"]]
    require(len(matches) == 1 and matches[0]["reviewable"] and not matches[0]["stale"], "fixture candidate is not reviewable")
    item = matches[0]
    require(item["candidate"]["target_path"] == SUMMARY_PATH, "candidate destination escaped the fixture")
    decision = {"item_id": item["id"], "run_entry_ref": item["run_entry_ref"], "run_version": item["run_version"],
        "candidate_hash": item["candidate_hash"], "expected_decisions_version": view["decision_version"],
        "decision": "approve", "comment": "Synthetic fixture owner approval; no personal proposal involved.",
        "idempotency_key": "canary-decision:" + admitted["attempt_id"]}
    reader.request("POST", "/v1/dreamer/review/decisions", decision, expected=(403,))
    runner.request("POST", "/v1/dreamer/review/decisions", decision, expected=(403,))
    approved = unwrap(owner.request("POST", "/v1/dreamer/review/decisions", decision))
    require(approved.get("application_status") == "applied", "full-mode owner decision did not publish")
    replay = unwrap(owner.request("POST", "/v1/dreamer/review/decisions", decision))
    require(all(replay.get(key) == approved.get(key) for key in
                ("saved", "decision", "application_status", "state_version")),
            "owner decision replay changed its application result")
    # Review is not a new nightly run: preserve the actual terminal identity/time.
    after = read_one(owner, path="dreams/latest-receipt.md", view="full", max_chars=200_000)
    after_payload = json.loads(re.findall(r"```json\n([^\n]+)\n```", after["text"])[0])
    for key in ("status", "completed_at", "receipt_ref", "receipt_version", "run_id"):
        require(after_payload[key] == previous_receipt[key], "owner decision replaced actual runner status")
    direct = read_one(reader, path=SUMMARY_PATH, view="current_state", max_chars=100_000)
    require(direct.get("representation") == "derived_summary" and direct["freshness"]["status"] == "fresh",
            "published summary path did not return a validated fresh summary")
    exact = read_one(reader, ref=source["entry_ref"], version=source["version"], view="full", max_chars=100_000)
    require(exact["reference"] == source["entry_ref"] and exact["text"].startswith("# Canary source\n"),
            "exact source read substituted a summary")
    current = read_one(reader, ref=source["entry_ref"], view="current_state", max_chars=100_000)
    if enable_gate is not None:
        require(current["reference"] == source["entry_ref"] and current["text"] == exact["text"]
                and current.get("representation") != "derived_summary",
                "disabled summary preference must preserve the canonical source")
        report["disabled_preference_baseline"] = "direct summary validated; canonical source preference preserved"
        enable_gate()
        current = read_one(reader, ref=source["entry_ref"], view="current_state", max_chars=100_000)
    require(current.get("representation") == "derived_summary" and current["freshness"]["status"] == "fresh",
            "current_state preference gate did not return a fresh summary (check read flag)")
    require(text in current["text"] and current["version"] == target_version + 1, "summary text/version mismatch")
    report["cycles"].append({"source_version": source["version"], "run_entry_ref": run_ref,
        "run_version": run_version, "candidate_run_version": item["run_version"], "item_id": item["id"],
        "notification_ref": published["notification_ref"], "delivery_count": 0,
        "narrative_context_count": len(context), "narrative_discovery_replayed": True,
        "summary_version": current["version"], "source_chars": len(exact["text"]),
        "summary_chars": len(current["text"]), "freshness": current["freshness"]["status"]})
    return current


def research_body(admission):
    body = {key: admission[key] for key in ("attempt_id", "fence")}
    body.update(expected_state_version=admission["state_version"], operation_id=str(uuid.uuid4()))
    if admission.get("research"):
        body.update(subject_ref=admission["research"]["subject_ref"],
                    research_version=admission["research"]["version"])
    return body


def measure_subject_reads(reader, canonical, summary, samples=5):
    """Alternate warmed HTTP reads; report actual response bytes without a speed gate."""
    measurements = {"canonical_current_state": [], "exact_raw_source": []}
    for sample in range(samples + 1):
        for name in measurements:
            request = {"ref": canonical["entry_ref"], "max_chars": 100_000}
            request.update({"view": "current_state"} if name == "canonical_current_state"
                           else {"view": "full", "version": canonical["version"]})
            item = read_one(reader, **request)
            if name == "canonical_current_state":
                require(item.get("representation") == "derived_summary"
                        and item["reference"] == summary["reference"]
                        and item["freshness"]["status"] == "fresh", "measured canonical read lost its fresh overview")
            else:
                require(item["reference"] == canonical["entry_ref"] and item["version"] == canonical["version"]
                        and item.get("representation") != "derived_summary", "measured exact source was substituted")
            if sample:  # One warmup per arm; retain no response text or metadata.
                call = reader.calls[-1]
                require(call["path"] == "/v1/workspace/read" and call["status"] == 200,
                        "read measurement did not match an HTTP response")
                measurements[name].append({key: call[key] for key in ("elapsed_ms", "response_bytes")})
    return {name: {"samples": rows, "median_ms": statistics.median(row["elapsed_ms"] for row in rows),
                   "median_response_bytes": statistics.median(row["response_bytes"] for row in rows)}
            for name, rows in measurements.items()}


def run_subject_cycle(owner, runner, reader, report):
    """Supplement legacy cycles in the same disposable fixture; no model or push."""
    canonical_path = "sources/Projects/Canary Aster/Canary Aster.md"
    trail_path, primary_path = "sources/CanaryResearch/Trail.md", "sources/CanaryResearch/Outcome.md"
    trail_target, primary_target = "CanaryResearch/Trail", "CanaryResearch/Outcome.md"
    canonical_text = ("## Purpose\n\nCanary Aster has a project trail at [[" + trail_target + "]].\n"
                      + "\nSynthetic padding for the fixture read comparison.\n" * 100)
    trail_text = "# Trail\n\nThe primary result is in [[" + primary_target + "]].\n"
    old_text = "# Outcome\n\nAn old plan is awaiting execution.\n"
    outcome = "The current outcome is complete; one equipment detail remains unresolved."
    new_text = "# Outcome\n\n" + outcome + "\n"
    canonical = write(owner, canonical_path, canonical_text, 0)
    trail = write(owner, trail_path, trail_text, 0)
    primary = write(owner, primary_path, old_text, 0)
    date = datetime.now(ZoneInfo("America/Los_Angeles")).date().isoformat()
    admitted = runner.request("POST", "/v1/workspace/dreamer/admit", {
        "attempt_id": str(uuid.uuid4()), "date": date, "kind": "manual", "lease_seconds": 600,
        "requested_subject_refs": [canonical["entry_ref"]]})
    require(admitted.get("admitted") is True and admitted.get("mode") == "full"
            and admitted.get("research_protocol") == 1, "release requires admitted full-mode research_protocol 1")
    expected = {row["entry_ref"]: row["version"] for row in (canonical, trail, primary)}
    require(not admitted.get("location_work") and len(admitted["inputs"]) == 3
            and {row["entry_ref"]: row["version"] for row in admitted["inputs"]} == expected,
            "subject admission escaped the three synthetic sources")
    next_body = research_body(admitted)
    reader.request("POST", "/v1/workspace/dreamer/research-next", next_body, expected=(403,))
    current = unwrap(runner.request("POST", "/v1/workspace/dreamer/research-next", next_body))
    require(current["research"]["subject_ref"] == canonical["entry_ref"]
            and current["research"]["subject_path"] == canonical_path,
            "requested canonical identity was not selected")
    canonical_read = read_one(reader, ref=canonical["entry_ref"], version=canonical["version"], view="full", max_chars=100_000)
    require(canonical_read["text"] == canonical_text, "canonical exact source changed before research")
    initial_notes = "Canary Aster has a project trail."
    initial_selectors = [{"entry_ref": canonical["entry_ref"], "version": canonical["version"],
                          "start_line": 3, "end_line": len(canonical_text.splitlines())}]
    # Full/range reads stop at EOF. A trailing-newline counting error must not
    # discard the research conclusions; the server records the actual range.
    requested_selectors = [{**row, "end_line": row["end_line"] + 1} for row in initial_selectors]
    current = unwrap(runner.request("POST", "/v1/workspace/dreamer/research-progress", {
        **research_body(current), "notes": initial_notes, "reviewed_sources": requested_selectors,
        "pending_queries": [], "pending_targets": [trail_target], "status": "researching"}))
    checked_selectors = current["research"]["reviewed_sources"]
    require(current["research"]["notes"] == initial_notes
            and [{key: row.get(key) for key in ("entry_ref", "version", "start_line", "end_line")}
                 for row in checked_selectors] == initial_selectors,
            "research checkpoint did not preserve notes with exact end-of-document selectors")
    require(all("excerpt" not in row for row in checked_selectors),
            "research checkpoint copied source excerpts instead of retaining compact selectors")
    # The imported display title is a section label, not another project name.
    unrelated_path = "sources/Elsewhere/Unrelated.md"
    unrelated = write(owner, unrelated_path, "## Purpose\n\nA separate fixture task has unrelated evidence.\n", 0)
    requests = []
    # Follow two source links, then re-read a changed primary within this attempt.
    for target, source, text in ((trail_target, trail, trail_text), (primary_target, primary, old_text),
                                (primary["entry_ref"], primary, new_text)):
        if len(requests) == 2:
            primary = write(owner, primary_path, new_text, primary["version"])
            source = primary
            rejected = runner.request("POST", "/v1/workspace/dreamer/research-progress", {
                **research_body(current), "notes": initial_notes, "reviewed_sources": checked_selectors,
                "pending_queries": [], "pending_targets": [], "status": "researching"}, expected=(400,))
            require(rejected.get("error", {}).get("code") == "research_refresh_required",
                    "changed subject scope did not return the typed refresh-required rejection")
        body = {**research_body(current), "queries": ["Canary Aster"] if not requests else [], "targets": [target]}
        requests.append(body)
        current = unwrap(runner.request("POST", "/v1/workspace/dreamer/narrative-discover", body))
        if len(requests) < 3:
            require(current["research"]["notes"] == initial_notes
                    and current["research"]["reviewed_sources"] == checked_selectors,
                    "additive discovery lost checked progress or marked new sources reviewed")
        else:
            require(not current["research"]["notes"] and not current["research"]["reviewed_sources"],
                    "changed dependency did not invalidate checked progress")
        require(any(row["entry_ref"] == source["entry_ref"] and row["version"] == source["version"]
                    for row in current["research"]["sources"]), "linked exact source was not admitted at its current version")
        unresolved = current["research"].get("coverage", {}).get("unresolved_targets")
        require(isinstance(unresolved, list) and target not in unresolved,
                "imported link resolution receipt still reports the admitted target unresolved")
        exact = read_one(reader, ref=source["entry_ref"], version=source["version"], view="full", max_chars=100_000)
        require(exact["reference"] == source["entry_ref"] and exact["text"] == text,
                "linked primary exact source mismatch")
        require(current["frozen_generation"] == admitted["frozen_generation"]
                and current["inputs"] == admitted["inputs"]
                and current["processed_generation"] == admitted["processed_generation"],
                "research discovery changed the frozen attempt or consumed pending inputs")
    require(current["research"]["snapshot_generation"] > admitted["frozen_generation"],
            "research did not advance beyond the original attempt snapshot")
    selectors = [{"entry_ref": row["entry_ref"], "version": row["version"], "start_line": 3, "end_line": 3}
                 for row in (canonical, primary)]
    current = unwrap(runner.request("POST", "/v1/workspace/dreamer/research-progress", {
        **research_body(current), "notes": outcome, "reviewed_sources": selectors,
        "pending_queries": [], "pending_targets": [], "status": "researching"}))
    require(current["research"]["notes"] == outcome
            and {(row["entry_ref"], row["version"]) for row in current["research"]["reviewed_sources"]}
                == {(row["entry_ref"], row["version"]) for row in selectors},
            "research checkpoint lost its supported notes or exact source selectors")
    replay = runner.request("POST", "/v1/workspace/dreamer/narrative-discover", requests[0])
    latest = unwrap(replay)
    require(replay.get("no_op") is True and latest["research"]["version"] == current["research"]["version"]
            and latest["research"]["sources"] == current["research"]["sources"]
            and latest["state_version"] == current["state_version"], "old discovery replay rewound newer research")
    current, job = latest, latest["research"]
    expected[primary["entry_ref"]] = primary["version"]
    require(len(job["sources"]) == 3 and {row["entry_ref"]: row["version"] for row in job["sources"]} == expected,
            "research header manifest omitted or duplicated a fixture dependency")
    content = "# Canary Aster overview\n\nCanary Aster has a project trail.[^s1]\n" + outcome + "[^s2]\n"
    submission = {
        **research_body(current), "candidates": [{"kind": "summary", "title": "Canary Aster overview",
            "summary": "Synthetic subject overview with one unresolved detail.", "reason": "Verify persistent exact-source research.",
            "subject_ref": canonical["entry_ref"], "path": job["output_path"], "expected_version": job["output_version"],
            "content": content, "sources": selectors}], "processed_inputs": [],
        "research_progress": {"notes": outcome, "reviewed_sources": selectors,
                              "pending_queries": [], "pending_targets": [], "status": "waiting"},
        "findings": ["Synthetic linked evidence inspected; original inputs deliberately retained."]}
    invalid_submission = json.loads(json.dumps(submission))
    invalid_submission["operation_id"] = str(uuid.uuid4())
    invalid_submission["candidates"][0]["sources"][1]["end_line"] = len(new_text.splitlines()) + 1
    rejected = runner.request("POST", "/v1/workspace/dreamer/candidates", invalid_submission, expected=(400,))
    require(rejected.get("http_status") == 400, "publication accepted an out-of-range source selector")
    submitted = runner.request("POST", "/v1/workspace/dreamer/candidates", submission)
    require(len(submitted["accepted_candidate_ids"]) == 1, "subject candidate was not accepted exactly once")
    terminal = {key: admitted[key] for key in ("attempt_id", "fence")}
    terminal.update(expected_state_version=submitted["state_version"], outcome="partial",
                    detail="Synthetic subject completed; original inputs retained.",
                    auth_persistence={"status": "not_needed"}, notification={"status": "not_needed"})
    finished = runner.request("POST", "/v1/workspace/dreamer/finish", terminal)
    require(finished["latest_receipt"]["status"] == "partial" and finished["latest_receipt"]["mode"] == "full",
            "subject terminal receipt misrepresented retained inputs")
    view = unwrap(owner.request("GET", "/v1/dreamer/review"))
    matches = [item for item in view["items"] if item["id"] in submitted["accepted_candidate_ids"]]
    require(len(matches) == 1 and matches[0]["reviewable"] and not matches[0]["stale"],
            "subject candidate was stale before fixture owner review")
    item = matches[0]
    require(item["candidate"]["target_path"] == job["output_path"], "subject candidate destination changed")
    approved = unwrap(owner.request("POST", "/v1/dreamer/review/decisions", {
        "item_id": item["id"], "run_entry_ref": item["run_entry_ref"], "run_version": item["run_version"],
        "candidate_hash": item["candidate_hash"], "expected_decisions_version": view["decision_version"],
        "decision": "approve", "comment": "Synthetic fixture subject approval.",
        "idempotency_key": "canary-subject-decision:" + admitted["attempt_id"]}))
    require(approved.get("application_status") == "applied", "full-mode subject approval did not publish")
    summary = read_one(reader, ref=canonical["entry_ref"], view="current_state", max_chars=100_000)
    require(summary.get("representation") == "derived_summary" and summary["freshness"]["status"] == "fresh"
            and summary["path"] == job["output_path"] and summary["version"] == job["output_version"] + 1
            and summary["text"].startswith(content), "canonical current_state did not select the fresh approved overview")
    manifest = summary.get("metadata", {}).get("dreamer_summary", {})
    dependencies = manifest.get("subject_scope", {}).get("dependencies", [])
    require(manifest.get("subject_ref") == canonical["entry_ref"] and len(dependencies) == 3
            and {row["entry_ref"]: row["version"] for row in dependencies} == expected
            and {row["entry_ref"] for row in manifest.get("sources", [])} == {row["entry_ref"] for row in selectors},
            "published subject lost its server-owned uncited research dependency")
    measurements = measure_subject_reads(reader, canonical, summary)
    write(owner, unrelated_path, "## Purpose\n\nThe separate fixture task has a later unrelated outcome.\n", unrelated["version"])
    for reference in (canonical["entry_ref"], summary["reference"]):
        fresh = read_one(reader, ref=reference, view="current_state", max_chars=100_000)
        require(fresh.get("representation") == "derived_summary" and fresh["freshness"]["status"] == "fresh"
                and fresh["reference"] == summary["reference"] and fresh["text"] == summary["text"],
                "unrelated section heading invalidated the subject overview")
    # Generated editions may repeat a subject without becoming new evidence.
    # Verify both the legacy type-only marker and the current structured marker.
    briefing_path = "Briefings/2026/Canary edition.md"
    for version, metadata in ((0, {"kind": "briefing_edition"}),
                              (1, {"kind": "briefing_edition", "briefing": {"schema": "briefing.v1"}})):
        briefing_text = ("# Synthetic briefing\n\nCanary Aster follows [[" + trail_path + "]].\n"
                         + "Unrelated fixture news revision " + str(version + 1) + ".\n")
        briefing = write(owner, briefing_path, briefing_text, version, metadata=metadata)
        for reference in (canonical["entry_ref"], summary["reference"]):
            fresh = read_one(reader, ref=reference, view="current_state", max_chars=100_000)
            require(fresh.get("representation") == "derived_summary" and fresh["freshness"]["status"] == "fresh"
                    and fresh["reference"] == summary["reference"] and fresh["text"] == summary["text"],
                    "generated briefing edition invalidated the subject overview")
        exact = read_one(reader, ref=briefing["entry_ref"], version=briefing["version"], view="full", max_chars=100_000)
        require(exact["reference"] == briefing["entry_ref"] and exact["version"] == briefing["version"]
                and exact["text"] == briefing_text, "generated briefing exact read was changed or unavailable")
    # Neither the canonical name nor a cited source occurs in this new note.
    write(owner, "sources/Elsewhere/ResearchUpdate.md", "# Update\n\nA later correction links to [[" + trail_path + "]].\n", 0)
    for reference in (canonical["entry_ref"], summary["reference"]):
        fallback = read_one(reader, ref=reference, view="current_state", max_chars=100_000)
        require(fallback.get("representation") == "current_source_fallback"
                and fallback["reference"] == canonical["entry_ref"] and fallback["text"] == canonical_text
                and fallback["freshness"]["reason"] == "subject_scope_changed",
                "uncited cross-directory link did not invalidate the published overview")
    for source, version, text in ((canonical, canonical["version"], canonical_text), (trail, trail["version"], trail_text),
                                  (primary, 1, old_text), (primary, primary["version"], new_text)):
        exact = read_one(reader, ref=source["entry_ref"], version=version, view="full", max_chars=100_000)
        require(exact["reference"] == source["entry_ref"] and exact["version"] == version and exact["text"] == text,
                "subject publication or invalidation mutated an exact source")
    report["subject_cycle"] = {"research_protocol": 1, "requested_subject_ref": canonical["entry_ref"],
        "discovery_rounds": len(requests), "replay_after_newer_round": True, "newer_primary_version": primary["version"],
        "supported_progress_checkpoint": True,
        "checkpoint_eof_normalized": True,
        "checkpoint_selectors_compact": True,
        "publication_eof_remains_strict": True,
        "imported_wiki_links_resolved": True,
        "subject_header_search_exercised": True,
        "changed_scope_refresh_signal": True,
        "saved_progress_survives_additive_discovery": True,
        "changed_dependency_invalidates_progress": True,
        "generic_section_heading_is_not_an_identity": True,
        "generated_briefing_editions_do_not_invalidate": True,
        "generated_briefing_exact_reads_preserved": True,
        "research_dependencies": len(dependencies), "claim_sources": len(selectors), "item_id": item["id"],
        "summary_ref": summary["reference"], "summary_version": summary["version"], "freshness_before_link": "fresh",
        "uncited_cross_directory_invalidation": "passed", "exact_sources_preserved": True,
        "terminal_outcome": "partial", "original_inputs_retained": len(admitted["inputs"]),
        "read_comparison": measurements, "latency_speed_gate": False}


def cleanup(owner, fixture, real_owner_id, timeout):
    identity = owner.request("GET", "/v1/me")
    fixture_identity(identity, fixture, real_owner_id)
    if identity.get("capabilities") == ["status"]:
        deletion = owner.request("GET", "/v1/account/deletion")
    else:
        deletion = owner.request("POST", "/v1/account/deletion", {
            "confirmation": "DELETE " + fixture["external_ref"], "reason": "Remove isolated Dreamer HTTP release canary."})
    deadline = time.monotonic() + timeout
    while True:
        deletion = owner.request("GET", "/v1/account/deletions/" + deletion["id"])
        status = deletion["status"]
        if status in {"completed", "awaiting_backup_expiry", "failed"} or time.monotonic() >= deadline:
            break
        time.sleep(2)
    result = {key: deletion.get(key) for key in (
        "id", "status", "records_total", "records_completed", "backup_expiry_due_at", "failure_code")}
    result["canonical_purge_verified"] = (status in {"completed", "awaiting_backup_expiry"}
                                          and deletion["records_completed"] == deletion["records_total"])
    result["backup_erasure_verified"] = status == "completed"
    return result


def execute(owner, real_owner_id, args, report, persist):
    require(args.expected_revision and report["preflight"]["build_revision"] == args.expected_revision,
            "production revision must exactly match the explicit deployed revision")
    require(report["preflight"]["review_http_status"] == 200, "new Review API is not deployed")
    phased = args.summary_preference == "disabled_then_enabled"
    require(report["preflight"].get("feature_flags", {}).get("dreamer_summary_reads_enabled") is (not phased),
            "initial summary preference flag differs from the selected canary phase")
    fixture_owner = None
    fixture = {"external_ref": PREFIX + str(uuid.uuid4())}
    report["fixture"] = fixture
    report["cycles"] = []
    persist()
    try:
        if args.fixture_fd is not None:
            require(args.fixture_fd > 2, "fixture credential input must use an inherited nonstandard descriptor")
            with os.fdopen(os.dup(args.fixture_fd), "r") as source:
                provisioned = json.loads(source.read(64_001))
            fixture["external_ref"] = provisioned["user"]["external_ref"]
        else:
            require(args.admin_base is not None, "provisioning requires a trusted private --admin-base or --fixture-fd")
            admin = Client(safe_base(args.admin_base, admin=True), owner.token, report["calls"], "admin")
            identity = admin.request("GET", "/v1/me")
            require(identity["user"]["id"] == real_owner_id and "admin" in identity.get("capabilities", []),
                    "private admin transport resolved to a different owner")
            provisioned = admin.request("POST", "/v1/admin/users", {
                "external_ref": fixture["external_ref"], "display_name": "Disposable Dreamer release canary",
                "credential_name": "Canary cleanup owner"})
        fixture["user_id"] = provisioned["user"]["id"]
        persist()
        # Credentials are not added to report, stdout, files, environment, or argv.
        fixture_owner = Client(owner.base, provisioned["credential"]["token"], report["calls"], "fixture_owner")
        identity = fixture_owner.request("GET", "/v1/me")
        fixture_identity(identity, fixture, real_owner_id, fresh=True)
        dashboard = unwrap(fixture_owner.request("GET", "/v1/workspace/dashboard"))
        require(dashboard["storage"]["text"]["count"] == 0, "fixture workspace must start empty")
        require(fixture_owner.request("GET", "/v1/workspace/notifications?limit=1")["items"] == [],
                "fixture inbox must start empty")
        persist()
        actors = {}
        for access in ("dreamer_runner", "read_only"):
            issued = fixture_owner.request("POST", "/v1/credentials", {"name": "Canary " + access, "access": access})
            if access == "dreamer_runner":
                require(set(issued["capabilities"]) == RUNNER_CAPS, "runner credential authority is not bounded")
            actors[access] = Client(owner.base, issued["token"], report["calls"], "fixture_" + access)
        runner, reader = actors["dreamer_runner"], actors["read_only"]
        forbidden = {"path": "sources/forbidden.md", "content": "Must not be written", "expected_version": 0}
        for actor in (runner, reader):
            actor.request("POST", "/v1/workspace/write", forbidden, expected=(403,))
        runner.request("GET", "/v1/workspace/notifications", expected=(403,))
        runner.request("POST", "/v1/workspace/read", {"requests": [{"path": SOURCE_PATH}]}, expected=(403,))
        write(fixture_owner, "dreams/CONTROL.md", "enabled: true\nmode: full\nadvance_after: 2099-01-01\n", 0)
        first = "The synthetic fixture valve is amber; its state is uncertain."
        second = "The synthetic fixture valve is violet; its state remains uncertain."
        padding = "\n\nSynthetic filler used only to measure fixture response sizes." * 80
        source = write(fixture_owner, SOURCE_PATH, "# Canary source\n\n" + first + padding + "\n", 0)
        enable_gate = (lambda: await_summary_preference(reader, report, persist, args.flag_wait_seconds)) if phased else None
        summary = run_cycle(fixture_owner, runner, reader, source, first, 0, report, enable_gate)
        persist()
        changed = write(fixture_owner, SOURCE_PATH, "# Canary source\n\n" + second + padding + "\n", source["version"])
        for request in ({"ref": source["entry_ref"], "view": "current_state"}, {"ref": summary["reference"], "view": "full"}):
            stale = read_one(reader, **request, max_chars=100_000)
            require(stale.get("representation") == "current_source_fallback" and second in stale["text"]
                    and first not in stale["text"], "stale summary was served after source edit")
        historical = read_one(reader, ref=source["entry_ref"], version=source["version"], view="full", max_chars=100_000)
        require(first in historical["text"] and second not in historical["text"], "exact historical source was replaced")
        report["source_edit_fallback"] = "passed"
        run_cycle(fixture_owner, runner, reader, changed, second, summary["version"], report)
        report["recovery"] = "fresh replacement summary published through a new owner decision"
        persist()
        run_subject_cycle(fixture_owner, runner, reader, report)
        report["status"] = "passed"
    finally:
        if fixture_owner is not None:
            try:
                report["cleanup"] = cleanup(fixture_owner, fixture, real_owner_id, args.cleanup_timeout)
                if not report["cleanup"]["canonical_purge_verified"]:
                    report["status"] = "cleanup_pending"
            except (CanaryError, ValueError, KeyError, TypeError, IndexError, OSError) as error:
                report["cleanup"] = {"status": "unverified", "error": str(error) if isinstance(error, CanaryError)
                                     else "Cleanup response or local input was invalid; details withheld."}
                report["status"] = "cleanup_failed"
        else:
            report["cleanup"] = {"status": "not_provisioned_or_result_uncertain",
                                  "note": "Use the recorded unique external_ref to investigate an uncertain provisioning result."}
        persist()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true", help="explicitly run fixture mutations after deployment")
    parser.add_argument("--expected-revision", help="full deployed API build revision required for execution")
    parser.add_argument("--admin-base", help="trusted existing loopback private API transport, without /v1")
    parser.add_argument("--fixture-fd", type=int, help="preprovisioned disposable user response via inherited descriptor, never a file")
    parser.add_argument("--report", type=Path, required=True, help="new private report path; never contains credentials")
    parser.add_argument("--cleanup-timeout", type=int, default=180, help="bounded canonical-purge polling seconds")
    parser.add_argument("--summary-preference", choices=("enabled", "disabled_then_enabled"), default="enabled",
                        help="optionally verify flag-off behavior, then wait for the release owner to enable it")
    parser.add_argument("--flag-wait-seconds", type=int, default=600, help="bounded flag-rollout GET polling, 1..900 seconds")
    args = parser.parse_args(argv)
    require(1 <= args.cleanup_timeout <= 600, "cleanup timeout must be 1..600 seconds")
    require(1 <= args.flag_wait_seconds <= 900, "flag wait must be 1..900 seconds")
    require(not (args.admin_base and args.fixture_fd is not None), "choose one fixture provisioning transport")
    require(not args.report.exists(), "report already exists; refusing to overwrite prior recovery facts")
    fd = os.open(args.report, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    os.close(fd)
    report = {"schema": "brunn.dreamer-canary.v1", "started_at": stamp(), "mode": "execute" if args.execute else "preflight",
              "status": "started", "calls": [], "model_calls": 0, "installation_registrations": 0}
    def persist():
        with args.report.open("w", encoding="utf-8") as output:
            json.dump(report, output, indent=2, ensure_ascii=False)
            output.write("\n")
    try:
        owner = Client(safe_base("https://brunn.ai/api"), owner_token(), report["calls"], "owner_preflight")
        report["preflight"], real_owner_id = preflight(owner)
        persist()
        if args.execute:
            execute(owner, real_owner_id, args, report, persist)
        else:
            report["status"] = "read_only_preflight_complete"
    except CanaryError as error:
        report["status"] = "failed" if report["status"] not in {"cleanup_failed", "cleanup_pending"} else report["status"]
        report["error"] = str(error)
    except (ValueError, KeyError, TypeError, IndexError, OSError, subprocess.SubprocessError):
        # Raw HTTP objects and subprocess errors may contain tokens. Never print them.
        report["status"] = "failed"
        report["error"] = "unexpected response shape or local input failure; credential-bearing details withheld"
    finally:
        report["finished_at"] = stamp()
        persist()
    print(json.dumps({"status": report["status"], "report": str(args.report), "production_mutations_requested": args.execute}))
    return 0 if report["status"] in {"passed", "read_only_preflight_complete"} else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except CanaryError as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(1)
