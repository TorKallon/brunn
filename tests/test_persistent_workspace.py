import json
import threading
import unittest
import urllib.request
from pathlib import Path
from tempfile import TemporaryDirectory

from straylight import StraylightService, StraylightStore
from straylight.embeddings import HashingEmbeddingProvider
from straylight_http import build_server


ROOT = Path(__file__).resolve().parents[1]


class PersistentWorkspaceTests(unittest.TestCase):
    def make_store(self, root: Path) -> tuple[StraylightStore, dict, dict]:
        store = StraylightStore(root / "straylight.db")
        base = store.ingest_documents(
            {
                "Projects/Alpha/Plan.md": (
                    "# Alpha plan\n\nThe earlier design uses a lexical index and a durable checkpoint.\n"
                )
            },
            note="base",
        )
        seed = store.create_checkpoint(
            base["seq"],
            {
                "objective": "Continue Alpha",
                "current_state": ["The lexical prototype is complete"],
                "decisions": ["Preserve source evidence"],
                "open_questions": ["How should semantic retrieval work?"],
                "next_actions": ["Build the persistent service"],
                "artifacts": ["Projects/Alpha/Plan.md"],
            },
            ["Projects/Alpha/Plan.md"],
        )
        delta = store.ingest_documents(
            {
                "Projects/Alpha/Delta.md": (
                    "# New constraint\n\nUse hybrid semantic and lexical retrieval in a snapshot-pinned session.\n"
                )
            },
            note="delta",
        )
        return store, seed, delta

    def test_checkpoint_reopen_delta_and_child_lineage(self):
        with TemporaryDirectory() as temp:
            store, seed, delta = self.make_store(Path(temp))
            service = StraylightService(store)
            opened = service.open({
                "task": "Advance Alpha",
                "scopes": ["Alpha"],
                "checkpoint_id": seed["checkpoint_id"],
            })
            self.assertEqual(opened["corpus_revision"], delta["revision_id"])
            self.assertEqual(opened["resumed_checkpoint"]["checkpoint_id"], seed["checkpoint_id"])
            self.assertEqual(len(opened["changes_since_checkpoint"]), 1)
            self.assertIn("hybrid semantic", opened["changes_since_checkpoint"][0]["content"])

            child = service.checkpoint({
                "session_id": opened["session_id"],
                "parent_checkpoint_id": seed["checkpoint_id"],
                "state": {
                    "objective": "Advance Alpha with hybrid retrieval",
                    "current_state": ["The constraint changed"],
                    "decisions": ["Build a persistent hybrid index"],
                    "open_questions": [],
                    "next_actions": ["Run transition tests"],
                    "artifacts": ["Projects/Alpha/Plan.md", "Projects/Alpha/Delta.md"],
                },
                "source_refs": ["Projects/Alpha/Plan.md", "Projects/Alpha/Delta.md"],
            })
            self.assertEqual(child["parent_checkpoint_id"], seed["checkpoint_id"])
            self.assertEqual(child["corpus_revision"], delta["revision_id"])
            self.assertEqual(service.status(opened["session_id"])["operations"]["completed_calls"], 2)

    def test_checkpoint_rejects_unknown_source_refs(self):
        with TemporaryDirectory() as temp:
            store, seed, _ = self.make_store(Path(temp))
            service = StraylightService(store)
            opened = service.open({"task": "Advance Alpha", "checkpoint_id": seed["checkpoint_id"]})
            with self.assertRaisesRegex(ValueError, "unknown source refs"):
                service.checkpoint({
                    "session_id": opened["session_id"],
                    "parent_checkpoint_id": seed["checkpoint_id"],
                    "state": {"objective": "Bad checkpoint"},
                    "source_refs": ["Projects/Alpha/Missing.md"],
                })

    def test_persistent_hybrid_index(self):
        with TemporaryDirectory() as temp:
            store, seed, _ = self.make_store(Path(temp))
            provider = HashingEmbeddingProvider()
            indexed = store.index_embeddings(provider)
            self.assertEqual(indexed["indexed"], 2)
            service = StraylightService(store, provider)
            opened = service.open({"task": "Find related design", "checkpoint_id": seed["checkpoint_id"]})
            result = service.query({
                "session_id": opened["session_id"],
                "queries": [{"query": "meaning based search", "scope": "Alpha", "limit": 4}],
            })
            self.assertEqual(result["results"][0]["status"], "complete")
            self.assertIn("semantic", result["results"][0]["modes"])
            self.assertTrue(result["results"][0]["results"])

    def test_empty_source_spans_are_not_embedded(self):
        with TemporaryDirectory() as temp:
            store = StraylightStore(Path(temp) / "straylight.db")
            store.ingest_documents(
                {"Projects/Alpha/Empty.md": "# Empty\n\n", "Projects/Alpha/Full.md": "# Full\n\nEvidence."},
                note="empty span regression",
            )
            with store.connection() as connection:
                contents = [row["content"] for row in connection.execute("SELECT content FROM chunks")]
            self.assertTrue(contents)
            self.assertTrue(all(content.strip() for content in contents))
            provider = HashingEmbeddingProvider()
            indexed = store.index_embeddings(provider)
            self.assertEqual(indexed["indexed"], len(contents))

    def test_http_api_round_trip(self):
        with TemporaryDirectory() as temp:
            store, seed, _ = self.make_store(Path(temp))
            server = build_server(store.path, port=0)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                request = urllib.request.Request(
                    f"http://127.0.0.1:{server.server_port}/v1/memory/open",
                    data=json.dumps({
                        "task": "Advance Alpha",
                        "scopes": ["Alpha"],
                        "checkpoint_id": seed["checkpoint_id"],
                    }).encode(),
                    headers={"Content-Type": "application/json"},
                    method="POST",
                )
                with urllib.request.urlopen(request) as response:
                    opened = json.load(response)
                self.assertEqual(opened["status"], "complete")
                self.assertEqual(opened["resumed_checkpoint"]["checkpoint_id"], seed["checkpoint_id"])
            finally:
                server.shutdown()
                server.server_close()
                thread.join(timeout=2)

    def test_transition_cards_reference_frozen_sources(self):
        manifest = json.loads((ROOT / "eval" / "transition_cases.json").read_text())
        seeds = json.loads((ROOT / manifest["seed_results"]).read_text())
        seed_ids = {
            record["case_id"] for record in seeds["records"]
            if record["condition"] == "workspace" and record.get("workspace_checkpoint")
        }
        corpus_paths = {
            path.relative_to(ROOT / manifest["base_corpus_root"]).as_posix()
            for path in (ROOT / manifest["base_corpus_root"]).rglob("*") if path.is_file()
        }
        self.assertEqual(len(manifest["cases"]), 5)
        self.assertEqual({case["workload"] for case in manifest["cases"]}, {
            "Warmind", "Charlemagne", "Star Rupture", "Switzerland", "Straylight"
        })
        for case in manifest["cases"]:
            self.assertIn(case["seed_case_id"], seed_ids)
            self.assertTrue((ROOT / case["delta_path"]).is_file())
            self.assertEqual(set(case["claim_slots"]), {rubric["id"] for rubric in case["rubric"]})
            for rubric in case["rubric"]:
                for source in rubric["sources_any"]:
                    self.assertTrue(source in corpus_paths or (ROOT / source).is_file(), source)


if __name__ == "__main__":
    unittest.main()
