#!/usr/bin/env python3
"""Create or update the source-controlled Brunn Datadog monitors."""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


CONFIG = Path(__file__).with_name("brunn-production-monitors.json")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--site", default=os.environ.get("DD_SITE", "datadoghq.com"))
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args()


def replace(value: Any, replacements: dict[str, str]) -> Any:
    if isinstance(value, str):
        for source, target in replacements.items():
            value = value.replace(source, target)
        return value
    if isinstance(value, list):
        return [replace(item, replacements) for item in value]
    if isinstance(value, dict):
        return {key: replace(item, replacements) for key, item in value.items()}
    return value


def request(
    method: str,
    url: str,
    api_key: str,
    app_key: str,
    body: dict[str, Any] | None = None,
) -> tuple[int, Any]:
    encoded = json.dumps(body).encode() if body is not None else None
    for attempt in range(5):
        operation = urllib.request.Request(
            url,
            data=encoded,
            method=method,
            headers={
                "Accept": "application/json",
                "Content-Type": "application/json",
                "DD-API-KEY": api_key,
                "DD-APPLICATION-KEY": app_key,
            },
        )
        try:
            with urllib.request.urlopen(operation, timeout=30) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            payload = json.loads(error.read().decode("utf-8", errors="replace"))
            if error.code != 429 or attempt == 4:
                return error.code, payload
            retry_after = error.headers.get("Retry-After")
            try:
                delay = float(retry_after) if retry_after else 2**attempt
            except ValueError:
                delay = 2**attempt
            time.sleep(max(1.0, min(delay, 60.0)))
    raise AssertionError("unreachable")


def main() -> int:
    args = parse_args()
    environment = os.environ.get("DD_ENV", "production").strip() or "production"
    service = os.environ.get("DD_SERVICE", "brunn").strip() or "brunn"
    notify = os.environ.get("BRUNN_DATADOG_NOTIFY", "").strip()
    notify_suffix = f"\n\n{notify}" if notify else ""
    replacements = {
        "__DD_ENV__": environment,
        "__DD_SERVICE__": service,
        "__NOTIFY__": notify_suffix,
    }
    monitors = replace(json.loads(CONFIG.read_text()), replacements)
    if not isinstance(monitors, list) or not monitors:
        raise SystemExit("monitor configuration must be a non-empty array")

    if args.dry_run:
        for monitor in monitors:
            print(f"{monitor['name']}: {monitor['query']}")
        print(f"validated={len(monitors)} env={environment} service={service}")
        return 0

    api_key = os.environ.get("DD_API_KEY", "")
    app_key = os.environ.get("DD_APP_KEY", "")
    if not api_key or not app_key:
        raise SystemExit("DD_API_KEY and DD_APP_KEY are required")
    base = f"https://api.{args.site}/api/v1/monitor"
    created = 0
    updated = 0

    for monitor in monitors:
        name = monitor["name"]
        status, response = request(
            "POST",
            f"{base}/validate",
            api_key,
            app_key,
            monitor,
        )
        if status != 200:
            errors = response.get("errors", response) if isinstance(response, dict) else response
            print(f"Datadog rejected {name}: HTTP {status}: {errors}", file=sys.stderr)
            return 1
        status, matches = request(
            "GET",
            f"{base}?name={urllib.parse.quote(name)}",
            api_key,
            app_key,
        )
        if status != 200 or not isinstance(matches, list):
            print(f"could not inspect monitor {name}: HTTP {status}", file=sys.stderr)
            return 1
        exact = [item for item in matches if item.get("name") == name]
        if len(exact) > 1:
            print(f"multiple monitors have the exact name {name}", file=sys.stderr)
            return 1
        method = "POST" if not exact else "PUT"
        url = base if not exact else f"{base}/{exact[0]['id']}"
        status, response = request(method, url, api_key, app_key, monitor)
        if status not in {200, 201}:
            errors = response.get("errors", response) if isinstance(response, dict) else response
            print(f"could not configure {name}: HTTP {status}: {errors}", file=sys.stderr)
            return 1
        if method == "POST":
            created += 1
        else:
            updated += 1

    print(f"configured={len(monitors)} created={created} updated={updated}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
