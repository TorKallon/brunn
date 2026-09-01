"""Deterministic conformance tests for mixed Markdown and native assets."""

from __future__ import annotations

import json
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
EVAL_ROOT = REPO_ROOT / "eval"
if str(EVAL_ROOT) not in sys.path:
    sys.path.insert(0, str(EVAL_ROOT))

from binary_asset_fixture import (  # noqa: E402
    GENERATED_DESCRIPTION_ROOT,
    AuthorizationError,
    Credential,
    PathContractError,
    RangeContractError,
    ReferenceAssetLedger,
    directory_manifest,
    generate_fixture,
    generated_description_path,
    parse_generated_description,
    sha256_file,
    validate_vault_paths,
)


class BinaryAssetConformanceTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls._temporary = tempfile.TemporaryDirectory(prefix="brunn-state-binary-tests-")
        cls.root = Path(cls._temporary.name)
        cls.fixture_root = cls.root / "fixture"
        cls.manifest = generate_fixture(cls.fixture_root)
        cls.read_write = Credential(
            "fixture-rw",
            frozenset({"personal", "work"}),
            frozenset(
                {
                    "asset:list",
                    "asset:metadata",
                    "asset:download",
                    "asset:import",
                    "asset:delete",
                    "export:read",
                }
            ),
        )
        cls.read_only_personal = Credential(
            "fixture-ro-personal",
            frozenset({"personal"}),
            frozenset(
                {
                    "asset:list",
                    "asset:metadata",
                    "asset:download",
                    "export:read",
                }
            ),
        )
        cls.ledger = cls._import_full_history()

    @classmethod
    def tearDownClass(cls) -> None:
        cls._temporary.cleanup()

    @classmethod
    def _snapshot_root(cls, revision: str, scope: str) -> Path:
        relative = cls.manifest["snapshots"][revision]["scopes"][scope]["root"]
        return cls.fixture_root / relative

    @classmethod
    def _deletions(cls, revision: str, scope: str) -> tuple[str, ...]:
        return tuple(
            cls.manifest["snapshots"][revision]["explicit_deletions"][scope]
        )

    @classmethod
    def _import_full_history(cls) -> ReferenceAssetLedger:
        ledger = ReferenceAssetLedger()
        for revision in ("r1", "r2"):
            for scope in ("personal", "work"):
                ledger.import_snapshot(
                    credential=cls.read_write,
                    scope=scope,
                    snapshot_root=cls._snapshot_root(revision, scope),
                    revision=revision,
                    explicit_deletions=cls._deletions(revision, scope),
                )
        return ledger

    def test_fixture_generation_is_byte_deterministic(self) -> None:
        second = self.root / "fixture-second"
        second_manifest = generate_fixture(second)
        self.assertEqual(self.manifest, second_manifest)
        self.assertEqual(
            directory_manifest(self.fixture_root),
            directory_manifest(second),
        )

    def test_fixture_contains_markdown_and_multiple_native_binary_types(self) -> None:
        files = self.manifest["snapshots"]["r2"]["scopes"]["personal"]["files"]
        roles = {item["role"] for item in files}
        media_types = {item["media_type"] for item in files}
        self.assertIn("source_markdown", roles)
        self.assertIn("generated_description", roles)
        self.assertIn("native_binary", roles)
        self.assertTrue(
            {
                "image/png",
                "application/pdf",
                "application/vnd.sqlite3",
            }.issubset(media_types)
        )
        work_media_types = {
            item["media_type"]
            for item in self.manifest["snapshots"]["r2"]["scopes"]["work"]["files"]
        }
        self.assertIn("application/zip", work_media_types)

    def test_every_native_binary_has_searchable_non_authoritative_markdown(self) -> None:
        for scope in ("personal", "work"):
            entries = self.manifest["snapshots"]["r2"]["scopes"][scope]["files"]
            descriptions = {
                item["describes_path"]: item
                for item in entries
                if item["role"] == "generated_description"
            }
            for binary in (item for item in entries if item["role"] == "native_binary"):
                self.assertIn(binary["path"], descriptions)
                description = descriptions[binary["path"]]
                self.assertEqual(
                    description["path"],
                    generated_description_path(binary["path"]),
                )
                metadata = parse_generated_description(
                    (
                        self._snapshot_root("r2", scope)
                        / Path(description["path"])
                    ).read_bytes()
                )
                self.assertIs(metadata["authoritative"], False)
                self.assertIs(metadata["generated"], True)
                self.assertEqual(
                    metadata["evidence_kind"],
                    "derived_non_authoritative",
                )
                self.assertEqual(metadata["describes_path"], binary["path"])
                self.assertEqual(
                    metadata["describes_sha256"],
                    f"sha256:{binary['sha256']}",
                )

    def test_sqlite_fixture_is_a_real_queryable_dataset(self) -> None:
        database = (
            self._snapshot_root("r2", "personal")
            / "Data/StarRupture/factory.sqlite"
        )
        connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
        try:
            count, total = connection.execute(
                "SELECT count(*), sum(ingots) FROM sites"
            ).fetchone()
            blocked = connection.execute(
                "SELECT origin, destination FROM routes WHERE status='blocked'"
            ).fetchall()
            user_version = connection.execute("PRAGMA user_version").fetchone()[0]
        finally:
            connection.close()
        self.assertEqual((count, total), (4, 258))
        self.assertEqual(blocked, [("Grand Basin", "Oil Mesa")])
        self.assertEqual(user_version, 2)

    def test_current_export_preserves_every_path_byte_and_hash(self) -> None:
        export_root = self.root / "current-export"
        export_manifest = self.ledger.export_current(
            credential=self.read_write,
            destination=export_root,
        )
        self.assertEqual(export_manifest["mode"], "current")
        for scope in ("personal", "work"):
            expected = directory_manifest(self._snapshot_root("r2", scope))
            actual = directory_manifest(export_root / "current" / scope)
            self.assertEqual(actual, expected)
        for record in export_manifest["records"]:
            output = (
                export_root
                / "current"
                / record["scope"]
                / Path(record["path"])
            )
            self.assertEqual(sha256_file(output), record["sha256"])
            self.assertEqual(output.stat().st_size, record["size_bytes"])

    def test_duplicate_bytes_remain_distinct_logical_assets_with_one_blob(self) -> None:
        first = self.ledger.assets[
            ("personal", "Trips/Switzerland/Expenses/alpine-lodge-receipt.png")
        ]
        second = self.ledger.assets[
            (
                "personal",
                "Records/Taxes/2026/Receipts/alpine-lodge-receipt-copy.png",
            )
        ]
        self.assertNotEqual(first.asset_ref, second.asset_ref)
        self.assertEqual(first.current.content_hash, second.current.content_hash)
        self.assertIn(first.current.content_hash, self.ledger.physical_blobs)

        personal_shared = self.ledger.assets[
            ("personal", "Reference/Shared Architecture.png")
        ]
        work_shared = self.ledger.assets[
            ("work", "Reference/Shared Architecture.png")
        ]
        self.assertNotEqual(personal_shared.asset_ref, work_shared.asset_ref)
        self.assertEqual(
            personal_shared.current.content_hash,
            work_shared.current.content_hash,
        )
        references = [
            version
            for asset in self.ledger.assets.values()
            for version in asset.versions
            if version.content_hash == personal_shared.current.content_hash
        ]
        self.assertGreaterEqual(len(references), 2)
        self.assertEqual(
            self.ledger.physical_blobs[personal_shared.current.content_hash],
            (
                self._snapshot_root("r2", "personal")
                / "Reference/Shared Architecture.png"
            ).read_bytes(),
        )

    def test_same_path_change_keeps_identity_and_creates_new_version(self) -> None:
        logical = self.ledger.assets[
            ("personal", "Data/StarRupture/factory.sqlite")
        ]
        self.assertEqual(len(logical.versions), 2)
        self.assertEqual([item.version for item in logical.versions], [1, 2])
        self.assertNotEqual(
            logical.versions[0].content_hash,
            logical.versions[1].content_hash,
        )
        self.assertEqual(logical.current.version, 2)
        self.assertEqual(
            logical.versions[0].content_hash,
            sha256_file(
                self._snapshot_root("r1", "personal")
                / "Data/StarRupture/factory.sqlite"
            ),
        )
        self.assertEqual(
            logical.current.content_hash,
            sha256_file(
                self._snapshot_root("r2", "personal")
                / "Data/StarRupture/factory.sqlite"
            ),
        )

    def test_generated_descriptions_are_searchable_but_not_authoritative(self) -> None:
        searchable = self.ledger.searchable_markdown(
            credential=self.read_only_personal,
            scope="personal",
        )
        authoritative = self.ledger.authoritative_markdown(
            credential=self.read_only_personal,
            scope="personal",
        )
        generated = [
            path
            for path in searchable
            if path.startswith(GENERATED_DESCRIPTION_ROOT + "/")
        ]
        self.assertGreater(len(generated), 0)
        self.assertTrue(set(generated).isdisjoint(authoritative))
        self.assertIn(
            "Trips/Switzerland/Expenses/Expense Ledger.md",
            authoritative,
        )

    def test_read_only_can_list_metadata_and_download_but_cannot_mutate(self) -> None:
        assets = self.ledger.list_assets(
            credential=self.read_only_personal,
            scope="personal",
        )
        receipt = next(
            item
            for item in assets
            if item["path"]
            == "Trips/Switzerland/Expenses/alpine-lodge-receipt.png"
        )
        metadata = self.ledger.metadata(
            credential=self.read_only_personal,
            scope="personal",
            asset_ref=receipt["asset_ref"],
        )
        download = self.ledger.download(
            credential=self.read_only_personal,
            scope="personal",
            asset_ref=receipt["asset_ref"],
        )
        self.assertEqual(
            metadata["content_hash"],
            f"sha256:{download.content_hash}",
        )
        self.assertEqual(metadata["size_bytes"], len(download.content))

        with self.assertRaises(AuthorizationError):
            self.ledger.import_snapshot(
                credential=self.read_only_personal,
                scope="personal",
                snapshot_root=self._snapshot_root("r2", "personal"),
                revision="forbidden",
            )
        with self.assertRaises(AuthorizationError):
            self.ledger.delete(
                credential=self.read_only_personal,
                scope="personal",
                path=receipt["path"],
                revision="forbidden",
            )
        with self.assertRaises(AuthorizationError):
            self.ledger.list_assets(
                credential=self.read_only_personal,
                scope="work",
            )

    def test_range_downloads_are_exact_bounded_and_hash_pinned(self) -> None:
        logical = self.ledger.assets[
            ("personal", "Data/StarRupture/factory.sqlite")
        ]
        native = self.ledger.physical_blobs[logical.current.content_hash]
        cases = {
            "bytes=0-15": (0, 15),
            "bytes=32-": (32, len(native) - 1),
            "bytes=-17": (len(native) - 17, len(native) - 1),
            "bytes=4-999999": (4, len(native) - 1),
        }
        for header, (start, end) in cases.items():
            with self.subTest(header=header):
                result = self.ledger.download(
                    credential=self.read_only_personal,
                    scope="personal",
                    asset_ref=logical.asset_ref,
                    range_header=header,
                )
                self.assertEqual(result.status, 206)
                self.assertEqual(result.content, native[start : end + 1])
                self.assertEqual(
                    result.content_range,
                    f"bytes {start}-{end}/{len(native)}",
                )
                self.assertEqual(result.content_hash, logical.current.content_hash)
        for invalid in (
            "items=0-1",
            "bytes=1-2,4-5",
            f"bytes={len(native)}-",
            "bytes=-0",
            "bytes=9-3",
        ):
            with self.subTest(invalid=invalid):
                with self.assertRaises(RangeContractError):
                    self.ledger.download(
                        credential=self.read_only_personal,
                        scope="personal",
                        asset_ref=logical.asset_ref,
                        range_header=invalid,
                    )

    def test_unsafe_and_ambiguous_paths_are_rejected_before_writes(self) -> None:
        unsafe = self.manifest["unsafe_path_cases"]
        for path in unsafe["individual"]:
            with self.subTest(path=repr(path)):
                with self.assertRaises(PathContractError):
                    validate_vault_paths([path])
        for paths in unsafe["collision_sets"]:
            with self.subTest(paths=paths):
                with self.assertRaises(PathContractError):
                    validate_vault_paths(paths)

        accepted = validate_vault_paths(
            [
                "Trips/Switzerland/Expense Ledger.md",
                "Trips/Switzerland/receipts/2026-07-18.png",
            ]
        )
        self.assertEqual(len(accepted), 2)

    def test_absence_is_not_silently_converted_to_deletion(self) -> None:
        ledger = ReferenceAssetLedger()
        ledger.import_snapshot(
            credential=self.read_write,
            scope="personal",
            snapshot_root=self._snapshot_root("r1", "personal"),
            revision="r1",
        )
        ledger.import_snapshot(
            credential=self.read_write,
            scope="personal",
            snapshot_root=self._snapshot_root("r2", "personal"),
            revision="r2",
        )
        proposal = ledger.assets[
            (
                "personal",
                "Trips/Switzerland/Proposals/Lake Transfer Proposal.pdf",
            )
        ]
        self.assertTrue(proposal.active)

    def test_explicit_deletion_removes_current_file_but_preserves_history(self) -> None:
        proposal_path = (
            "Trips/Switzerland/Proposals/Lake Transfer Proposal.pdf"
        )
        description_path = generated_description_path(proposal_path)
        proposal = self.ledger.assets[("personal", proposal_path)]
        description = self.ledger.assets[("personal", description_path)]
        self.assertFalse(proposal.active)
        self.assertFalse(description.active)
        self.assertEqual(proposal.deleted_revision, "r2")
        self.assertEqual(description.deleted_revision, "r2")

        current_root = self.root / "deleted-current"
        self.ledger.export_current(
            credential=self.read_write,
            destination=current_root,
        )
        self.assertFalse(
            (current_root / "current" / "personal" / proposal_path).exists()
        )
        self.assertFalse(
            (current_root / "current" / "personal" / description_path).exists()
        )

        history_root = self.root / "deleted-history"
        history = self.ledger.export_history(
            credential=self.read_write,
            destination=history_root,
        )
        record = next(
            item
            for item in history["records"]
            if item["scope"] == "personal" and item["path"] == proposal_path
        )
        self.assertEqual(record["state"], "deleted")
        self.assertEqual(record["deleted_revision"], "r2")
        self.assertEqual(len(record["versions"]), 1)
        blob = history_root / record["versions"][0]["blob_path"]
        self.assertEqual(sha256_file(blob), record["versions"][0]["sha256"])

    def test_history_export_retains_superseded_versions_and_current_uses_head(self) -> None:
        history_root = self.root / "complete-history"
        history = self.ledger.export_history(
            credential=self.read_write,
            destination=history_root,
        )
        database_record = next(
            item
            for item in history["records"]
            if item["scope"] == "personal"
            and item["path"] == "Data/StarRupture/factory.sqlite"
        )
        self.assertEqual(database_record["state"], "active")
        self.assertEqual(
            [item["version"] for item in database_record["versions"]],
            [1, 2],
        )
        for version in database_record["versions"]:
            blob = history_root / version["blob_path"]
            self.assertTrue(blob.is_file())
            self.assertEqual(sha256_file(blob), version["sha256"])
            self.assertEqual(blob.stat().st_size, version["size_bytes"])

        current_root = self.root / "head-only-current"
        current = self.ledger.export_current(
            credential=self.read_write,
            destination=current_root,
        )
        current_record = next(
            item
            for item in current["records"]
            if item["scope"] == "personal"
            and item["path"] == "Data/StarRupture/factory.sqlite"
        )
        self.assertEqual(current_record["version"], 2)
        self.assertEqual(
            current_record["sha256"],
            database_record["versions"][-1]["sha256"],
        )
        self.assertNotEqual(
            current_record["sha256"],
            database_record["versions"][0]["sha256"],
        )


class BinaryAssetReasoningCardTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.cards_path = EVAL_ROOT / "binary_asset_reasoning_cards.json"
        cls.schema_path = EVAL_ROOT / "binary_asset_reasoning_cards.schema.json"
        cls.document = json.loads(cls.cards_path.read_text(encoding="utf-8"))
        cls.schema = json.loads(cls.schema_path.read_text(encoding="utf-8"))

    def test_card_and_schema_documents_are_machine_readable(self) -> None:
        self.assertEqual(self.document["benchmark_version"], "1.0")
        self.assertEqual(
            self.document["fixture"]["format"],
            "brunn-state-binary-fixture-v1",
        )
        self.assertEqual(self.document["scorecard"]["priority"], "quality_first")
        self.assertTrue(
            self.document["scorecard"]["quality_gate"][
                "native_hash_verification_required"
            ]
        )
        self.assertEqual(
            self.schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema",
        )
        self.assertGreaterEqual(len(self.document["cards"]), 6)

    def test_card_ids_evidence_ids_and_claim_ids_are_unique(self) -> None:
        card_ids = [card["id"] for card in self.document["cards"]]
        self.assertEqual(len(card_ids), len(set(card_ids)))
        for card in self.document["cards"]:
            with self.subTest(card=card["id"]):
                evidence_ids = [item["id"] for item in card["evidence"]]
                claim_ids = [item["id"] for item in card["claims"]]
                self.assertEqual(len(evidence_ids), len(set(evidence_ids)))
                self.assertEqual(len(claim_ids), len(set(claim_ids)))
                known = set(evidence_ids)
                for claim in card["claims"]:
                    self.assertTrue(set(claim["evidence_ids"]).issubset(known))

    def test_required_workloads_and_baselines_are_present(self) -> None:
        workloads = {card["workload"] for card in self.document["cards"]}
        self.assertTrue(
            {
                "receipts_images",
                "sqlite_sql_datasets",
                "maps_images",
                "vacation_planning",
                "kids_sports",
                "product_work",
            }.issubset(workloads)
        )
        self.assertEqual(
            set(self.document["conditions"]),
            {
                "brunn-state",
                "matched-filesystem",
                "operational-filesystem",
            },
        )
        matched = self.document["conditions"]["matched-filesystem"]
        operational = self.document["conditions"]["operational-filesystem"]
        self.assertIn("content-matched", matched["corpus_projection"].casefold())
        self.assertIn("byte-identical", matched["corpus_projection"].casefold())
        self.assertIn("full current snapshot", operational["corpus_projection"].casefold())
        self.assertEqual(
            self.document["scorecard"]["comparison"]["primary"],
            "claim_and_forbidden_error_parity_or_better",
        )

    def test_every_card_requires_native_evidence_and_authority_boundaries(self) -> None:
        for card in self.document["cards"]:
            with self.subTest(card=card["id"]):
                authorities = {item["authority"] for item in card["evidence"]}
                kinds = {item["kind"] for item in card["evidence"]}
                self.assertIn("asset_path", kinds)
                self.assertIn("native", authorities)
                self.assertIn("source", authorities)
                self.assertGreaterEqual(len(card["claims"]), 3)
                self.assertGreaterEqual(len(card["verification_steps"]), 1)
                self.assertGreaterEqual(len(card["forbidden"]), 1)

    def test_all_card_evidence_resolves_in_fixture(self) -> None:
        with tempfile.TemporaryDirectory(prefix="brunn-state-card-fixture-") as temporary:
            root = Path(temporary)
            manifest = generate_fixture(root)
            for card in self.document["cards"]:
                snapshot_root = (
                    root
                    / manifest["snapshots"]["r2"]["scopes"][card["scope"]]["root"]
                )
                with self.subTest(card=card["id"]):
                    for evidence in card["evidence"]:
                        if evidence["kind"] == "generated_description":
                            path = generated_description_path(evidence["path"])
                        else:
                            path = evidence["path"]
                        self.assertTrue(
                            (snapshot_root / Path(path)).is_file(),
                            f"{card['id']} evidence is missing: {path}",
                        )

    def test_image_only_challenge_values_do_not_leak_into_searchable_text(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            generate_fixture(root)
            searchable = "\n".join(
                path.read_text(encoding="utf-8", errors="replace")
                for path in (root / "evaluation" / "r2").rglob("*")
                if path.is_file()
                and path.suffix.casefold()
                in {".md", ".txt", ".json", ".sql", ".ics"}
            )
        for card in self.document["cards"]:
            for native_only_value in card["native_only_values"]:
                with self.subTest(card=card["id"], value=native_only_value):
                    self.assertNotIn(native_only_value, searchable)


if __name__ == "__main__":
    unittest.main()
