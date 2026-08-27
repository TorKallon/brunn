#!/usr/bin/env python3

"""Gate-12b guard flow through a live API and the real guard CLI."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from typing import Any


SCHEMA = "straylight-task-guard-gate12@v1"


class ContractFailure(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractFailure(message)


class Client:
    def __init__(self, base_url: str, token: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token

    def request(
        self,
        method: str,
        path: str,
        *,
        body: Any | None = None,
        expected: int = 200,
    ) -> dict[str, Any]:
        payload = None if body is None else json.dumps(body).encode("utf-8")
        headers = {"accept": "application/json", "authorization": f"Bearer {self.token}"}
        if payload is not None:
            headers["content-type"] = "application/json"
        request = urllib.request.Request(
            f"{self.base_url}{path}", data=payload, method=method, headers=headers
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                status = response.status
                raw = response.read()
        except urllib.error.HTTPError as error:
            status = error.code
            raw = error.read()
        parsed = json.loads(raw) if raw else {}
        if status != expected:
            raise ContractFailure(
                f"{method} {path} returned HTTP {status}, expected {expected}: "
                f"{json.dumps(parsed, sort_keys=True)[:1000]}"
            )
        require(isinstance(parsed, dict), f"{method} {path} returned non-object JSON")
        return parsed


def data(response: dict[str, Any]) -> dict[str, Any]:
    value = response.get("data", response)
    require(isinstance(value, dict), "response data was not an object")
    return value


def first_value(value: Any, key: str) -> Any | None:
    if isinstance(value, dict):
        if key in value:
            return value[key]
        for nested in value.values():
            found = first_value(nested, key)
            if found is not None:
                return found
    elif isinstance(value, list):
        for nested in value:
            found = first_value(nested, key)
            if found is not None:
                return found
    return None


def capture_task(
    client: Client,
    suffix: str,
    label: str,
    *,
    title: str,
    due: str,
    source: str,
    note: str | None = None,
) -> str:
    hard_due: dict[str, Any] = {"value": due, "source": source}
    if note is not None:
        hard_due["note"] = note
    response = client.request(
        "POST",
        "/v1/workspace/tasks/capture",
        body={
            "idempotency_key": f"guard-gate12:{suffix}:capture:{label}",
            "items": [
                {
                    "client_ref": f"guard-{suffix}-{label}",
                    "captured_from": "gate12b:live-guard",
                    "raw_text": title,
                    "title": title,
                    "hard_due": hard_due,
                }
            ],
        },
    )
    task_ref = first_value(response, "task_ref")
    require(isinstance(task_ref, str), f"capture {label} omitted task_ref")
    parsed = uuid.UUID(task_ref)
    require(parsed.version == 7 and str(parsed) == task_ref, "task_ref was not canonical UUIDv7")
    return task_ref


def run_guard(binary: Path, as_of: str) -> dict[str, Any]:
    completed = subprocess.run(
        [str(binary), "task-guard-once", "--as-of", as_of],
        env=os.environ,
        text=True,
        capture_output=True,
        check=False,
        timeout=60,
    )
    if completed.returncode != 0:
        raise ContractFailure(f"task guard CLI failed: {completed.stderr[-1500:]}")
    parsed: dict[str, Any] | None = None
    for line in reversed(completed.stdout.splitlines()):
        try:
            candidate = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(candidate, dict) and "events" in candidate:
            parsed = candidate
            break
    require(parsed is not None, f"guard CLI omitted JSON report: {completed.stdout[-1000:]}")
    return parsed


def task_events(report: dict[str, Any], task_ref: str) -> list[dict[str, Any]]:
    events = report.get("events")
    require(isinstance(events, list), "guard report omitted events")
    return [
        event
        for event in events
        if isinstance(event, dict) and event.get("task_id") == task_ref
    ]


def notification_for_task(client: Client, task_ref: str) -> dict[str, Any]:
    listed = client.request("GET", "/v1/workspace/notifications?limit=100")
    items = listed.get("items")
    require(isinstance(items, list), "notification list omitted items")
    matches = [
        item
        for item in items
        if isinstance(item, dict)
        and item.get("target") == {"type": "task", "task_ref": task_ref}
    ]
    require(matches, f"no typed task notification found for {task_ref}")
    notification_ref = matches[0].get("notification_ref")
    require(isinstance(notification_ref, str), "notification omitted reference")
    detail = client.request("GET", f"/v1/workspace/notifications/{notification_ref}")
    notification = detail.get("notification")
    require(isinstance(notification, dict), "notification detail omitted notification")
    require(
        notification.get("target") == {"type": "task", "task_ref": task_ref},
        "notification detail changed typed target",
    )
    return notification


def update_settings(client: Client, suffix: str) -> None:
    settings = data(client.request("GET", "/v1/workspace/tasks/settings")).get("settings")
    require(isinstance(settings, dict), "task settings GET omitted settings")
    version = settings.get("version")
    require(isinstance(version, int), "task settings omitted version")
    client.request(
        "PUT",
        "/v1/workspace/tasks/settings",
        body={
            "expected_version": version,
            "idempotency_key": f"guard-gate12:{suffix}:settings",
            "timezone": "UTC",
            "hard_lead_days": 7,
            "hard_second_lead_hours": 48,
            "due_day_local_time": "07:00",
            "quiet_hours_start": "22:00",
            "quiet_hours_end": "07:00",
            "quiet_override_enabled": True,
            "quiet_override_within_hours": 24,
        },
    )


def run(args: argparse.Namespace) -> dict[str, Any]:
    started = time.monotonic()
    client = Client(args.base_url, args.token)
    suffix = uuid.uuid4().hex[:12]
    checks: list[dict[str, Any]] = []

    def check(name: str, action: Any) -> Any:
        before = time.monotonic()
        result = action()
        checks.append(
            {"name": name, "elapsed_ms": round((time.monotonic() - before) * 1000, 3)}
        )
        return result

    check("settings.quiet_hours", lambda: update_settings(client, suffix))
    explicit = check(
        "task.capture.explicit",
        lambda: capture_task(
            client,
            suffix,
            "explicit",
            title="Gate 12 explicit deadline",
            due="2026-09-04T12:00:00Z",
            source="owner",
        ),
    )
    first = check("guard.seven_day", lambda: run_guard(args.binary, "2026-08-28T12:00:00Z"))
    first_events = task_events(first, explicit)
    require(len(first_events) == 1, f"seven-day run emitted {first_events!r}")
    event = first_events[0]
    require(event.get("inserted") is True, "first lead event was not inserted")
    require(event.get("event_key") == f"task-deadline:{explicit}:7d", "lead key changed")
    require(event.get("route") == f"straylight://task/{explicit}", "typed route changed")
    notification_for_task(client, explicit)
    checks.append({"name": "notification.typed_target", "elapsed_ms": 0})

    replay = check("guard.replay", lambda: run_guard(args.binary, "2026-08-28T12:00:00Z"))
    replay_events = task_events(replay, explicit)
    require(len(replay_events) == 1 and replay_events[0].get("inserted") is False, "lead replay was not deduped")
    listed = client.request("GET", "/v1/workspace/notifications?limit=100").get("items", [])
    explicit_notifications = [
        item
        for item in listed
        if isinstance(item, dict)
        and item.get("target") == {"type": "task", "task_ref": explicit}
    ]
    require(len(explicit_notifications) == 1, "event-key replay created duplicate inbox rows")

    quiet = check(
        "task.capture.quiet",
        lambda: capture_task(
            client,
            suffix,
            "quiet",
            title="Gate 12 quiet explicit deadline",
            due="2026-08-30T12:00:00Z",
            source="owner",
        ),
    )
    inferred = check(
        "task.capture.inferred",
        lambda: capture_task(
            client,
            suffix,
            "inferred",
            title="Gate 12 inferred deadline",
            due="2026-08-29T12:00:00Z",
            source="agent:gate12",
            note="inferred — confirm?",
        ),
    )
    quiet_report = check(
        "guard.quiet_time_travel", lambda: run_guard(args.binary, "2026-08-28T23:00:00Z")
    )
    quiet_events = task_events(quiet_report, quiet)
    require(quiet_events and all(event.get("quiet_delayed") for event in quiet_events), "quiet explicit deadline was not delayed")
    inferred_events = task_events(quiet_report, inferred)
    require(inferred_events, "inferred deadline emitted no lead event")
    require(all(event.get("inferred") is True for event in inferred_events), "inferred marker missing")
    require(all(event.get("quiet_delayed") is True for event in inferred_events), "inferred deadline broke quiet hours")
    require(
        all(str(event.get("delivery_available_at", "")).startswith("2026-08-29T07:00:00") for event in inferred_events),
        "inferred delivery did not delay to quiet-hours end",
    )
    inferred_notification = notification_for_task(client, inferred)
    require("inferred" in str(inferred_notification.get("body", "")).lower(), "inbox copy omitted inferred confirmation marker")
    checks.append({"name": "guard.inferred_never_breaks_quiet", "elapsed_ms": 0})

    return {
        "schema": SCHEMA,
        "status": "pass",
        "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
        "checks": checks,
        "summary": {
            "dedupe": "one_inbox_row_per_event_key",
            "typed_target": {"type": "task", "route": "straylight://task/<uuidv7>"},
            "quiet_explicit": "delayed",
            "quiet_inferred": "delayed_with_confirm_marker",
        },
    }


def parse_args() -> argparse.Namespace:
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--token", default=os.environ.get("STRAYLIGHT_GATE12_OWNER_TOKEN"))
    parser.add_argument(
        "--binary", type=Path, default=root / "apps/api/target/debug/straylight"
    )
    parser.add_argument(
        "--artifact", type=Path, default=root / "release-artifacts/task-gate12/guard.json"
    )
    args = parser.parse_args()
    require(isinstance(args.token, str) and bool(args.token), "--token is required")
    args.binary = args.binary.resolve()
    args.artifact = args.artifact.resolve()
    return args


def main() -> int:
    args = parse_args()
    try:
        evidence = run(args)
    except Exception as error:
        evidence = {
            "schema": SCHEMA,
            "status": "fail",
            "elapsed_ms": 0,
            "checks": [],
            "error": f"{type(error).__name__}: {error}",
        }
    args.artifact.parent.mkdir(parents=True, exist_ok=True)
    args.artifact.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(evidence, indent=2, sort_keys=True))
    return 0 if evidence.get("status") == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
