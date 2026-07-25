#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.parse
from pathlib import Path
from typing import Any

from native_eval import NativeApiClient, NativeApiError
from native_memory import (
    locked_state,
    record_state,
    render_json,
    response_payload_metrics,
)


def load_payload(value: str | None) -> dict[str, Any] | None:
    if value is None:
        return None
    raw = Path(value[1:]).read_text(encoding="utf-8") if value.startswith("@") else value
    payload = json.loads(raw)
    if not isinstance(payload, dict):
        raise ValueError("HTTP request payload must be a JSON object")
    return payload


def download_identity(path: str) -> tuple[str | None, int | None]:
    parsed = urllib.parse.urlparse(path)
    match = re.search(
        r"/v1/assets/([^/]+)/versions/([1-9][0-9]*)/content$",
        parsed.path,
    )
    if match is None:
        return None, None
    return urllib.parse.unquote(match.group(1)), int(match.group(2))


def operation_name(method: str, path: str) -> str:
    leaf = path.rstrip("/").rsplit("/", 1)[-1]
    return f"http.{method.casefold()}.{leaf or 'root'}"


def response_status(body: dict[str, Any]) -> str | None:
    status = body.get("status")
    return str(status) if status is not None else None


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Thin authenticated HTTP client for the CarryState evaluation surface",
    )
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--case-id", required=True)
    parser.add_argument("--pretty", action="store_true")
    parser.add_argument("method", choices=("get", "post", "download"))
    parser.add_argument("path")
    parser.add_argument("payload", nargs="?")
    parser.add_argument(
        "--sha256",
        help="Expected SHA-256 from session-pinned asset metadata (download only)",
    )
    parser.add_argument(
        "--size",
        type=int,
        help="Expected byte count from session-pinned asset metadata (download only)",
    )
    args = parser.parse_args()

    if args.method == "download":
        if not args.payload:
            parser.error("download requires an output path")
        if args.sha256 is None or args.size is None:
            parser.error("download requires --sha256 and --size from asset metadata")
        output = Path(args.payload)
        if not output.is_absolute():
            output = Path.cwd() / output
        output = output.resolve()
        run_root = args.state.parent.resolve()
        try:
            output.relative_to(run_root)
        except ValueError:
            parser.error("download output must remain inside the evaluation run directory")

        client = NativeApiClient(run_id=args.run_id, case_id=args.case_id)
        with locked_state(args.state) as state:
            try:
                downloaded = client.download_verified(
                    args.path,
                    output,
                    expected_hash=args.sha256,
                    expected_size=args.size,
                )
            except NativeApiError as error:
                rendered = render_json(error.body, pretty=args.pretty)
                record_state(
                    args.state,
                    state,
                    "http.download.asset",
                    result_chars=len(rendered),
                    elapsed_ms=error.elapsed_ms,
                    http_status=error.status,
                    request_id=None,
                    service_status=response_status(error.body),
                    result_metrics=response_payload_metrics(rendered),
                )
                print(rendered)
                raise SystemExit(1) from error
            except (OSError, RuntimeError, ValueError) as error:
                body = {
                    "error": {
                        "code": "asset_download_verification_failed",
                        "message": str(error),
                    }
                }
                rendered = render_json(body, pretty=args.pretty)
                record_state(
                    args.state,
                    state,
                    "http.download.asset",
                    result_chars=len(rendered),
                    elapsed_ms=0,
                    http_status=0,
                    request_id=None,
                    service_status="asset_download_verification_failed",
                    result_metrics=response_payload_metrics(rendered),
                )
                print(rendered)
                raise SystemExit(1) from error

            body = {
                "status": "complete",
                "local_path": str(downloaded.path),
                "content_hash": downloaded.content_hash,
                "size_bytes": downloaded.size_bytes,
                "media_type": downloaded.media_type,
            }
            rendered = render_json(body, pretty=args.pretty)
            asset_ref, asset_version = download_identity(args.path)
            record_state(
                args.state,
                state,
                "http.download.asset",
                result_chars=len(rendered),
                elapsed_ms=downloaded.elapsed_ms,
                http_status=200,
                request_id=None,
                service_status="complete",
                result_metrics={
                    **response_payload_metrics(rendered),
                    "binary_bytes": downloaded.size_bytes,
                    "http_calls": 1,
                    "asset_content_hash": downloaded.content_hash,
                    "asset_local_path": str(downloaded.path),
                    "asset_ref": asset_ref,
                    "asset_version": asset_version,
                    "asset_size_bytes": downloaded.size_bytes,
                },
            )
            sys.stdout.write(rendered + "\n")
        return

    payload = load_payload(args.payload)
    if args.method == "get" and payload is not None:
        raise ValueError("GET requests do not accept a payload")
    if args.method == "post" and payload is None:
        payload = {}

    client = NativeApiClient(run_id=args.run_id, case_id=args.case_id)
    with locked_state(args.state) as state:
        try:
            response = client.request(args.method.upper(), args.path, payload)
        except NativeApiError as error:
            rendered = render_json(error.body, pretty=args.pretty)
            record_state(
                args.state,
                state,
                operation_name(args.method, args.path),
                result_chars=len(rendered),
                elapsed_ms=error.elapsed_ms,
                http_status=error.status,
                request_id=None,
                service_status=response_status(error.body),
                result_metrics=response_payload_metrics(rendered),
            )
            print(rendered)
            raise SystemExit(1) from error

        rendered = render_json(response.body, pretty=args.pretty)
        record_state(
            args.state,
            state,
            operation_name(args.method, args.path),
            result_chars=len(rendered),
            elapsed_ms=response.elapsed_ms,
            http_status=response.http_status,
            request_id=response.headers.get("x-request-id")
            or str(response.body.get("request_id") or "")
            or None,
            service_status=response_status(response.body),
            response=response,
            result_metrics=response_payload_metrics(rendered),
        )
        sys.stdout.write(rendered + "\n")


if __name__ == "__main__":
    main()
