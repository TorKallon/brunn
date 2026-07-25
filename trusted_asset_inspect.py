#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import os
import urllib.request
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Ask the evaluation supervisor to verify a locally inspected asset",
    )
    parser.add_argument("path", type=Path)
    parser.add_argument(
        "--sqlite-query",
        help="Run one bounded read-only SELECT through the evaluation supervisor",
    )
    args = parser.parse_args()
    endpoint = os.environ.get("CARRYSTATE_INSPECT_URL")
    capability = os.environ.get("CARRYSTATE_INSPECT_CAPABILITY")
    if not endpoint or not capability:
        raise SystemExit("trusted inspection supervisor is not available")
    local_path = args.path if args.path.is_absolute() else Path.cwd() / args.path
    request = urllib.request.Request(
        endpoint,
        data=json.dumps({
            "capability": capability,
            "path": str(local_path),
            "operation": "sqlite_query" if args.sqlite_query else "inspect",
            "query": args.sqlite_query,
        }).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        payload = response.read()
    print(json.dumps(json.loads(payload), indent=2))


if __name__ == "__main__":
    main()
