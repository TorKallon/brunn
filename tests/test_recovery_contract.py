import json
import hashlib
import os
import shutil
import subprocess
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


class RecoveryContractTests(unittest.TestCase):
    def test_migration_keeps_triggers_enabled_and_locks_parents(self) -> None:
        migration = (
            ROOT
            / "apps/api/migrations/0040_recovery_and_stage_safety.sql"
        ).read_text()
        self.assertNotIn("session_replication_role", migration)
        self.assertNotIn("DISABLE TRIGGER", migration.upper())
        self.assertIn(
            "CREATE OR REPLACE FUNCTION brunn.remap_asset_object_versions",
            migration,
        )
        self.assertIn("FOR UPDATE OF version,asset", migration)
        self.assertIn(
            "brunn.foreign_key_reference_exists(\n"
            "      'brunn.asset_versions'::regclass",
            migration,
        )
        self.assertIn(
            "REVOKE ALL ON FUNCTION "
            "brunn.remap_asset_object_versions(jsonb)",
            migration,
        )

    def test_restore_is_journaled_multipart_and_identity_checked(self) -> None:
        recovery = (
            ROOT / "apps/api/src/object_store/backup.rs"
        ).read_text()
        for expected in (
            "brunn-object-version-restore-map@v2",
            "write_json_atomic(mapping_path",
            "reconcile_restore_journal",
            "restore_object_multipart",
            "abort_all_multipart_uploads",
            "brunn.remap_asset_object_versions($1::jsonb)",
            "database_references_verified",
            "verify_remote_object(store, source",
        ):
            self.assertIn(expected, recovery)
        self.assertNotIn(
            "UPDATE brunn.asset_versions version\n"
            "         SET object_version_id=",
            recovery,
        )

    def test_redacted_deletion_receipts_are_not_physical_object_references(self) -> None:
        recovery = (ROOT / "apps/api/src/object_store/backup.rs").read_text()
        reference_inventory = (
            ROOT / "scripts/database-object-references.sql"
        ).read_text()
        account_export = (ROOT / "apps/api/src/account_worker.rs").read_text()
        markers = (
            "/deleted-assets/",
            "e3b0c44298fc1c149afbf4c8996fb924"
            "27ae41e4649b934ca495991b7852b855",
            "size_bytes=0",
            "application/x-brunn-deleted",
            "redacted_by_deletion_job",
        )
        for marker in markers:
            self.assertIn(marker, recovery)
            self.assertIn(marker.replace("size_bytes=0", "size_bytes = 0"), reference_inventory)
            self.assertIn(marker, account_export)
        self.assertGreaterEqual(
            recovery.count("application/x-brunn-deleted"),
            6,
        )
        self.assertIn("FROM brunn.entry_versions", reference_inventory)
        self.assertIn("content_sha256", reference_inventory)
        self.assertNotIn("FROM brunn.asset_uploads", reference_inventory)
        self.assertNotIn("WHERE size_bytes > 0", reference_inventory)

    def test_restore_drill_retains_resumable_state_and_reverifies(self) -> None:
        script = (
            ROOT / "scripts/managed-s3-restore-drill.sh"
        ).read_text()
        for expected in (
            "BRUNN_MANAGED_RESTORE_STATE_ROOT",
            "restore_state_key",
            "database_references_verified",
            "re-verifying restored object bytes after database remap",
            "cleanup-restore",
        ):
            self.assertIn(expected, script)

    def test_managed_backup_verifier_accepts_complete_minimal_archive(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            backup = Path(temporary) / "backup"
            objects = backup / "object-store/objects"
            objects.mkdir(parents=True)
            blob = b"abc"
            digest = hashlib.sha256(blob).hexdigest()
            (objects / digest).write_bytes(blob)
            (backup / "object-store/manifest.json").write_text(
                json.dumps(
                    {
                        "format": "brunn-object-version-archive@v1",
                        "created_at": "2026-07-24T00:00:00Z",
                        "source_bucket": "recovery-test",
                        "bucket_versioning": "Enabled",
                        "entries": [
                            {
                                "kind": "object_version",
                                "key": "user/blob",
                                "source_version_id": "source-version",
                                "is_latest": True,
                                "last_modified_unix_seconds": 1,
                                "last_modified_subsecond_nanos": 0,
                                "size_bytes": len(blob),
                                "sha256": f"sha256:{digest}",
                                "metadata": {},
                            }
                        ],
                        "summary": {
                            "object_versions": 1,
                            "delete_markers": 0,
                            "unique_blobs": 1,
                            "logical_bytes": len(blob),
                            "stored_bytes": len(blob),
                        },
                    }
                )
            )
            (backup / "manifest.json").write_text(
                json.dumps(
                    {
                        "format": "brunn-managed-s3-coordinated-backup@v1",
                        "backup_id": "minimal",
                        "created_at": "2026-07-24T00:00:00Z",
                        "completed_at": "2026-07-24T00:01:00Z",
                        "expires_at": "2026-08-24T00:00:00Z",
                        "retention_days": 30,
                        "object_bucket": "recovery-test",
                        "object_archive": "object-store/manifest.json",
                        "consistency": {
                            "writers_stopped": True,
                            "postgres_snapshot": "serializable-deferrable",
                            "object_snapshot": "portable-all-versions",
                        },
                    }
                )
            )
            fixture_files = {
                "postgres.dump": b"dump",
                "db-inventory.txt": b"inventory",
                "database-invariants.json": b'{"safe":true}\n',
                "database-object-references.json": json.dumps(
                    {
                        "format": "brunn-database-object-references@v1",
                        "object_bucket": "recovery-test",
                        "references": [
                            {
                                "relation": "asset_versions",
                                "user_id": "00000000-0000-0000-0000-000000000001",
                                "bucket": "recovery-test",
                                "object_key": "user/blob",
                                "object_version_id": "source-version",
                                "content_hash": f"sha256:{digest}",
                                "size_bytes": len(blob),
                            }
                        ],
                        "summary": {
                            "reference_count": 1,
                            "distinct_object_versions": 1,
                        },
                    }
                ).encode(),
                "runtime-images.json": b"[]\n",
                "compose-service-hashes.txt": b"api hash\n",
            }
            for relative, content in fixture_files.items():
                (backup / relative).write_bytes(content)
            checksummed = [
                "postgres.dump",
                "db-inventory.txt",
                "database-invariants.json",
                "database-object-references.json",
                "object-store/manifest.json",
                "runtime-images.json",
                "compose-service-hashes.txt",
                "manifest.json",
            ]
            (backup / "CHECKSUMS.sha256").write_text(
                "".join(
                    f"{hashlib.sha256((backup / relative).read_bytes()).hexdigest()}  "
                    f"{relative}\n"
                    for relative in checksummed
                )
            )
            verified = subprocess.run(
                [str(ROOT / "scripts/verify-managed-backup.sh"), str(backup)],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(verified.returncode, 0, verified.stderr)

    def test_pruning_requires_newer_verified_unexpired_backup(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture_root = Path(temporary)
            scripts = fixture_root / "scripts"
            scripts.mkdir()
            shutil.copy2(ROOT / "scripts/prune-backups.sh", scripts)
            shutil.copy2(
                ROOT / "scripts/verify-restore-drill-receipt.sh",
                scripts,
            )
            for name in ("verify-backup.sh", "verify-managed-backup.sh"):
                verifier = scripts / name
                verifier.write_text("#!/bin/sh\nexit 0\n")
                verifier.chmod(0o755)

            backups = fixture_root / "backups"
            backups.mkdir()
            now = datetime.now(timezone.utc).replace(microsecond=0)
            expired = self._write_manifest(
                backups / "expired",
                "brunn-managed-s3-coordinated-backup@v1",
                now - timedelta(days=3),
                now - timedelta(days=1),
            )
            command = [str(scripts / "prune-backups.sh"), "--apply", str(backups)]
            protected = subprocess.run(
                command,
                text=True,
                capture_output=True,
                check=False,
                env={**os.environ, "TZ": "UTC"},
            )
            self.assertNotEqual(protected.returncode, 0)
            self.assertTrue(expired.exists())
            self.assertIn("newer verified unexpired", protected.stderr)

            current = self._write_manifest(
                backups / "current",
                "brunn-managed-s3-coordinated-backup@v1",
                now - timedelta(hours=12),
                now + timedelta(days=2),
            )
            self._write_restore_receipt(current)
            pruned = subprocess.run(
                command,
                text=True,
                capture_output=True,
                check=False,
                env={**os.environ, "TZ": "UTC"},
            )
            self.assertEqual(pruned.returncode, 0, pruned.stderr)
            self.assertFalse(expired.exists())
            self.assertTrue(current.exists())

    @staticmethod
    def _write_manifest(
        directory: Path,
        format_name: str,
        completed_at: datetime,
        expires_at: datetime,
    ) -> Path:
        directory.mkdir()
        rendered = lambda value: value.isoformat().replace("+00:00", "Z")
        (directory / "manifest.json").write_text(
            json.dumps(
                {
                    "format": format_name,
                    "backup_id": directory.name,
                    "created_at": rendered(completed_at - timedelta(minutes=1)),
                    "completed_at": rendered(completed_at),
                    "expires_at": rendered(expires_at),
                }
            )
        )
        return directory

    @staticmethod
    def _write_restore_receipt(directory: Path) -> None:
        manifest = directory / "manifest.json"
        manifest_sha256 = hashlib.sha256(manifest.read_bytes()).hexdigest()
        backup_id = json.loads(manifest.read_text())["backup_id"]
        (directory / "restore-drill-receipt.json").write_text(
            json.dumps(
                {
                    "format": "brunn-restore-drill-receipt@v1",
                    "status": "pass",
                    "backup_id": backup_id,
                    "backup_manifest_sha256": f"sha256:{manifest_sha256}",
                    "completed_at": "2026-07-24T00:02:00Z",
                    "rto_seconds": 1,
                    "checks": {
                        "database_restore": True,
                        "object_restore": True,
                        "object_reference_integrity": True,
                        "api_ready": True,
                        "worker_ready": True,
                    },
                }
            )
        )


if __name__ == "__main__":
    unittest.main()
