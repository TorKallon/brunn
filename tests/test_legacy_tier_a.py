from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
import uuid
from pathlib import Path

from legacy_tier_a import (
    AUDIT_SCHEMA,
    COMPOSITE_SCHEMA,
    LEGACY_CURRENT_FORMAT,
    LEGACY_DELTA_FORMAT,
    LEGACY_MANIFEST_FORMAT,
    LEGACY_NATIVE_FORMAT,
    EXACT_HISTORY_SEMANTICS,
    ORDINARY_HISTORY_SEMANTICS,
    PORTABLE_COMPANION_FORMAT,
    TIER_A_HISTORY_STAGE_FORMAT,
    TierAError,
    compose,
    expected_service_history,
    local_audit,
    materialize_stage,
    render_native_record,
    rebuild_native_materialization,
    roundtrip_audit,
    sha256_value,
    stage_history_semantics,
    validate_checkpoint_graph,
    verify_checksum_tree,
)


ROOT = Path(__file__).resolve().parents[1]
STABLE_IMPORT_ID = "vault:synthetic-owner"
PARENT_ID = "11111111-1111-4111-8111-111111111111"
CHILD_ID = "22222222-2222-4222-8222-222222222222"


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def write_checksums(root: Path) -> None:
    rows = []
    for path in root.rglob("*"):
        if not path.is_file() or path.name == "CHECKSUMS.sha256":
            continue
        relative = path.relative_to(root).as_posix()
        rows.append((relative, hashlib.sha256(path.read_bytes()).hexdigest()))
    rows.sort()
    (root / "CHECKSUMS.sha256").write_text(
        "".join(f"{digest}  {relative}\n" for relative, digest in rows),
        encoding="utf-8",
    )


def source_ref(value: int) -> str:
    return f"source:00000000-0000-4000-8000-{value:012d}"


def asset_ref(value: int) -> str:
    return f"asset:00000000-0000-4000-8000-{value:012d}"


def history_entry(
    *,
    path: str,
    payload: bytes,
    source_number: int,
    active: bool,
    captured_at: str,
    media_type: str = "text/markdown",
    source_kind: str = "content_pack",
    category: str = "source",
    original_path: str | None = None,
) -> dict:
    return {
        "path": path,
        "original_path": original_path,
        "import_metadata": {"modified_unix_ns": 1_700_000_000_000_000_000, "mode": 0o600},
        "category": category,
        "source_ref": source_ref(source_number),
        "source_kind": source_kind,
        "description_status": "complete" if category == "generated_description" else None,
        "description_method": "fixture" if category == "generated_description" else None,
        "asset_ref": asset_ref(source_number),
        "version": 1,
        "previous_version": None,
        "content_hash": sha256_value(payload),
        "size_bytes": len(payload),
        "media_type": media_type,
        "captured_at": captured_at,
        "stored_at": captured_at,
        "active": active,
    }


def current_entry(path: str, entry: dict, payload: bytes) -> dict:
    return {
        "path": path,
        "content_hash": sha256_value(payload),
        "size_bytes": len(payload),
        "media_type": entry["media_type"],
        "source_ref": entry["source_ref"],
        "source_kind": entry["source_kind"],
        "asset_ref": entry["asset_ref"],
        "asset_version": 1,
        "active": True,
        "modified_unix_ns": 1_700_000_000_000_000_000,
        "mode": 0o600,
    }


def native_checkpoint(checkpoint_id: str, parent_id: str | None) -> dict:
    return {
        "record_ref": f"checkpoint:{checkpoint_id}",
        "record_kind": "checkpoint",
        "record_version": None,
        "title": f"Checkpoint {checkpoint_id[:4]}",
        "payload": {
            "record": {
                "id": checkpoint_id,
                "parent_checkpoint_id": parent_id,
                "corpus_revision_id": "33333333-3333-4333-8333-333333333333",
                "session_id": "44444444-4444-4444-8444-444444444444",
                "summary": json.dumps(
                    {
                        "objective": "Synthetic fidelity proof",
                        "current_state": ["Exact bytes retained."],
                    }
                ),
            },
            "source_refs": [],
        },
    }


