#!/usr/bin/env python3
"""Destructive live checks for alpha account, authorization, and deletion safety."""

from __future__ import annotations

import argparse
import io
import json
import subprocess
import sys
import tarfile
import time
import urllib.request
import uuid
from pathlib import Path
from typing import Any, Sequence

from live_api_smoke import ApiClient, Sanitizer, SmokeFailure, effective_env


def check(condition: bool, message: str, context: Any = None) -> None:
    if not condition:
        detail = "" if context is None else f"\ncontext={context!r}"
        raise SmokeFailure(message + detail)


def bootstrap_privilege_is_locked() -> None:
    sql = (
        "SELECT "
        "has_function_privilege('app_rw',"
        "'straylight_auth.bootstrap_user(text,text,text,text,text[])','EXECUTE'),"
        "has_function_privilege('app_ro',"
        "'straylight_auth.bootstrap_user(text,text,text,text,text[])','EXECUTE'),"
        "has_function_privilege('app_rw',"
        "'straylight_auth.bootstrap_evaluation_user(text,text,text,text,text[])',"
        "'EXECUTE'),"
        "has_function_privilege('app_ro',"
        "'straylight_auth.bootstrap_evaluation_user(text,text,text,text,text[])',"
        "'EXECUTE')"
    )
    result = subprocess.run(
        [
            "docker",
            "exec",
            "straylight-db-1",
            "psql",
            "-U",
            "admin",
            "-d",
            "straylight",
            "-v",
            "ON_ERROR_STOP=1",
            "-At",
            "-F",
            "|",
            "-c",
            sql,
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise SmokeFailure(f"could not inspect bootstrap privileges: {result.stderr.strip()}")
    check(
        result.stdout.strip() == "f|f|t|f",
        "application bootstrap authority is broader or narrower than intended",
        result.stdout.strip(),
    )


def poll(
    client: ApiClient,
    path: str,
    wanted: set[str],
    timeout: float,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    last: dict[str, Any] = {}
    while time.monotonic() < deadline:
        body = client.request("GET", path).body
        check(isinstance(body, dict), f"{path} returned a non-object", body)
        last = body
        status = body.get("status")
        if status in wanted:
            return body
        if status in {"failed", "canceled"}:
            raise SmokeFailure(f"{path} reached terminal status {status}: {body!r}")
        time.sleep(0.25)
    raise SmokeFailure(f"{path} did not reach {sorted(wanted)}; last={last!r}")


def download(base_url: str, token: str, path: str, timeout: float) -> bytes:
    request = urllib.request.Request(
        base_url.rstrip("/") + path,
        headers={"Authorization": f"Bearer {token}", "Accept": "application/gzip"},
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        check(response.status == 200, "export download did not return 200")
        return response.read()


def set_backup_deadline_expired(user_id: uuid.UUID) -> None:
    sql = (
        "UPDATE straylight.account_deletion_requests "
        "SET backup_expiry_due_at=clock_timestamp()-interval '1 second' "
        f"WHERE user_id='{user_id}'::uuid AND status='awaiting_backup_expiry'"
    )
    result = subprocess.run(
        [
            "docker",
            "exec",
            "straylight-db-1",
            "psql",
            "-U",
            "admin",
            "-d",
            "straylight",
            "-v",
            "ON_ERROR_STOP=1",
            "-Atc",
            sql,
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise SmokeFailure(f"could not expire test backup deadline: {result.stderr.strip()}")


def database_account_status(user_id: uuid.UUID) -> tuple[str, bool, str]:
    sql = (
        "SELECT user_row.account_status,"
        "bool_and(credential.disabled_at IS NOT NULL),"
        "request.status "
        "FROM straylight.users AS user_row "
        "JOIN straylight.api_credentials AS credential ON credential.user_id=user_row.id "
        "JOIN straylight.account_deletion_requests AS request ON request.user_id=user_row.id "
        f"WHERE user_row.id='{user_id}'::uuid "
        "GROUP BY user_row.account_status,request.status"
    )
    result = subprocess.run(
        [
            "docker",
            "exec",
            "straylight-db-1",
            "psql",
            "-U",
            "admin",
            "-d",
            "straylight",
            "-v",
            "ON_ERROR_STOP=1",
            "-At",
            "-F",
            "|",
            "-c",
            sql,
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise SmokeFailure(f"could not inspect deleted test account: {result.stderr.strip()}")
    fields = result.stdout.strip().split("|")
    check(len(fields) == 3, "unexpected account status query output", fields)
    return fields[0], fields[1] == "t", fields[2]


def database_retention_redaction(
    user_id: uuid.UUID,
    status_credential_id: uuid.UUID,
) -> tuple[str, str, bool, bool, bool, bool]:
    sql = (
        "SELECT user_row.external_ref,user_row.display_name,"
        "NOT EXISTS (SELECT 1 FROM straylight.scopes WHERE user_id=user_row.id),"
        "NOT EXISTS (SELECT 1 FROM straylight.policies WHERE user_id=user_row.id),"
        "bool_and(request.reason='deleted'),"
        "bool_and((credential.id=$status_credential$"
        f"{status_credential_id}"
        "$status_credential$::uuid "
        "AND credential.disabled_at IS NULL "
        "AND credential.capabilities=ARRAY['status']::text[]) "
        "OR (credential.id<>$status_credential$"
        f"{status_credential_id}"
        "$status_credential$::uuid AND credential.disabled_at IS NOT NULL)) "
        "FROM straylight.users AS user_row "
        "JOIN straylight.api_credentials AS credential ON credential.user_id=user_row.id "
        "JOIN straylight.account_deletion_requests AS request ON request.user_id=user_row.id "
        f"WHERE user_row.id='{user_id}'::uuid "
        "GROUP BY user_row.id,user_row.external_ref,user_row.display_name"
    )
    result = subprocess.run(
        [
            "docker",
            "exec",
            "straylight-db-1",
            "psql",
            "-U",
            "admin",
            "-d",
            "straylight",
            "-v",
            "ON_ERROR_STOP=1",
            "-At",
            "-F",
            "|",
            "-c",
            sql,
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise SmokeFailure(
            f"could not inspect retention-stage redaction: {result.stderr.strip()}"
        )
    fields = result.stdout.strip().split("|")
    check(len(fields) == 6, "unexpected retention redaction query output", fields)
    return (
        fields[0],
        fields[1],
        fields[2] == "t",
        fields[3] == "t",
        fields[4] == "t",
        fields[5] == "t",
    )


def create_ungranted_scope(user_id: uuid.UUID, scope_id: uuid.UUID) -> str:
    scope_ref = f"scope:alpha-ungranted:{scope_id}"
    sql = (
        "INSERT INTO straylight.scopes (id,user_id,scope_ref,name) VALUES "
        f"('{scope_id}'::uuid,'{user_id}'::uuid,"
        f"'{scope_ref}','Alpha ungranted scope')"
    )
    result = subprocess.run(
        [
            "docker",
            "exec",
            "straylight-db-1",
            "psql",
            "-U",
            "admin",
            "-d",
            "straylight",
            "-v",
            "ON_ERROR_STOP=1",
            "-Atc",
            sql,
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise SmokeFailure(f"could not create delegation fixture: {result.stderr.strip()}")
    return scope_ref


def assert_nonreceipt_user_rows_are_purged(user_id: uuid.UUID) -> None:
    sql = f"""
DO $purge_audit$
DECLARE
  table_row record;
  remaining bigint;
BEGIN
  FOR table_row IN
    SELECT column_row.table_name
    FROM information_schema.columns AS column_row
    JOIN information_schema.tables AS base_table
      ON base_table.table_schema=column_row.table_schema
     AND base_table.table_name=column_row.table_name
     AND base_table.table_type='BASE TABLE'
    WHERE column_row.table_schema='straylight'
      AND column_row.column_name='user_id'
      AND column_row.table_name <> ALL(ARRAY[
        'api_credentials','account_deletion_requests'
      ]::text[])
    ORDER BY column_row.table_name
  LOOP
    EXECUTE format(
      'SELECT count(*) FROM straylight.%I WHERE user_id=$1',
      table_row.table_name
    ) INTO remaining USING '{user_id}'::uuid;
    IF remaining <> 0 THEN
      RAISE EXCEPTION '% retained % rows', table_row.table_name, remaining;
    END IF;
  END LOOP;
END
$purge_audit$;
"""
    result = subprocess.run(
        [
            "docker",
            "exec",
            "straylight-db-1",
            "psql",
            "-U",
            "admin",
            "-d",
            "straylight",
            "-v",
            "ON_ERROR_STOP=1",
            "-Atc",
            sql,
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise SmokeFailure(
            f"account deletion retained nonreceipt user rows: {result.stderr.strip()}"
        )


def run(args: argparse.Namespace, sanitizer: Sanitizer) -> None:
    bootstrap_privilege_is_locked()
    env = effective_env(args.env_file)
    sanitizer.register_env(env)
    admin_token = env.get("STRAYLIGHT_DEV_READ_WRITE_TOKEN", "")
    check(admin_token, "STRAYLIGHT_DEV_READ_WRITE_TOKEN is required")
    admin = ApiClient(args.base_url, sanitizer, token=admin_token, timeout=args.timeout)
    me = admin.request("GET", "/v1/me").body
    check("admin" in me.get("capabilities", []), "dev credential lacks admin", me)

    marker = uuid.uuid4().hex
    limited = admin.request(
        "POST",
        "/v1/credentials",
        json_body={"name": f"alpha-limited-{marker}", "access": "read_write"},
    ).body
    limited_token = limited["token"]
    sanitizer.register(limited_token)
    limited_client = ApiClient(
        args.base_url, sanitizer, token=limited_token, timeout=args.timeout
    )
    valid_eval = {
        "schema": "straylight-eval-import@v1",
        "run_id": marker,
        "case_id": "authorization-denial",
        "authorization_scope": "scope:root",
        "display_scope": "scope:root",
        "access_mode": "read_only",
        "documents": [],
        "delta_documents": [],
        "seed_checkpoint": None,
        "idempotency_key": marker,
    }
    limited_client.request(
        "POST", "/v1/admin/eval/import", json_body=valid_eval, expected=403
    )
    limited_client.request(
        "POST",
        "/v1/admin/users",
        json_body={
            "external_ref": f"denied-{marker}",
            "display_name": "Denied",
        },
        expected=403,
    )

    external_ref = f"alpha-safety-{marker}"
    provisioned = admin.request(
        "POST",
        "/v1/admin/users",
        json_body={
            "external_ref": external_ref,
            "display_name": "Alpha safety fixture",
            "credential_name": "Initial owner",
        },
    ).body
    user_id = uuid.UUID(provisioned["user"]["id"].split(":", 1)[1])
    owner_token = provisioned["credential"]["token"]
    sanitizer.register(owner_token)
    owner = ApiClient(args.base_url, sanitizer, token=owner_token, timeout=args.timeout)
    owner_me = owner.request("GET", "/v1/me").body
    check(owner_me["user"]["external_ref"] == external_ref, "provisioned wrong user", owner_me)
    check("credential:manage" in owner_me["capabilities"], "owner lacks credential control")

    opened = owner.request(
        "POST",
        "/v1/memory/open",
        json_body={
            "task": "Verify credential rotation continuity",
            "hints": {"authorization_scope": "scope:root"},
            "as_of": owner_me["corpus_revision"],
            "mode": "continuation",
            "token_budget": 2_000,
        },
    ).body
    session_id = opened.get("session_id")
    check(isinstance(session_id, str), "open did not return a session", opened)
    replacement = owner.request(
        "POST",
        "/v1/credentials",
        json_body={"name": "Redundant owner", "access": "owner"},
    ).body
    replacement_token = replacement["token"]
    replacement_credential_id = uuid.UUID(replacement["id"].split(":", 1)[1])
    sanitizer.register(replacement_token)
    replacement_owner = ApiClient(
        args.base_url, sanitizer, token=replacement_token, timeout=args.timeout
    )
    replacement_owner.request("GET", f"/v1/sessions/{session_id}")
    ungranted_scope = create_ungranted_scope(user_id, uuid.uuid4())
    delegation_denial = replacement_owner.request(
        "POST",
        "/v1/credentials",
        json_body={
            "name": "Forbidden broader-scope owner",
            "access": "owner",
            "scope_ids": [ungranted_scope],
        },
        expected=403,
    ).body
    check(
        delegation_denial.get("error", {}).get("code")
        == "credential_delegation_denied",
        "credential manager delegated an ungranted scope",
        delegation_denial,
    )

    retained_marker = f"account-content-purge-{marker}"
    retained_object_ref = f"object:{uuid.uuid4()}"
    saved = replacement_owner.request(
        "POST",
        "/v1/memory/save",
        json_body={
            "intent": retained_marker,
            "scope": "scope:root",
            "root_refs": [retained_object_ref],
            "source_refs": [],
            "items": [
                {
                    "action": "create",
                    "kind": "object",
                    "ref": retained_object_ref,
                    "payload": {
                        "type_profiles": ["core.artifact"],
                        "label": retained_marker,
                        "source_text": retained_marker,
                    },
                }
            ],
            "idempotency_key": retained_marker,
        },
    ).body
    check(saved.get("status") == "committed", "account purge fixture was not saved", saved)

    export = replacement_owner.request("POST", "/v1/account/exports").body
    export_id = export["id"]
    ready = poll(
        replacement_owner,
        f"/v1/account/exports/{export_id}",
        {"ready"},
        args.poll_timeout,
    )
    archive = download(
        args.base_url,
        replacement_token,
        ready["download_path"],
        args.timeout,
    )
    check(owner_token.encode() not in archive, "initial owner token leaked into export")
    check(replacement_token.encode() not in archive, "replacement owner token leaked into export")
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as exported:
        names = set(exported.getnames())
        credential_name = "straylight-export/tables/api_credentials.jsonl"
        check("straylight-export/manifest.json" in names, "export manifest is missing", names)
        check(credential_name in names, "credential inventory is missing", names)
        credential_bytes = exported.extractfile(credential_name).read()
        check(b"token_hash" not in credential_bytes, "credential hashes leaked into export")

    deletion = replacement_owner.request(
        "POST",
        "/v1/account/deletion",
        json_body={
            "confirmation": f"DELETE {external_ref}",
            "reason": "Automated alpha safety verification",
        },
    ).body
    deletion_id = deletion["id"]
    owner.request("GET", "/v1/me", expected=401)
    deletion_identity = replacement_owner.request("GET", "/v1/me").body
    check(
        deletion_identity.get("capabilities") == ["status"],
        "deletion credential was not reduced to status-only",
        deletion_identity,
    )
    awaiting = poll(
        replacement_owner,
        f"/v1/account/deletions/{deletion_id}",
        {"awaiting_backup_expiry"},
        args.poll_timeout,
    )
    check(
        awaiting["records_completed"] == awaiting["records_total"],
        "account deletion did not complete all record targets",
        awaiting,
    )
    check(
        awaiting["result"]["backup_status"] == "retained_until_deadline",
        "backup retention is not explicit",
        awaiting,
    )
    (
        retained_external_ref,
        retained_display_name,
        scopes_purged,
        policies_purged,
        deletion_reason_redacted,
        only_status_credential_active,
    ) = database_retention_redaction(user_id, replacement_credential_id)
    check(
        retained_external_ref == f"deleting:{user_id}",
        "account identity remains in the canonical retention-stage database",
        retained_external_ref,
    )
    check(
        retained_display_name == "Deleting account",
        "account display name remains in the retention-stage database",
        retained_display_name,
    )
    check(scopes_purged, "scope rows remain during backup retention")
    check(policies_purged, "policy rows remain during backup retention")
    check(deletion_reason_redacted, "deletion reason remains during backup retention")
    check(
        only_status_credential_active,
        "a non-status credential remains active during backup retention",
    )
    assert_nonreceipt_user_rows_are_purged(user_id)
    set_backup_deadline_expired(user_id)

    deadline = time.monotonic() + args.poll_timeout
    while time.monotonic() < deadline:
        response = replacement_owner.request(
            "GET", "/v1/me", expected={200, 401}
        )
        if response.status == 401:
            break
        time.sleep(0.25)
    else:
        raise SmokeFailure("deleted account credential remained usable")
    account_status, all_disabled, deletion_status = database_account_status(user_id)
    check(account_status == "deleted", "account did not reach deleted state", account_status)
    check(all_disabled, "one or more deleted account credentials remain active")
    check(deletion_status == "completed", "deletion receipt is not complete", deletion_status)
    assert_nonreceipt_user_rows_are_purged(user_id)

    admin.request(
        "DELETE",
        f"/v1/credentials/{limited['id']}",
    )
    print(
        "[alpha-safety] PASS: admin isolation, bounded credential delegation, "
        "rotation, complete export, schema-derived content purge, object-version "
        "purge, and retention-gated deletion"
    )


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser()
    result.add_argument("--base-url", default="http://localhost:18110")
    result.add_argument(
        "--env-file",
        type=Path,
        default=Path(__file__).resolve().parents[1] / ".env",
    )
    result.add_argument("--timeout", type=float, default=60.0)
    result.add_argument("--poll-timeout", type=float, default=180.0)
    return result


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    sanitizer = Sanitizer()
    try:
        run(args, sanitizer)
        return 0
    except SmokeFailure as error:
        print(f"[alpha-safety] FAIL: {sanitizer.text(error)}", file=sys.stderr)
        return 1
    except Exception as error:
        print(
            f"[alpha-safety] FAIL: unexpected {type(error).__name__}: "
            f"{sanitizer.text(error)}",
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
