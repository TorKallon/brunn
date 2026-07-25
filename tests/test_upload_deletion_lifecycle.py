from __future__ import annotations

import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
UPLOAD = ROOT / "apps/api/src/upload_service.rs"
WRITE = ROOT / "apps/api/src/write_service.rs"
QUOTA = ROOT / "apps/api/src/quota.rs"
ACCOUNT = ROOT / "apps/api/src/account_worker.rs"
ACCOUNT_SERVICE = ROOT / "apps/api/src/account_service.rs"
MIGRATION = ROOT / "apps/api/migrations/0041_upload_deletion_fencing.sql"
BACKUP_ERASURE_MIGRATION = (
    ROOT / "apps/api/migrations/0046_verified_backup_erasure.sql"
)
EXPORT_DELETE_MIGRATION = (
    ROOT / "apps/api/migrations/0047_resumable_account_export_deletion.sql"
)
COMPLETED_UPLOAD_LEASE_MIGRATION = (
    ROOT / "apps/api/migrations/0048_completed_upload_finalization_lease.sql"
)
LIVE_ALPHA = ROOT / "tests/live_alpha_safety.py"


class UploadDeletionLifecycleContractTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.upload = UPLOAD.read_text(encoding="utf-8")
        cls.write = WRITE.read_text(encoding="utf-8")
        cls.quota = QUOTA.read_text(encoding="utf-8")
        cls.account = ACCOUNT.read_text(encoding="utf-8")
        cls.account_service = ACCOUNT_SERVICE.read_text(encoding="utf-8")
        cls.migration = MIGRATION.read_text(encoding="utf-8")
        cls.backup_erasure_migration = BACKUP_ERASURE_MIGRATION.read_text(
            encoding="utf-8"
        )
        cls.export_delete_migration = EXPORT_DELETE_MIGRATION.read_text(
            encoding="utf-8"
        )
        cls.completed_upload_lease_migration = (
            COMPLETED_UPLOAD_LEASE_MIGRATION.read_text(encoding="utf-8")
        )
        cls.live_alpha = LIVE_ALPHA.read_text(encoding="utf-8")

    def test_deletion_transition_installs_an_immutable_storage_fence(self) -> None:
        self.assertIn("CREATE TABLE straylight.account_deletion_fences", self.migration)
        self.assertIn("account_deletion_fences_immutable", self.migration)
        transition = self.migration.index("OLD.account_status = 'active'")
        storage_lock = self.migration.index(
            "hashtextextended('storage:' || NEW.id::text, 0)", transition
        )
        fence_insert = self.migration.index(
            "INSERT INTO straylight.account_deletion_fences", storage_lock
        )
        self.assertLess(transition, storage_lock)
        self.assertLess(storage_lock, fence_insert)
        self.assertIn("a deletion-fenced account cannot be reactivated", self.migration)

    def test_upload_creation_has_a_durable_pre_s3_intent_and_compensation(self) -> None:
        intent = self.upload.index(
            "INSERT INTO straylight.asset_multipart_intents"
        )
        intent_commit = self.upload.index("intent_tx.commit().await?", intent)
        multipart_create = self.upload.index(".create_multipart_upload(", intent_commit)
        audit = self.upload.index('"asset.upload.create"', multipart_create)
        final_commit = self.upload.index("tx.commit().await?", audit)
        compensation = self.upload.index(".abort_multipart_upload(", final_commit)
        self.assertLess(intent, intent_commit)
        self.assertLess(intent_commit, multipart_create)
        self.assertLess(multipart_create, audit)
        self.assertLess(audit, final_commit)
        self.assertLess(final_commit, compensation)
        self.assertIn("reconcile_one_orphan_multipart", self.upload)
        self.assertIn(".list_multipart_uploads()", self.upload)

    def test_completion_rechecks_exact_lease_after_multipart_completion(self) -> None:
        complete = self.upload.index("pub async fn complete")
        storage_lock = self.upload.index("lock_storage_write(&mut promote_tx", complete)
        multipart_complete = self.upload.index(".complete_multipart_upload(", storage_lock)
        row_lock = self.upload.index(
            "load_upload(&mut promote_tx, auth, upload_id, true)", multipart_complete
        )
        lease_check = self.upload.index("completion_lease_owned(", row_lock)
        promotion = self.upload.index(".promote_verified_upload(", lease_check)
        self.assertLess(storage_lock, multipart_complete)
        self.assertLess(multipart_complete, row_lock)
        self.assertLess(row_lock, lease_check)
        self.assertLess(lease_check, promotion)
        self.assertIn("compensate_promoted_object", self.upload)
        self.assertIn("purge_exact_object_version", self.upload)

    def test_completed_upload_is_leased_through_exactly_once_consumption(self) -> None:
        complete = self.upload.index("pub async fn complete")
        published = self.upload.index("SET status='completed'", complete)
        retained_token = self.upload.index("failure_code=NULL,completion_token=$6", published)
        retained_lease = self.upload.index(
            "completion_lease_expires_at=clock_timestamp()", retained_token
        )
        stage = self.upload.index("write_service::stage_uploads(", retained_lease)
        consumed = self.upload.index("SET status='consumed'", stage)
        token_match = self.upload.index("AND completion_token=$4", consumed)
        affected = self.upload.index(".rows_affected()", token_match)
        exact = self.upload.index("if completion_update != 1", affected)
        self.assertLess(published, retained_token)
        self.assertLess(retained_token, retained_lease)
        self.assertLess(retained_lease, stage)
        self.assertLess(stage, consumed)
        self.assertLess(consumed, token_match)
        self.assertLess(token_match, affected)
        self.assertLess(affected, exact)
        self.assertIn(
            "OR status IN ('verifying','completed')",
            self.completed_upload_lease_migration,
        )
        self.assertIn(
            "(completion_token IS NULL) = (completion_lease_expires_at IS NULL)",
            self.completed_upload_lease_migration,
        )

        expiry = self.upload.index("pub async fn expire_one")
        expiry_body = self.upload[expiry : self.upload.index("pub async fn expire_one_stage")]
        self.assertGreaterEqual(expiry_body.count("status <> 'completed'"), 2)
        self.assertGreaterEqual(
            expiry_body.count(
                "completion_lease_expires_at <= clock_timestamp()"
            ),
            2,
        )

    def test_stored_stage_validates_exact_object_identity_before_replay(self) -> None:
        stage = self.write.index("async fn stage_uploads_inner")
        storage_lock = self.write.index(
            "pg_advisory_xact_lock(hashtextextended('storage:'", stage
        )
        validation = self.write.index(
            "validate_stored_stage_objects(state, auth, &prepared)", storage_lock
        )
        replay_lookup = self.write.index(
            "SELECT id,input_hash,inventory_hash", validation
        )
        helper = self.write.index("async fn validate_stored_stage_objects")
        exact_version = self.write.index(
            ".verify_object_identity_version(", helper
        )
        self.assertLess(storage_lock, validation)
        self.assertLess(validation, replay_lookup)
        self.assertLess(helper, exact_version)
        self.assertIn("Some(object_version_id)", self.write[helper:exact_version + 200])
        self.assertIn(
            'format!("{}/blobs/{digest}", auth.user_id.0)',
            self.write[helper:exact_version],
        )

    def test_verifying_abort_clears_completion_lease_atomically(self) -> None:
        abort = self.upload.index("pub async fn abort_active_for_user")
        update = self.upload.index("SET status='aborted'", abort)
        token_clear = self.upload.index("completion_token=NULL", update)
        lease_clear = self.upload.index("completion_lease_expires_at=NULL", token_clear)
        self.assertLess(update, token_clear)
        self.assertLess(token_clear, lease_clear)

    def test_account_deletion_holds_lock_through_two_storage_sweeps(self) -> None:
        advance = self.account.index("async fn advance_account_deletion")
        lock = self.account.index("pg_advisory_xact_lock", advance)
        abort_rows = self.account.index("abort_active_for_user", lock)
        abort_first = self.account.index("abort_multipart_prefix", abort_rows)
        purge_first = self.account.index("purge_prefix", abort_first)
        abort_second = self.account.index("abort_multipart_prefix", purge_first)
        purge_second = self.account.index("purge_prefix", abort_second)
        redact = self.account.index("redact_account_for_retention", purge_second)
        commit = self.account.index("tx.commit().await?", redact)
        self.assertLess(lock, abort_rows)
        self.assertLess(abort_rows, abort_first)
        self.assertLess(abort_first, purge_first)
        self.assertLess(purge_first, abort_second)
        self.assertLess(abort_second, purge_second)
        self.assertLess(purge_second, redact)
        self.assertLess(redact, commit)

    def test_account_export_is_streamed_and_published_under_the_fence(self) -> None:
        publish = self.account.index("async fn publish_account_export")
        lock = self.account.index("assert_storage_write_allowed", publish)
        create = self.account.index(".create_multipart_upload(", lock)
        parts = self.account.index("upload_export_parts(", create)
        complete = self.account.index(".complete_multipart_upload(", parts)
        ready = self.account.index("SET status='ready'", complete)
        commit = self.account.index("tx.commit().await", ready)
        self.assertLess(lock, create)
        self.assertLess(create, parts)
        self.assertLess(parts, complete)
        self.assertLess(complete, ready)
        self.assertLess(ready, commit)
        self.assertIn("ACCOUNT_EXPORT_PART_SIZE_BYTES", self.account)
        self.assertNotIn(".put_file(", self.account[publish:])

    def test_account_export_uses_a_separate_bounded_temporary_allowance(self) -> None:
        publish = self.account.index("async fn publish_account_export")
        allowance = self.account.index(
            "ensure_temporary_export_capacity", publish
        )
        multipart = self.account.index(".create_multipart_upload(", allowance)
        self.assertLess(allowance, multipart)

        durable_quota = self.quota[
            self.quota.index("pub async fn ensure_storage_capacity_for_objects")
            : self.quota.index("pub async fn ensure_temporary_export_capacity")
        ]
        temporary_start = self.quota.index(
            "pub async fn ensure_temporary_export_capacity"
        )
        temporary_quota = self.quota[
            temporary_start : self.quota.index(
                "impl UsageReservation", temporary_start
            )
        ]
        self.assertNotIn("straylight.account_exports", durable_quota)
        self.assertIn("temporary_export_limit(storage_limit)", temporary_quota)
        self.assertIn("TEMPORARY_EXPORT_MAX_OVERHEAD_BYTES", self.quota)

    def test_account_export_download_stream_enforces_size_and_sha256(self) -> None:
        download = self.account_service.index("pub async fn download_export")
        exact_version = self.account_service.index(
            ".get_stream_version(&object_key, Some(&object_version_id))", download
        )
        head_size = self.account_service.index(
            "object.content_length !=", exact_version
        )
        verifier = self.account_service.index(
            "AccountExportIntegrityReader::new(", head_size
        )
        body = self.account_service.index("Body::from_stream(stream)", verifier)
        self.assertLess(exact_version, head_size)
        self.assertLess(head_size, verifier)
        self.assertLess(verifier, body)
        self.assertIn(
            "account export stream failed size or SHA-256 verification",
            self.account_service,
        )

    def test_export_delete_is_durable_before_object_purge(self) -> None:
        delete = self.account_service.index("pub async fn delete_export")
        begin = self.account_service.index(
            "straylight_auth.begin_account_export_delete", delete
        )
        durable = self.account_service.index("tx.commit().await?", begin)
        purge = self.account_service.index(".purge_all_versions(", durable)
        finish = self.account_service.index(
            "straylight_auth.finish_account_export_delete", purge
        )
        completed = self.account_service.index("tx.commit().await?", finish)
        self.assertLess(begin, durable)
        self.assertLess(durable, purge)
        self.assertLess(purge, finish)
        self.assertLess(finish, completed)
        self.assertIn("SECURITY DEFINER", self.export_delete_migration)
        self.assertIn("FOR UPDATE", self.export_delete_migration)
        self.assertIn(
            "straylight_auth.require_credential_control",
            self.export_delete_migration,
        )
        self.assertIn("SET status='deleting'", self.export_delete_migration)
        self.assertIn("TO app_rw", self.export_delete_migration)
        self.assertIn(
            "DROP FUNCTION straylight_auth.lock_account_export_for_delete",
            self.export_delete_migration,
        )

    def test_account_deletion_requires_verified_backup_erasure(self) -> None:
        self.assertIn(
            "account_deletion_completion_requires_backup_proof",
            self.backup_erasure_migration,
        )
        self.assertIn("canonical_purged_at", self.account)
        self.assertIn("backup_erasure_verified", self.account)
        self.assertIn(
            "account deletion cannot complete without verified backup erasure",
            self.account,
        )
        self.assertIn("record_backup_erasure_proof", self.live_alpha)

    def test_unready_backup_retention_does_not_starve_actionable_deletions(self) -> None:
        claim = self.account.index("async fn claim_account_deletion")
        prepare = self.account.index("async fn prepare_account_deletion", claim)
        claim_body = self.account[claim:prepare]
        self.assertIn("status IN ('queued','running')", claim_body)
        self.assertIn(
            "status='awaiting_backup_expiry'\n"
            "              AND backup_expiry_due_at <= clock_timestamp()",
            claim_body,
        )
        self.assertIn(
            "CASE status WHEN 'running' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END",
            claim_body,
        )

    def test_account_export_includes_completed_upload_crash_window(self) -> None:
        build = self.account.index("async fn build_export_inner")
        query = self.account.index("WITH object_references AS", build)
        asset_versions = self.account.index(
            "FROM straylight.asset_versions", query
        )
        completed_uploads = self.account.index(
            "FROM straylight.asset_uploads", asset_versions
        )
        exact_dedupe = self.account.index(
            "GROUP BY bucket,object_key,object_version_id", completed_uploads
        )
        download = self.account.index(
            ".download_version_to_path(", exact_dedupe
        )
        hash_check = self.account.index(
            "export object integrity check failed", download
        )
        self.assertLess(query, asset_versions)
        self.assertLess(asset_versions, completed_uploads)
        self.assertLess(completed_uploads, exact_dedupe)
        self.assertLess(exact_dedupe, download)
        self.assertLess(download, hash_check)
        self.assertIn("status IN ('completed','consumed')", self.account[query:exact_dedupe])
        self.assertIn("canonical_object_version_id IS NOT NULL", self.account[query:exact_dedupe])
        self.assertIn("count(DISTINCT content_hash)", self.account[query:exact_dedupe])
        self.assertIn("count(DISTINCT size_bytes)", self.account[query:exact_dedupe])
        self.assertIn(
            "application/x-straylight-deleted",
            self.account[query:completed_uploads],
        )
        self.assertIn(
            "redacted_by_deletion_job",
            self.account[query:completed_uploads],
        )

    def test_live_alpha_exercises_deletion_during_verification(self) -> None:
        self.assertIn("force_upload_verifying", self.live_alpha)
        self.assertIn("completion_lease_expires_at", self.live_alpha)
        self.assertIn('"asset_upload_sessions_aborted"', self.live_alpha)
        self.assertIn('"storage_reconciliation_passes"', self.live_alpha)

    def test_live_alpha_injects_create_and_orphan_failures(self) -> None:
        self.assertIn('for phase in ("audit", "commit")', self.live_alpha)
        self.assertIn("DEFERRABLE INITIALLY DEFERRED", self.live_alpha)
        self.assertIn("assert_orphan_multipart_reconciliation", self.live_alpha)
        self.assertIn("create-multipart-upload", self.live_alpha)
        self.assertIn("asset_multipart_intents", self.live_alpha)


if __name__ == "__main__":
    unittest.main()
