#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
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


def uuid7() -> uuid.UUID:
    """Return a canonical RFC 9562 UUIDv7 without depending on Python 3.14."""
    timestamp_ms = int(time.time() * 1000) & ((1 << 48) - 1)
    randomness = int.from_bytes(os.urandom(10), "big")
    random_a = (randomness >> 68) & ((1 << 12) - 1)
    random_b = randomness & ((1 << 62) - 1)
    value = (
        (timestamp_ms << 80)
        | (0x7 << 76)
        | (random_a << 64)
        | (0b10 << 62)
        | random_b
    )
    generated = uuid.UUID(int=value)
    require(generated.version == 7, "generated task id is not UUIDv7")
    require(generated.variant == uuid.RFC_4122, "generated task id has the wrong variant")
    require(str(generated) == str(generated).lower(), "generated task id is not lowercase")
    return generated


def sourced(value: Any, source: str) -> dict[str, Any]:
    return {
        "value": value,
        "source": source,
        "set_at": "2026-08-27T08:00:00Z",
    }


def task_metadata(task_id: uuid.UUID, status: str) -> dict[str, Any]:
    task: dict[str, Any] = {
        "id": str(task_id),
        "title": "Verify generic task storage",
        "status": sourced(status, "owner"),
        "project": sourced("brunn", "agent:codex"),
        "required_contexts": sourced(["phone", "online"], "owner"),
        "cost_of_delay": sourced(
            {"amount_cents": 700, "per": "week", "since": "2026-08-01"},
            "agent:codex",
        ),
        "provenance": {
            "captured_by": "agent:codex",
            "captured_from": "live_tasks_storage_contract",
            "created_at": "2026-08-27T08:00:00Z",
        },
    }
    if status == "done":
        task["done_at"] = "2026-08-27T08:05:00Z"
    return {
        "_brunn_import": {
            "format": "brunn-workspace-import-manifest@v1"
        },
        "portable": {"modified_unix_ns": None, "mode": None},
        "client": {"kind": "task", "schema": "task.v1", "task": task},
    }


def portable_import_idempotency_key(path: str, content: str) -> str:
    content_hash = "sha256:" + hashlib.sha256(content.encode("utf-8")).hexdigest()
    identity = f"{path}\0{content_hash}\0".encode("utf-8")
    return "workspace-import:" + hashlib.sha256(identity).hexdigest()[:32]


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
            "idempotency_key": portable_import_idempotency_key(path, content),
        },
    )


