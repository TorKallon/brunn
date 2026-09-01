from __future__ import annotations

import hashlib
import json
import subprocess
import tempfile
import unittest
from datetime import UTC, datetime, timedelta
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PAYLOADS = [
    "postgres.dump",
    "minio-data.tar",
    "db-inventory.txt",
    "minio-versions.jsonl",
]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    digest.update(path.read_bytes())
    return digest.hexdigest()


def create_backup(
    root: Path,
    name: str,
    *,
    expires_at: datetime,
    version: str = "brunn-coordinated-backup@v2",
) -> Path:
    backup = root / name
    backup.mkdir()
    for payload in PAYLOADS:
        (backup / payload).write_text(f"{name}:{payload}\n")
    created_at = datetime.now(UTC) - timedelta(days=1)
    manifest = {
        "format": version,
        "backup_id": name,
        "created_at": created_at.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "completed_at": (created_at + timedelta(minutes=1)).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        ),
        "expires_at": expires_at.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "retention_days": 30,
        "consistency": {"writers_stopped": True},
    }
    (backup / "manifest.json").write_text(json.dumps(manifest, sort_keys=True) + "\n")
    checksums = [
        f"{sha256(backup / filename)}  {filename}"
        for filename in [*PAYLOADS, "manifest.json"]
    ]
    (backup / "CHECKSUMS.sha256").write_text("\n".join(checksums) + "\n")
    return backup


def add_restore_receipt(backup: Path) -> None:
    manifest = json.loads((backup / "manifest.json").read_text())
    receipt = {
        "format": "brunn-restore-drill-receipt@v1",
        "status": "pass",
        "backup_id": manifest["backup_id"],
        "backup_manifest_sha256": f"sha256:{sha256(backup / 'manifest.json')}",
        "completed_at": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "rto_seconds": 1,
        "checks": {
            "database_restore": True,
            "object_restore": True,
            "object_reference_integrity": True,
            "api_ready": True,
            "worker_ready": True,
        },
    }
    (backup / "restore-drill-receipt.json").write_text(
        json.dumps(receipt, sort_keys=True) + "\n"
    )


class BackupRetentionTests(unittest.TestCase):
    def test_prune_is_dry_run_by_default_and_only_deletes_verified_expired_v2(self):
        with tempfile.TemporaryDirectory() as temp:
            backup_root = Path(temp)
            now = datetime.now(UTC)
            expired = create_backup(
                backup_root,
                "expired",
                expires_at=now - timedelta(hours=1),
            )
            current = create_backup(
                backup_root,
                "current",
                expires_at=now + timedelta(days=1),
            )
            add_restore_receipt(current)
            legacy = create_backup(
                backup_root,
                "legacy",
                expires_at=now - timedelta(hours=1),
                version="brunn-coordinated-backup@v1",
            )

            dry_run = subprocess.run(
                [str(ROOT / "scripts/prune-backups.sh"), str(backup_root)],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=True,
            )
            self.assertIn("expired=1 kept=1 skipped=1 apply=false", dry_run.stdout)
            self.assertTrue(expired.exists())
            self.assertTrue(current.exists())
            self.assertTrue(legacy.exists())

            applied = subprocess.run(
                [
                    str(ROOT / "scripts/prune-backups.sh"),
                    "--apply",
                    str(backup_root),
                ],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=True,
            )
            self.assertIn("expired=1 kept=1 skipped=1 apply=true", applied.stdout)
            self.assertFalse(expired.exists())
            self.assertTrue(current.exists())
            self.assertTrue(legacy.exists())

    def test_prune_refuses_an_expired_backup_with_a_tampered_manifest(self):
        with tempfile.TemporaryDirectory() as temp:
            backup_root = Path(temp)
            backup = create_backup(
                backup_root,
                "tampered",
                expires_at=datetime.now(UTC) - timedelta(hours=1),
            )
            manifest = json.loads((backup / "manifest.json").read_text())
            manifest["retention_days"] = 1
            (backup / "manifest.json").write_text(json.dumps(manifest) + "\n")

            result = subprocess.run(
                [
                    str(ROOT / "scripts/prune-backups.sh"),
                    "--apply",
                    str(backup_root),
                ],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertNotEqual(0, result.returncode)
            self.assertTrue(backup.exists())


if __name__ == "__main__":
    unittest.main()