class SyntheticFixture:
    def __init__(
        self,
        root: Path,
        *,
        omit_description: bool = False,
        repeated_markdown_bytes: bool = False,
    ):
        self.current = root / "current"
        self.delta = root / "delta"
        self.history_path = root / "history.json"
        self.native_path = root / "native.json"
        self.current.mkdir()
        self.delta.mkdir()

        old = b"# Plan\r\n\r\nold\r\n"
        stale_current = b"# Plan\r\n\r\nstale current\r\n"
        active = old if repeated_markdown_bytes else b"# Plan\r\n\r\nactive\r\n"
        binary = b"\x89PNG\r\n\x1a\nsynthetic"
        description = b"# Exact binary description\r\n\r\nbyte copied\r\n"

        plan_old = history_entry(
            path="Notes/Plan.md",
            payload=old,
            source_number=1,
            active=False,
            captured_at="2026-07-26T00:00:00Z",
        )
        plan_active = history_entry(
            path="Notes/Plan.md",
            payload=active,
            source_number=2,
            active=True,
            captured_at="2026-07-27T00:00:00Z",
        )
        binary_entry = history_entry(
            path="Assets/photo.png",
            payload=binary,
            source_number=3,
            active=True,
            captured_at="2026-07-25T00:00:00Z",
            media_type="image/png",
        )
        description_entry = history_entry(
            path=".brunn-state/descriptions/photo.md",
            payload=description,
            source_number=4,
            active=True,
            captured_at="2026-07-25T00:00:01Z",
            source_kind="derived_asset_description",
            category="generated_description",
            original_path="Assets/photo.png",
        )
        history_entries = [description_entry, binary_entry, plan_old, plan_active]
        if omit_description:
            history_entries.remove(description_entry)

        current_entries = [
            current_entry("sources/Notes/Plan.md", plan_old, stale_current),
            current_entry("sources/Assets/photo.png", binary_entry, binary),
        ]
        (self.current / "sources/Notes").mkdir(parents=True)
        (self.current / "sources/Notes/Plan.md").write_bytes(stale_current)
        (self.current / "sources/Assets").mkdir(parents=True)
        (self.current / "sources/Assets/photo.png").write_bytes(binary)
        if not omit_description:
            current_entries.append(
                current_entry(
                    "workspace/assets/Assets/photo.png.md",
                    description_entry,
                    description,
                )
            )
            (self.current / "workspace/assets/Assets").mkdir(parents=True)
            (self.current / "workspace/assets/Assets/photo.png.md").write_bytes(
                description
            )

        current_manifest = {
            "format": LEGACY_CURRENT_FORMAT,
            "version": 1,
            "stable_import_id": STABLE_IMPORT_ID,
            "entries": current_entries,
        }
        write_json(self.current / "manifest.json", current_manifest)
        write_checksums(self.current)

        history = {
            "format": LEGACY_MANIFEST_FORMAT,
            "stable_import_id": STABLE_IMPORT_ID,
            "corpus_revision": "revision:33333333-3333-4333-8333-333333333333",
            "include_history": True,
            "include_scope_memory": False,
            "entries": history_entries,
            "native_records": [],
        }
        write_json(self.history_path, history)

        inactive_path = (
            "history/sources/Notes/Plan.md/v1-"
            + sha256_value(old).removeprefix("sha256:")[:16]
        )
        (self.delta / inactive_path).parent.mkdir(parents=True)
        (self.delta / inactive_path).write_bytes(old)
        active_path = "sources/Notes/Plan.md"
        (self.delta / active_path).parent.mkdir(parents=True, exist_ok=True)
        (self.delta / active_path).write_bytes(active)
        delta_entries = [
            {
                **current_entry(inactive_path, plan_old, old),
                "active": False,
                "reason": "inactive_history",
            },
            {
                **current_entry(active_path, plan_active, active),
                "active": True,
                "reason": "active_drift",
            },
        ]
        delta_manifest = {
            "format": LEGACY_DELTA_FORMAT,
            "stable_import_id": STABLE_IMPORT_ID,
            "corpus_revision": history["corpus_revision"],
            "source_manifest_sha256": sha256_value(self.history_path.read_bytes()),
            "base_current_manifest_sha256": sha256_value(
                (self.current / "manifest.json").read_bytes()
            ),
            "active_drift_count": 1,
            "inactive_history_count": 1,
            "entries": delta_entries,
        }
        write_json(self.delta / "manifest.json", delta_manifest)
        write_checksums(self.delta)

        native = {
            "schema": LEGACY_NATIVE_FORMAT,
            "stable_import_id": STABLE_IMPORT_ID,
            "corpus_revision": history["corpus_revision"],
            "records": [
                native_checkpoint(PARENT_ID, None),
                native_checkpoint(CHILD_ID, PARENT_ID),
                {
                    "record_ref": "claim:55555555-5555-4555-8555-555555555555",
                    "record_kind": "claim",
                    "record_version": None,
                    "title": "Synthetic claim",
                    "payload": {"record": {"id": "55555555-5555-4555-8555-555555555555"}},
                },
            ],
        }
        write_json(self.native_path, native)