def provision_user(admin: Client, run_id: str, label: str) -> tuple[Client, str, str]:
    external_ref = f"contract:task-storage:{run_id}:{label}"
    created = data(
        admin.request(
            "POST",
            "/v1/admin/users",
            body={
                "external_ref": external_ref,
                "display_name": f"Task storage contract {label}",
                "credential_name": f"Task storage contract {label} owner",
            },
        )
    )
    user = created.get("user")
    credential = created.get("credential")
    require(isinstance(user, dict), f"{label} provisioning omitted user")
    require(isinstance(credential, dict), f"{label} provisioning omitted credential")
    user_ref = user.get("id")
    token = credential.get("token")
    require(
        isinstance(user_ref, str) and user_ref.startswith("user:"),
        f"{label} provisioning omitted user reference",
    )
    require(isinstance(token, str) and token, f"{label} provisioning omitted token")
    return Client(admin.base_url, token), user_ref, external_ref


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
    external_ref: str,
) -> list[str]:
    sql = f"""
    SELECT task.entry_version,task.status,task.cost_amount_cents,task.cost_period,
           cardinality(task.required_contexts),
           (SELECT count(*) FROM brunn.search_chunks AS chunk
            WHERE chunk.user_id=task.user_id AND chunk.entry_id=task.entry_id),
           (SELECT count(*) FROM brunn.jobs AS job
            WHERE job.user_id=task.user_id
              AND job.payload->>'entry_id'=task.entry_id::text)
    FROM brunn.task_index AS task
    JOIN brunn.users AS owner ON owner.id=task.user_id
    WHERE task.task_id='{task_id}'::uuid
      AND owner.external_ref='{external_ref}'
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


def run_cli(
    args: argparse.Namespace,
    token: str,
    *command: str,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        [str(args.brunn_state), *command],
        env={
            **os.environ,
            "BRUNN_STATE_API_URL": args.base_url,
            "BRUNN_STATE_API_TOKEN": token,
        },
        text=True,
        capture_output=True,
        check=False,
    )
    require(
        completed.returncode == 0,
        f"brunn-state {' '.join(command[:2])} failed: {completed.stderr[-1000:]}",
    )
    return completed


def task_history_entries(
    manifest: dict[str, Any], task_path: str
) -> list[dict[str, Any]]:
    entries = manifest.get("entries")
    require(isinstance(entries, list), "export manifest omitted entries")
    history = [
        entry
        for entry in entries
        if isinstance(entry, dict)
        and entry.get("path") == task_path
        and isinstance(entry.get("archive_path"), str)
        and entry["archive_path"].startswith("history/")
    ]
    return history


def task_current_entry(
    manifest: dict[str, Any], task_path: str
) -> dict[str, Any]:
    entries = manifest.get("entries")
    require(isinstance(entries, list), "export manifest omitted entries")
    current = [
        entry
        for entry in entries
        if isinstance(entry, dict)
        and entry.get("path") == task_path
        and entry.get("archive_path") == f"workspace/{task_path}"
    ]
    require(len(current) == 1, f"expected one current task export, got {current!r}")
    return current[0]


def write_checksums(root: Path) -> None:
    files = sorted(
        path
        for path in root.rglob("*")
        if path.is_file() and path.name != "CHECKSUMS.sha256"
    )
    lines = [
        f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(root).as_posix()}"
        for path in files
    ]
    (root / "CHECKSUMS.sha256").write_text("\n".join(lines) + "\n", encoding="utf-8")


def replay_history_through_cli(
    args: argparse.Namespace,
    token: str,
    source_export: Path,
    manifest: dict[str, Any],
    task_path: str,
    history: list[dict[str, Any]],
    scratch: Path,
) -> list[int]:
    imported_versions: list[int] = []
    for entry in history:
        version = entry.get("version")
        require(isinstance(version, int) and version > 0, "history entry has invalid version")
        stage = scratch / f"history-v{version:020d}"
        workspace_file = stage / "workspace" / task_path
        workspace_file.parent.mkdir(parents=True)
        shutil.copyfile(source_export / str(entry["archive_path"]), workspace_file)
        portable_entry = dict(entry)
        portable_entry["archive_path"] = f"workspace/{task_path}"
        stage_manifest = {
            "format": manifest.get("format"),
            "version": manifest.get("version"),
            "workspace_generation": manifest.get("workspace_generation"),
            "workspace_generation_is_snapshot": manifest.get(
                "workspace_generation_is_snapshot", False
            ),
            "entries": [portable_entry],
        }
        (stage / "manifest.json").write_text(
            json.dumps(stage_manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        write_checksums(stage)
        run_cli(
            args,
            token,
            "workspace",
            "import",
            "--root",
            str(stage),
            "--state-dir",
            str(scratch / "import-state"),
            "--describe-binaries",
            "false",
        )
        imported_versions.append(version)
    return imported_versions


def exported_task_bytes(
    root: Path, history: list[dict[str, Any]]
) -> dict[int, bytes]:
    result: dict[int, bytes] = {}
    for entry in history:
        version = entry.get("version")
        archive_path = entry.get("archive_path")
        require(isinstance(version, int), "history entry version was not an integer")
        require(isinstance(archive_path, str), "history entry omitted archive path")
        result[version] = (root / archive_path).read_bytes()
    return result


def history_identity(history: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [
        {
            "version": entry.get("version"),
            "content_hash": entry.get("content_hash"),
            "size_bytes": entry.get("size_bytes"),
            "media_type": entry.get("media_type"),
            "metadata": entry.get("metadata"),
            "current": entry.get("current"),
            "deleted": entry.get("deleted"),
        }
        for entry in history
    ]


def run(args: argparse.Namespace) -> dict[str, Any]:
    started = time.monotonic()
    admin = Client(args.base_url, args.token)
    run_id = str(uuid7())
    source, source_user_ref, source_external_ref = provision_user(admin, run_id, "source")
    target, target_user_ref, target_external_ref = provision_user(admin, run_id, "target")
    require(source_user_ref != target_user_ref, "source and target reused one identity")

    task_id = uuid7()
    task_path = f".brunn/tasks/{task_id}.md"
    require(
        str(task_id) == str(task_id).lower() and task_id.version == 7,
        "task path does not use a canonical lowercase UUIDv7",
    )
    version_content = {
        1: "# Verify generic task storage\n\nExact task bytes v1: Téléphone.\n",
        2: "# Verify generic task storage\n\nExact task bytes v2: Téléphone terminé.\n",
    }

    first = data(
        write_task(
            source,
            task_path,
            version_content[1],
            task_metadata(task_id, "open"),
            0,
        )
    )
    require(first.get("version") == 1, "generic task import did not create version 1")
    entry_ref = first.get("entry_ref")
    require(isinstance(entry_ref, str), "task import omitted entry_ref")

    second = data(
        write_task(
            source,
            task_path,
            version_content[2],
            task_metadata(task_id, "done"),
            1,
        )
    )
    require(second.get("version") == 2, "task mutation did not create version 2")

    changes = data(
        source.request("GET", "/v1/workspace/changes?since_generation=0&limit=200")
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

    first_read = read_version(source, entry_ref, 1)
    second_read = read_version(source, entry_ref, 2)
    require(first_read.get("text") == version_content[1], "version 1 task bytes changed")
    require(second_read.get("text") == version_content[2], "version 2 task bytes changed")
    require(first_read.get("metadata", {}).get("client", {}).get("task", {}).get("status", {}).get("value") == "open", "version 1 metadata was not preserved")
    require(second_read.get("metadata", {}).get("client", {}).get("task", {}).get("status", {}).get("value") == "done", "version 2 metadata was not preserved")

    row = projection_row(
        args.database_container,
        args.database_user,
        args.database_name,
        task_id,
        source_external_ref,
    )
    require(
        row == ["2", "done", "700", "week", "2", "0", "0"],
        f"generic import produced an invalid projection/chunk/job row: {row!r}",
    )

    with tempfile.TemporaryDirectory(prefix="brunn-task-history-") as temporary:
        temporary_root = Path(temporary)
        source_export = temporary_root / "source-export"
        run_cli(
            args,
            source.token,
            "workspace",
            "export",
            "--output",
            str(source_export),
            "--history",
        )
        source_manifest = json.loads(
            (source_export / "manifest.json").read_text(encoding="utf-8")
        )
        source_history = task_history_entries(source_manifest, task_path)
        require(
            [entry["version"] for entry in source_history] == [1, 2],
            f"history export omitted task versions: {source_history!r}",
        )
        source_bytes = exported_task_bytes(source_export, source_history)
        require(
            source_bytes
            == {
                version: content.encode("utf-8")
                for version, content in version_content.items()
            },
            "source history export changed exact task bytes",
        )
        source_current = task_current_entry(source_manifest, task_path)
        require(
            (source_export / str(source_current["archive_path"])).read_bytes()
            == version_content[2].encode("utf-8"),
            "source current workspace export changed task bytes",
        )

        replayed_versions = replay_history_through_cli(
            args,
            target.token,
            source_export,
            source_manifest,
            task_path,
            source_history,
            temporary_root / "replay",
        )
        require(replayed_versions == [1, 2], "CLI history replay order changed")
        run_cli(
            args,
            target.token,
            "workspace",
            "import",
            "--root",
            str(source_export),
            "--state-dir",
            str(temporary_root / "full-import-state"),
            "--describe-binaries",
            "false",
        )

        target_changes = data(
            target.request("GET", "/v1/workspace/changes?since_generation=0&limit=200")
        ).get("changes")
        require(isinstance(target_changes, list), "target memory.changes omitted changes")
        target_task_changes = [
            change
            for change in target_changes
            if isinstance(change, dict) and change.get("path") == task_path
        ]
        require(
            [change.get("version") for change in target_task_changes] == [1, 2],
            f"target memory.changes lost task history: {target_task_changes!r}",
        )

        target_row = projection_row(
            args.database_container,
            args.database_user,
            args.database_name,
            task_id,
            target_external_ref,
        )
        require(
            target_row == ["2", "done", "700", "week", "2", "0", "0"],
            f"target import did not rebuild task projection/chunk/job state: {target_row!r}",
        )

        target_export = temporary_root / "target-export"
        run_cli(
            args,
            target.token,
            "workspace",
            "export",
            "--output",
            str(target_export),
            "--history",
        )
        target_manifest = json.loads(
            (target_export / "manifest.json").read_text(encoding="utf-8")
        )
        target_history = task_history_entries(target_manifest, task_path)
        require(
            history_identity(target_history) == history_identity(source_history),
            "re-export changed task version order, hashes, metadata, or state",
        )
        require(
            exported_task_bytes(target_export, target_history) == source_bytes,
            "re-export changed exact historical task Markdown bytes",
        )
        target_current = task_current_entry(target_manifest, task_path)
        target_entry_ref = target_current.get("entry_ref")
        require(
            isinstance(target_entry_ref, str) and target_entry_ref != entry_ref,
            "target round trip reused the source workspace entry identity",
        )
        require(
            (target_export / str(target_current["archive_path"])).read_bytes()
            == (source_export / str(source_current["archive_path"])).read_bytes(),
            "re-export changed exact current task Markdown bytes",
        )
        require(
            target_current.get("metadata") == source_current.get("metadata"),
            "re-export changed current task metadata",
        )
        target_first_read = read_version(target, target_entry_ref, 1)
        target_second_read = read_version(target, target_entry_ref, 2)
        require(
            [target_first_read.get("text"), target_second_read.get("text")]
            == [version_content[1], version_content[2]],
            "target versioned reads changed historical task Markdown bytes",
        )
        require(
            [target_first_read.get("metadata"), target_second_read.get("metadata")]
            == [first_read.get("metadata"), second_read.get("metadata")],
            "target versioned reads changed task metadata",
        )

    return {
        "schema": "brunn-live-tasks-storage-contract@v1",
        "status": "pass",
        "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
        "task_id": str(task_id),
        "entry_ref": entry_ref,
        "checks": {
            "canonical_lowercase_uuidv7": True,
            "disposable_identity_roundtrip": {
                "source_user_ref": source_user_ref,
                "target_user_ref": target_user_ref,
                "distinct": True,
            },
            "generic_portable_import": True,
            "projection_rebuilt": True,
            "target_projection_rebuilt": True,
            "no_chunks_or_jobs": True,
            "memory_changes_versions": [1, 2],
            "target_memory_changes_versions": [1, 2],
            "exact_version_reads": [1, 2],
            "cli_history_export_versions": [1, 2],
            "cli_history_import_versions": [1, 2],
            "cli_history_reexport_versions": [1, 2],
            "exact_current_and_history_bytes": True,
            "exact_metadata": True,
            "exact_version_order_and_hashes": True,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://127.0.0.1:18112")
    parser.add_argument("--token", required=True)
    parser.add_argument("--database-container", default="brunn-task-m1-db")
    parser.add_argument("--database-user", default="admin")
    parser.add_argument("--database-name", default="brunn")
    parser.add_argument(
        "--brunn-state",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "apps"
        / "api"
        / "target"
        / "debug"
        / "brunn-state",
    )
    args = parser.parse_args()
    try:
        result = run(args)
    except (ContractFailure, KeyError, OSError, ValueError) as error:
        result = {
            "schema": "brunn-live-tasks-storage-contract@v1",
            "status": "fail",
            "error": str(error),
        }
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
