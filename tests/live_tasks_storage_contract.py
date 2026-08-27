#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import os
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from typing import Any


class ContractFailure(RuntimeError):
    pass


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
        request = urllib.request.Request(
            f"{self.base_url}{path}",
            data=payload,
            method=method,
            headers={
                "Accept": "application/json",
                "Authorization": f"Bearer {self.token}",
                **({"Content-Type": "application/json"} if payload is not None else {}),
            },
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
        if not isinstance(parsed, dict):
            raise ContractFailure(f"{method} {path} returned a non-object JSON response")
        return parsed


def data(response: dict[str, Any]) -> dict[str, Any]:
    result = response.get("data", response)
    if not isinstance(result, dict):
        raise ContractFailure("response data was not an object")
    return result


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractFailure(message)


def sourced(value: Any, source: str) -> dict[str, Any]:
    return {
        "value": value,
        "source": source,
        "set_at": "2026-08-27T12:00:00Z",
    }


def task_metadata(task_id: uuid.UUID, status: str) -> dict[str, Any]:
    task: dict[str, Any] = {
        "id": str(task_id),
        "title": "Verify generic task storage",
        "status": sourced(status, "owner"),
        "project": sourced("straylight", "agent:codex"),
        "required_contexts": sourced(["phone", "online"], "owner"),
        "cost_of_delay": sourced(
            {"amount_cents": 700, "per": "week", "since": "2026-08-01"},
            "agent:codex",
        ),
        "provenance": {
            "captured_by": "agent:codex",
            "captured_from": "live_tasks_storage_contract",
            "created_at": "2026-08-27T12:00:00Z",
        },
    }
    if status == "done":
        task["done_at"] = "2026-08-27T12:05:00Z"
    return {
        "_straylight_import": {
            "format": "straylight-workspace-import-manifest@v1"
        },
        "client": {"kind": "task", "schema": "task.v1", "task": task},
    }


def write_task(
    client: Client,
    path: str,
    content: str,
    metadata: dict[str, Any],
    expected_version: int,
) -> dict[str, Any]:
    return client.request(
        "POST",
        "/v1/workspace/write",
        body={
            "path": path,
            "content": content,
            "media_type": "text/markdown",
            "metadata": metadata,
            "expected_version": expected_version,
        },
    )


def read_version(client: Client, entry_ref: str, version: int) -> dict[str, Any]:
    response = client.request(
        "POST",
        "/v1/workspace/read",
        body={
            "requests": [
                {"ref": entry_ref, "version": version, "view": "full", "max_chars": 10000}
            ]
        },
    )
    items = data(response).get("items")
    require(isinstance(items, list) and len(items) == 1, "versioned read omitted task")
    item = items[0]
    require(isinstance(item, dict), "versioned read returned a non-object item")
    return item


def projection_row(
    container: str,
    database_user: str,
    database_name: str,
    task_id: uuid.UUID,
) -> list[str]:
    sql = f"""
    SELECT task.entry_version,task.status,task.cost_amount_cents,task.cost_period,
           cardinality(task.required_contexts),
           (SELECT count(*) FROM straylight.search_chunks AS chunk
            WHERE chunk.user_id=task.user_id AND chunk.entry_id=task.entry_id),
           (SELECT count(*) FROM straylight.jobs AS job
            WHERE job.user_id=task.user_id
              AND job.payload->>'entry_id'=task.entry_id::text)
    FROM straylight.task_index AS task
    WHERE task.task_id='{task_id}'::uuid
    """
    completed = subprocess.run(
        [
            "docker",
            "exec",
            container,
            "psql",
            "-U",
            database_user,
            "-d",
            database_name,
            "-At",
            "-F",
            "\t",
            "-c",
            sql,
        ],
        text=True,
        capture_output=True,
        check=False,
    )
    require(completed.returncode == 0, f"projection query failed: {completed.stderr[-500:]}")
    rows = [line.split("\t") for line in completed.stdout.splitlines() if line.strip()]
    require(len(rows) == 1, f"expected one projected task row, got {rows!r}")
    return rows[0]


def run(args: argparse.Namespace) -> dict[str, Any]:
    started = time.monotonic()
    client = Client(args.base_url, args.token)
    task_id = uuid.uuid4()
    task_path = f".straylight/tasks/{task_id}.md"
    exact_content = "# Verify generic task storage\n\nExact task bytes: Téléphone.\n"

    first = data(
        write_task(client, task_path, exact_content, task_metadata(task_id, "open"), 0)
    )
    require(first.get("version") == 1, "generic task import did not create version 1")
    entry_ref = first.get("entry_ref")
    require(isinstance(entry_ref, str), "task import omitted entry_ref")

    second = data(
        write_task(client, task_path, exact_content, task_metadata(task_id, "done"), 1)
    )
    require(second.get("version") == 2, "metadata-only task mutation did not create version 2")

    changes = data(
        client.request("GET", "/v1/workspace/changes?since_generation=0&limit=200")
    ).get("changes")
    require(isinstance(changes, list), "memory.changes omitted its change list")
    task_changes = [
        change
        for change in changes
        if isinstance(change, dict) and change.get("path") == task_path
    ]
    require(
        [change.get("version") for change in task_changes] == [1, 2],
        f"memory.changes did not retain task versions 1 and 2: {task_changes!r}",
    )

    first_read = read_version(client, entry_ref, 1)
    second_read = read_version(client, entry_ref, 2)
    require(first_read.get("text") == exact_content, "version 1 task bytes changed")
    require(second_read.get("text") == exact_content, "version 2 task bytes changed")
    require(first_read.get("metadata", {}).get("client", {}).get("task", {}).get("status", {}).get("value") == "open", "version 1 metadata was not preserved")
    require(second_read.get("metadata", {}).get("client", {}).get("task", {}).get("status", {}).get("value") == "done", "version 2 metadata was not preserved")

    row = projection_row(
        args.database_container,
        args.database_user,
        args.database_name,
        task_id,
    )
    require(
        row == ["2", "done", "700", "week", "2", "0", "0"],
        f"generic import produced an invalid projection/chunk/job row: {row!r}",
    )

    with tempfile.TemporaryDirectory(prefix="straylight-task-history-") as temporary:
        output = Path(temporary) / "export"
        exported = subprocess.run(
            [
                str(args.carrystate),
                "workspace",
                "export",
                "--output",
                str(output),
                "--history",
            ],
            env={
                **os.environ,
                "CARRYSTATE_API_URL": args.base_url,
                "CARRYSTATE_API_TOKEN": args.token,
            },
            text=True,
            capture_output=True,
            check=False,
        )
        require(exported.returncode == 0, f"history export failed: {exported.stderr[-1000:]}")
        manifest = json.loads((output / "manifest.json").read_text(encoding="utf-8"))
        task_entries = [
            entry
            for entry in manifest["entries"]
            if entry["path"] == task_path and entry["archive_path"].startswith("history/")
        ]
        require(
            [entry["version"] for entry in task_entries] == [1, 2],
            f"history export omitted task versions: {task_entries!r}",
        )
        for entry in task_entries:
            exported_bytes = (output / entry["archive_path"]).read_bytes()
            require(exported_bytes == exact_content.encode("utf-8"), "history export changed task bytes")
        require(
            (output / "workspace" / task_path).read_bytes() == exact_content.encode("utf-8"),
            "current workspace export changed task bytes",
        )

    return {
        "schema": "straylight-live-tasks-storage-contract@v1",
        "status": "pass",
        "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
        "task_id": str(task_id),
        "entry_ref": entry_ref,
        "checks": {
            "generic_portable_import": True,
            "projection_rebuilt": True,
            "metadata_only_version": True,
            "no_chunks_or_jobs": True,
            "memory_changes_versions": [1, 2],
            "exact_version_reads": [1, 2],
            "cli_history_export_versions": [1, 2],
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://127.0.0.1:18112")
    parser.add_argument("--token", required=True)
    parser.add_argument("--database-container", default="straylight-task-m1-db")
    parser.add_argument("--database-user", default="admin")
    parser.add_argument("--database-name", default="straylight")
    parser.add_argument(
        "--carrystate",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "apps"
        / "api"
        / "target"
        / "debug"
        / "carrystate",
    )
    args = parser.parse_args()
    try:
        result = run(args)
    except (ContractFailure, KeyError, OSError, ValueError) as error:
        result = {
            "schema": "straylight-live-tasks-storage-contract@v1",
            "status": "fail",
            "error": str(error),
        }
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
