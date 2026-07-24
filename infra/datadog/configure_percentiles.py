#!/usr/bin/env python3
"""Enable Datadog percentiles and bounded queryable tags for distributions."""

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


CONFIG = Path(__file__).with_name("distribution-percentiles.json")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--site", default=os.environ.get("DD_SITE", "datadoghq.com"))
    parser.add_argument(
        "--strict",
        action="store_true",
        help="fail if a configured distribution has not reported yet",
    )
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args()


def request(
    method: str,
    url: str,
    api_key: str,
    app_key: str,
    body: dict | None = None,
) -> tuple[int, dict]:
    data = json.dumps(body).encode() if body is not None else None
    for attempt in range(5):
        request = urllib.request.Request(
            url,
            data=data,
            method=method,
            headers={
                "Accept": "application/json",
                "Content-Type": "application/json",
                "DD-API-KEY": api_key,
                "DD-APPLICATION-KEY": app_key,
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            payload = json.loads(error.read().decode("utf-8", errors="replace"))
            if error.code != 429 or attempt == 4:
                return error.code, payload
            reset = error.headers.get("Retry-After") or error.headers.get(
                "X-RateLimit-Reset"
            )
            try:
                delay = float(reset) if reset is not None else 2**attempt
            except ValueError:
                delay = 2**attempt
            time.sleep(max(1.0, min(delay, 60.0)))
    raise AssertionError("unreachable")


def body(metric: str, tags: list[str], include_type: bool) -> dict:
    attributes: dict[str, object] = {
        "include_percentiles": True,
        "tags": tags,
    }
    if include_type:
        attributes["metric_type"] = "distribution"
    return {
        "data": {
            "type": "manage_tags",
            "id": metric,
            "attributes": attributes,
        }
    }


def main() -> int:
    args = parse_args()
    api_key = os.environ.get("DD_API_KEY", "")
    app_key = os.environ.get("DD_APP_KEY", "")
    if not args.dry_run and (not api_key or not app_key):
        raise SystemExit("DD_API_KEY and DD_APP_KEY are required")
    configurations: dict[str, list[str]] = json.loads(CONFIG.read_text())
    base = f"https://api.{args.site}/api/v2/metrics"
    configured: list[str] = []
    missing: list[str] = []

    for metric, tags in configurations.items():
        if args.dry_run:
            print(f"would configure {metric}: {','.join(tags)}")
            continue
        encoded = urllib.parse.quote(metric, safe="")
        url = f"{base}/{encoded}/tags"
        status, response = request("GET", url, api_key, app_key)
        if status not in {200, 404}:
            print(
                f"failed to inspect {metric}: HTTP {status}: "
                f"{response.get('errors', [])}",
                file=sys.stderr,
            )
            return 1
        attributes = response.get("data", {}).get("attributes", {})
        if (
            status == 200
            and attributes.get("include_percentiles")
            and set(attributes.get("tags", [])) == set(tags)
        ):
            configured.append(metric)
            continue
        method = "PATCH" if status == 200 else "POST"
        status, response = request(
            method,
            url,
            api_key,
            app_key,
            body(metric, tags, include_type=method == "POST"),
        )
        if method == "POST" and status == 409:
            status, response = request(
                "PATCH",
                url,
                api_key,
                app_key,
                body(metric, tags, include_type=False),
            )
        errors = response.get("errors", [])
        if status == 400 and any("does not exist" in error for error in errors):
            missing.append(metric)
            continue
        if status not in {200, 201}:
            print(
                f"failed to configure {metric}: HTTP {status}: {errors}",
                file=sys.stderr,
            )
            return 1
        attributes = response.get("data", {}).get("attributes", {})
        if not attributes.get("include_percentiles"):
            print(f"percentiles remain disabled for {metric}", file=sys.stderr)
            return 1
        configured.append(metric)

    print(f"configured={len(configured)} missing={len(missing)}")
    for metric in missing:
        print(f"not reporting yet: {metric}")
    if args.strict and missing:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
