#!/usr/bin/env python3
"""Destructive live checks for alpha account, authorization, and deletion safety."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import time
import urllib.request
import uuid
from datetime import datetime, timezone
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


def record_backup_erasure_proof(user_id: uuid.UUID) -> None:
    marker = f"live-alpha-safety:{user_id}"
    receipt_sha256 = hashlib.sha256(marker.encode("ascii")).hexdigest()
    watermark = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    result = subprocess.run(
        [
            "docker",
            "compose",
            "run",
            "--rm",
            "-T",
            "migrate",
            "operator",
            "record-backup-watermark",
            "--oldest-retained-created-at",
            watermark,
            "--receipt-sha256",
            receipt_sha256,
            "--source",
            marker,
        ],
        cwd=Path(__file__).resolve().parents[1],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise SmokeFailure(
            f"could not record test backup-erasure proof: {result.stderr.strip()}"
        )
    try:
        receipt = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise SmokeFailure(
            f"backup-erasure command returned invalid JSON: {result.stdout!r}"
        ) from error
    check(
        receipt.get("account_deletions_verified", 0) >= 1,
        "backup-erasure proof did not cover the test deletion",
        receipt,
    )


def force_upload_verifying(user_id: uuid.UUID, upload_id: uuid.UUID) -> None:
    sql = (
        "UPDATE straylight.asset_uploads "
        "SET status='verifying',completion_token=gen_random_uuid(),"
        "completion_lease_expires_at=clock_timestamp()+interval '1 hour' "
        f"WHERE user_id='{user_id}'::uuid AND id='{upload_id}'::uuid "
        "AND status='uploading' RETURNING id"
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
    if result.returncode != 0 or str(upload_id) not in result.stdout:
        raise SmokeFailure(
            "could not place deletion-race upload in verification: "
            + result.stderr.strip()
        )


def admin_sql(sql: str, failure: str) -> str:
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
        raise SmokeFailure(f"{failure}: {result.stderr.strip()}")
    return result.stdout.strip()


def install_upload_fault(
    user_id: uuid.UUID,
    marker: str,
    phase: str,
) -> tuple[str, str, str]:
    check(phase in {"audit", "commit"}, "unsupported upload fault phase", phase)
    check(marker.isalnum(), "upload fault marker contains unsafe characters")
    suffix = f"{phase}_{marker}"
    function_name = f"live_fail_upload_{suffix}"
    trigger_name = f"live_fail_upload_{suffix}"
    table_name = "audit_events" if phase == "audit" else "asset_uploads"
    condition = (
        f"NEW.user_id='{user_id}'::uuid AND NEW.action='asset.upload.create'"
        if phase == "audit"
        else f"NEW.user_id='{user_id}'::uuid"
    )
    trigger_kind = (
        "CREATE TRIGGER"
        if phase == "audit"
        else "CREATE CONSTRAINT TRIGGER"
    )
    timing = (
        "BEFORE INSERT"
        if phase == "audit"
        else "AFTER INSERT"
    )
    deferral = "" if phase == "audit" else "DEFERRABLE INITIALLY DEFERRED"
    admin_sql(
        f"""
        CREATE FUNCTION straylight.{function_name}()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $fault$
        BEGIN
          IF {condition} THEN
            RAISE EXCEPTION 'live upload {phase} fault';
          END IF;
          RETURN NEW;
        END
        $fault$;
        {trigger_kind} {trigger_name}
        {timing} ON straylight.{table_name}
        {deferral}
        FOR EACH ROW EXECUTE FUNCTION straylight.{function_name}();
        """,
        f"could not install {phase} upload fault",
    )
    return table_name, trigger_name, function_name


def remove_upload_fault(table_name: str, trigger_name: str, function_name: str) -> None:
    admin_sql(
        f"""
        DROP TRIGGER IF EXISTS {trigger_name} ON straylight.{table_name};
        DROP FUNCTION IF EXISTS straylight.{function_name}();
        """,
        "could not remove upload fault",
    )


def s3_cli_environment(env: dict[str, str]) -> tuple[list[str], dict[str, str], str]:
    aws = shutil.which("aws")
    check(aws is not None, "AWS CLI is required for multipart lifecycle checks")
    endpoint = env.get("STRAYLIGHT_S3_ENDPOINT", "")
    if not endpoint or endpoint == "http://minio:9000":
        endpoint = (
            "http://127.0.0.1:"
            + env.get("STRAYLIGHT_MINIO_PORT", "19110")
        )
    bucket = (
        env.get("STRAYLIGHT_S3_BUCKET")
        or env.get("STRAYLIGHT_MINIO_BUCKET")
        or "straylight"
    )
    access_key = env.get("STRAYLIGHT_S3_ACCESS_KEY") or env.get(
        "STRAYLIGHT_MINIO_ACCESS_KEY"
    )
    secret_key = env.get("STRAYLIGHT_S3_SECRET_KEY") or env.get(
        "STRAYLIGHT_MINIO_SECRET_KEY"
    )
    region = (
        env.get("STRAYLIGHT_S3_REGION")
        or env.get("STRAYLIGHT_MINIO_REGION")
        or "us-east-1"
    )
    process_env = os.environ.copy()
    if access_key and secret_key:
        process_env["AWS_ACCESS_KEY_ID"] = access_key
        process_env["AWS_SECRET_ACCESS_KEY"] = secret_key
    process_env["AWS_DEFAULT_REGION"] = region
    return [aws, "--endpoint-url", endpoint, "s3api"], process_env, bucket


def s3api(env: dict[str, str], *arguments: str) -> dict[str, Any]:
    command, process_env, _ = s3_cli_environment(env)
    result = subprocess.run(
        [*command, *arguments],
        env=process_env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise SmokeFailure(f"S3 lifecycle command failed: {result.stderr.strip()}")
    return json.loads(result.stdout or "{}")


def multipart_upload_ids(
    env: dict[str, str],
    object_prefix: str,
) -> set[str]:
    _, _, bucket = s3_cli_environment(env)
    listed = s3api(
        env,
        "list-multipart-uploads",
        "--bucket",
        bucket,
        "--prefix",
        object_prefix,
    )
    return {
        str(upload["UploadId"])
        for upload in listed.get("Uploads", [])
        if str(upload.get("Key", "")).startswith(object_prefix)
    }


def assert_upload_creation_compensation(
    client: ApiClient,
    env: dict[str, str],
    user_id: uuid.UUID,
    marker: str,
) -> None:
    for phase in ("audit", "commit"):
        fault = install_upload_fault(user_id, marker, phase)
        path = f"alpha-safety/{marker}/{phase}-failure.bin"
        payload = f"{phase}-failure:{marker}".encode("ascii")
        try:
            client.request(
                "POST",
                "/v1/asset-uploads",
                json_body={
                    "scope": "scope:root",
                    "path": path,
                    "content_hash": "sha256:" + hashlib.sha256(payload).hexdigest(),
                    "size_bytes": len(payload),
                    "media_type": "application/octet-stream",
                    "describe_binary": False,
                },
                expected=500,
            )
        finally:
            remove_upload_fault(*fault)
        row_counts = admin_sql(
            "SELECT "
            f"(SELECT count(*) FROM straylight.asset_uploads WHERE user_id='{user_id}'::uuid "
            f"AND path='{path}'),"
            f"(SELECT count(*) FROM straylight.asset_multipart_intents "
            f"WHERE user_id='{user_id}'::uuid)",
            f"could not inspect {phase} upload compensation",
        )
        check(
            row_counts == "0|0",
            f"{phase} failure left upload or multipart intent rows",
            row_counts,
        )
        check(
            not multipart_upload_ids(env, f"{user_id}/uploads/"),
            f"{phase} failure left an incomplete multipart upload",
        )


def assert_orphan_multipart_reconciliation(
    env: dict[str, str],
    user_id: uuid.UUID,
    credential_id: uuid.UUID,
    timeout: float,
) -> None:
    _, _, bucket = s3_cli_environment(env)
    object_id = uuid.uuid4()
    object_key = f"{user_id}/uploads/{object_id}"
    created = s3api(
        env,
        "create-multipart-upload",
        "--bucket",
        bucket,
        "--key",
        object_key,
    )
    upload_id = str(created["UploadId"])
    check(
        upload_id in multipart_upload_ids(env, object_key),
        "orphan multipart fixture was not created",
    )
    encoded_upload_id = upload_id.encode("utf-8").hex()
    inserted_intent = admin_sql(
        f"""
        INSERT INTO straylight.asset_multipart_intents (
          id,user_id,scope_id,credential_id,object_key,multipart_upload_id,
          created_at,updated_at,expires_at
        )
        SELECT
          '{object_id}'::uuid,'{user_id}'::uuid,scope.id,
          '{credential_id}'::uuid,'{object_key}',
          convert_from(decode('{encoded_upload_id}','hex'),'UTF8'),
          clock_timestamp()-interval '20 minutes',
          clock_timestamp()-interval '20 minutes',
          clock_timestamp()-interval '5 minutes'
        FROM straylight.scopes AS scope
        WHERE scope.user_id='{user_id}'::uuid
          AND scope.scope_ref='scope:root'
        RETURNING id;
        """,
        "could not install expired multipart intent fixture",
    )
    check(
        str(object_id) in inserted_intent,
        "expired multipart intent fixture did not resolve its root scope",
        inserted_intent,
    )
    try:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            remaining = multipart_upload_ids(env, object_key)
            intent_count = admin_sql(
                "SELECT count(*) FROM straylight.asset_multipart_intents "
                f"WHERE user_id='{user_id}'::uuid AND id='{object_id}'::uuid",
                "could not inspect orphan multipart intent",
            )
            if upload_id not in remaining and intent_count == "0":
                return
            time.sleep(0.5)
        raise SmokeFailure(
            "orphan multipart reconciliation did not abort S3 state and remove its intent"
        )
    finally:
        remaining = multipart_upload_ids(env, object_key)
        if upload_id in remaining:
            s3api(
                env,
                "abort-multipart-upload",
                "--bucket",
                bucket,
                "--key",
                object_key,
                "--upload-id",
                upload_id,
            )
        admin_sql(
            "DELETE FROM straylight.asset_multipart_intents "
            f"WHERE user_id='{user_id}'::uuid AND id='{object_id}'::uuid",
            "could not clean orphan multipart fixture",
        )


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
        'api_credentials','account_deletion_requests','account_deletion_fences'
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

    assert_upload_creation_compensation(
        replacement_owner,
        env,
        user_id,
        marker,
    )
    assert_orphan_multipart_reconciliation(
        env,
        user_id,
        replacement_credential_id,
        args.poll_timeout,
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

    deletion_race_bytes = f"deletion-race:{marker}".encode("ascii")
    deletion_race_upload = replacement_owner.request(
        "POST",
        "/v1/asset-uploads",
        json_body={
            "scope": "scope:root",
            "path": f"alpha-safety/{marker}/deletion-race.bin",
            "content_hash": "sha256:" + hashlib.sha256(deletion_race_bytes).hexdigest(),
            "size_bytes": len(deletion_race_bytes),
            "media_type": "application/octet-stream",
            "describe_binary": False,
        },
    ).body
    deletion_race_upload_id = uuid.UUID(
        deletion_race_upload["upload_ref"].split(":", 1)[1]
    )
    force_upload_verifying(user_id, deletion_race_upload_id)

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
    check(
        awaiting["result"]["asset_upload_sessions_aborted"] >= 1,
        "account deletion did not abort the upload already in verification",
        awaiting,
    )
    check(
        awaiting["result"]["storage_reconciliation_passes"] == 2,
        "account deletion did not complete both storage reconciliation passes",
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
    record_backup_erasure_proof(user_id)

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
        "rotation, upload fault compensation, orphan multipart reconciliation, "
        "complete export, schema-derived content purge, object-version purge, "
        "and retention-gated deletion"
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