class LegacyTierATests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="legacy-tier-a-test-")
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def compose_fixture(self, *, omit_description: bool = False) -> tuple[SyntheticFixture, Path, dict]:
        fixture = SyntheticFixture(self.root, omit_description=omit_description)
        output = self.root / "composite"
        artifact = compose(
            current_root=fixture.current,
            history_manifest_path=fixture.history_path,
            delta_root=fixture.delta,
            native_records_path=fixture.native_path,
            output=output,
        )
        return fixture, output, artifact

    def test_composes_exact_history_without_copying_or_regenerating_descriptions(self) -> None:
        _, output, artifact = self.compose_fixture()

        self.assertEqual(artifact["schema"], COMPOSITE_SCHEMA)
        self.assertEqual(artifact["totals"]["logical_paths"], 3)
        self.assertEqual(artifact["totals"]["versions"], 4)
        self.assertEqual(artifact["totals"]["max_versions_per_path"], 2)
        self.assertEqual(artifact["totals"]["binary_descriptions"], 1)
        self.assertEqual(
            artifact["totals"]["content_origins"],
            {"base_current": 2, "delta": 2},
        )
        audit = local_audit(output)
        self.assertEqual(audit["schema"], AUDIT_SCHEMA)
        self.assertEqual(audit["verdict"], "pass")
        self.assertEqual(audit["counts"]["differences"], 0)

        binary = next(
            entry
            for entry in artifact["entries"]
            if entry["path"] == "sources/Assets/photo.png"
        )
        description = next(
            entry
            for entry in artifact["entries"]
            if entry["path"] == "workspace/assets/Assets/photo.png.md"
        )
        self.assertEqual(
            binary["binary_description_path"],
            description["path"],
        )
        self.assertEqual(
            (output / description["archive_path"]).read_bytes(),
            b"# Exact binary description\r\n\r\nbyte copied\r\n",
        )
        source_inode = (
            self.root / "current/workspace/assets/Assets/photo.png.md"
        ).stat().st_ino
        self.assertEqual((output / description["archive_path"]).stat().st_ino, source_inode)

    def test_materializes_bounded_replay_stages_with_exact_companion_contract(self) -> None:
        _, output, artifact = self.compose_fixture()
        stage_zero = self.root / "stage-zero"
        first = materialize_stage(output, stage_index=0, output=stage_zero)
        self.assertEqual(first["tier_a"]["stage_count"], 2)
        self.assertEqual(len(first["entries"]), 3)
        binary = next(entry for entry in first["entries"] if entry["kind"] == "binary")
        description = next(
            entry
            for entry in first["entries"]
            if entry["metadata"].get("kind") == "binary_description"
        )
        self.assertEqual(
            binary["metadata"]["_brunn_tier_a"]["format"],
            PORTABLE_COMPANION_FORMAT,
        )
        self.assertEqual(
            binary["metadata"]["_brunn_tier_a"]["binary_description_path"],
            description["path"],
        )
        self.assertEqual(
            description["metadata"]["_brunn_tier_a_history"],
            {
                "format": TIER_A_HISTORY_STAGE_FORMAT,
                "target_lineage_ordinal": 1,
                "semantics": ORDINARY_HISTORY_SEMANTICS,
            },
        )
        self.assertEqual(
            verify_checksum_tree(stage_zero)["manifest.json"],
            sha256_value((stage_zero / "manifest.json").read_bytes()),
        )

        stage_one = self.root / "stage-one"
        second = materialize_stage(
            output,
            stage_index=1,
            output=stage_one,
            include_native=True,
        )
        self.assertEqual(len(second["entries"]), 7)
        self.assertEqual(
            {entry["path"] for entry in second["entries"]},
            {
                "sources/Notes/Plan.md",
                f"memory/checkpoint/{PARENT_ID}.md",
                f"memory/checkpoint/{CHILD_ID}.md",
                "memory/claim/55555555-5555-4555-8555-555555555555.md",
                "Brunn Migration/Native/legacy-native-records.json",
                "Brunn Migration/Native/legacy-native-records.index.json",
                "Brunn Migration/Native/legacy-native-records.description.md",
            },
        )
        self.assertEqual(artifact["native"]["records"]["count"], 3)
        self.assertEqual(len(artifact["native"]["materialized"]), 3)

    def test_materialized_markdown_marks_an_intentional_same_byte_version(self) -> None:
        fixture = SyntheticFixture(self.root, repeated_markdown_bytes=True)
        composite = self.root / "same-byte-composite"
        compose(
            current_root=fixture.current,
            history_manifest_path=fixture.history_path,
            delta_root=fixture.delta,
            native_records_path=fixture.native_path,
            output=composite,
        )
        stage_zero = materialize_stage(
            composite,
            stage_index=0,
            output=self.root / "same-byte-stage-zero",
        )
        stage_one = materialize_stage(
            composite,
            stage_index=1,
            output=self.root / "same-byte-stage-one",
        )
        first = next(
            entry
            for entry in stage_zero["entries"]
            if entry["path"] == "sources/Notes/Plan.md"
        )
        second = next(
            entry
            for entry in stage_one["entries"]
            if entry["path"] == "sources/Notes/Plan.md"
        )

        self.assertEqual(first["content_hash"], second["content_hash"])
        self.assertEqual(
            first["metadata"]["_brunn_tier_a_history"],
            {
                "format": TIER_A_HISTORY_STAGE_FORMAT,
                "target_lineage_ordinal": 1,
                "semantics": ORDINARY_HISTORY_SEMANTICS,
            },
        )
        self.assertEqual(
            second["metadata"]["_brunn_tier_a_history"],
            {
                "format": TIER_A_HISTORY_STAGE_FORMAT,
                "target_lineage_ordinal": 2,
                "semantics": EXACT_HISTORY_SEMANTICS,
            },
        )

    def test_same_byte_binary_companion_history_fails_closed(self) -> None:
        content_hash = sha256_value(b"exact companion bytes")
        predecessor = {
            "content_hash": content_hash,
            "size_bytes": 21,
        }
        companion = {
            **predecessor,
            "describes_binary_path": "sources/Assets/photo.png",
        }

        with self.assertRaisesRegex(TierAError, "binary companion history is unsupported"):
            stage_history_semantics(
                companion,
                predecessor,
                kind="markdown",
            )

    def test_native_record_renderer_matches_the_legacy_export_contract(self) -> None:
        rendered = render_native_record(
            {
                "record_ref": "claim:55555555-5555-4555-8555-555555555555",
                "record_kind": "claim",
                "record_version": None,
                "title": "Decision\nwith newline",
                "payload": {"value": "literal ``` fence", "verified": True},
            },
            corpus_revision="revision:test",
        ).decode("utf-8")
        self.assertIn('record_ref: "claim:55555555-5555-4555-8555-555555555555"', rendered)
        self.assertIn("record_version: null", rendered)
        self.assertIn("# Decision with newline", rendered)
        self.assertIn("````json", rendered)
        self.assertIn('"verified": true', rendered)

    def test_roundtrip_audit_verifies_every_downloaded_history_byte(self) -> None:
        _, composite_root, artifact = self.compose_fixture()
        export_root = self.root / "roundtrip"
        export_root.mkdir()
        manifest_entries = []
        expected = expected_service_history(artifact)

        def source_bytes(item: dict) -> bytes:
            for entry in artifact["entries"]:
                if (
                    entry["path"] == item["path"]
                    and entry["lineage_ordinal"] == item["version"]
                ):
                    return (composite_root / entry["archive_path"]).read_bytes()
            for entry in artifact["native"]["materialized"]:
                if entry["path"] == item["path"]:
                    return (composite_root / entry["archive_path"]).read_bytes()
            native = artifact["native"]
            if item["path"] == native["archive"]["path"]:
                return (composite_root / native["archive"]["path"]).read_bytes()
            if item["path"] == native["index"]["path"]:
                return (composite_root / native["index"]["path"]).read_bytes()
            if item["path"] == native["archive"]["description_path"]:
                return (
                    composite_root / native["archive"]["description_path"]
                ).read_bytes()
            for checkpoint in native["checkpoints"]:
                if checkpoint["path"] == item["path"]:
                    return checkpoint["content"].encode("utf-8")
            self.fail(f"no fixture bytes for {item['path']}")

        for ordinal, item in enumerate(expected, start=1):
            payload = source_bytes(item)
            history_path = f"history/{ordinal:06d}/v{item['version']:020d}"
            destination = export_root / history_path
            destination.parent.mkdir(parents=True)
            destination.write_bytes(payload)
            common = {
                **item,
                "entry_ref": f"entry:{uuid.uuid4()}",
                "media_type": (
                    "text/markdown"
                    if item["kind"] == "markdown"
                    else "application/octet-stream"
                ),
                "deleted": False,
                "created_at": None,
                "metadata": {},
                "modified_unix_ns": None,
                "mode": 0o600,
            }
            manifest_entries.append({**common, "archive_path": history_path})
            if item["current"]:
                current_path = f"workspace/{item['path']}"
                current = export_root / current_path
                current.parent.mkdir(parents=True, exist_ok=True)
                current.write_bytes(payload)
                manifest_entries.append({**common, "archive_path": current_path})
        write_json(
            export_root / "manifest.json",
            {
                "format": "brunn-workspace-export@v1",
                "version": 1,
                "workspace_generation": 1,
                "workspace_generation_is_snapshot": False,
                "entries": manifest_entries,
            },
        )
        write_checksums(export_root)

        result = roundtrip_audit(composite_root, export_root=export_root)
        self.assertEqual(result["verdict"], "pass")
        self.assertEqual(result["counts"]["differences"], 0)
        self.assertEqual(
            result["counts"]["expected_history_versions"],
            len(expected),
        )

    def test_rebuilds_native_materializations_from_a_verified_predecessor(self) -> None:
        _, composite_root, predecessor = self.compose_fixture()
        rebuilt_root = self.root / "rebuilt"
        rebuilt = rebuild_native_materialization(
            composite_root,
            output=rebuilt_root,
        )
        self.assertEqual(
            rebuilt["source"]["predecessor_composite_sha256"],
            predecessor["composite_sha256"],
        )
        self.assertEqual(len(rebuilt["native"]["materialized"]), 3)
        self.assertEqual(local_audit(rebuilt_root)["verdict"], "pass")

    def test_checkpoint_graph_is_topological_and_fails_on_missing_parent(self) -> None:
        _, _, artifact = self.compose_fixture()
        ordered = validate_checkpoint_graph(artifact["native"]["checkpoints"])
        self.assertEqual(
            [item["checkpoint_ref"] for item in ordered],
            [f"checkpoint:{PARENT_ID}", f"checkpoint:{CHILD_ID}"],
        )
        broken = [dict(item) for item in ordered]
        broken[1]["parent_checkpoint_ref"] = (
            "checkpoint:" + str(uuid.uuid4())
        )
        with self.assertRaises(TierAError):
            validate_checkpoint_graph(broken)

    def test_fails_closed_when_a_binary_lacks_its_exact_legacy_description(self) -> None:
        fixture = SyntheticFixture(self.root, omit_description=True)
        with self.assertRaisesRegex(TierAError, "no exact byte-copied description"):
            compose(
                current_root=fixture.current,
                history_manifest_path=fixture.history_path,
                delta_root=fixture.delta,
                native_records_path=fixture.native_path,
                output=self.root / "blocked-composite",
            )

    def test_fails_closed_when_a_pinned_delta_byte_changes(self) -> None:
        fixture = SyntheticFixture(self.root)
        (fixture.delta / "sources/Notes/Plan.md").write_bytes(b"tampered")
        with self.assertRaisesRegex(TierAError, "checksum mismatch"):
            compose(
                current_root=fixture.current,
                history_manifest_path=fixture.history_path,
                delta_root=fixture.delta,
                native_records_path=fixture.native_path,
                output=self.root / "blocked-composite",
            )

    def test_committed_artifact_schemas_are_valid_json(self) -> None:
        for relative in (
            "eval/tier_a_legacy_composite.schema.json",
            "eval/tier_a_legacy_audit.schema.json",
            "eval/tier_a_checkpoint_import.schema.json",
        ):
            value = json.loads((ROOT / relative).read_text(encoding="utf-8"))
            self.assertEqual(value["$schema"], "https://json-schema.org/draft/2020-12/schema")
            self.assertEqual(value["type"], "object")


if __name__ == "__main__":
    unittest.main()
