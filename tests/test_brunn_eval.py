import tempfile
import unittest
from pathlib import Path

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from brunn_eval import (  # noqa: E402
    BM25Index,
    build_corpus,
    normalize_text,
    one_shot_pack,
    score_pack,
    split_section_lines,
    tokenize,
    validate_manifest,
    workspace_pack,
)


class BrunnEvalTests(unittest.TestCase):
    def test_normalization_and_tokens(self):
        self.assertEqual(normalize_text("  One\n two  "), "one two")
        tokens = tokenize("Use memory.save for project:summer-trip")
        self.assertIn("memory.save", tokens)
        self.assertIn("memory", tokens)
        self.assertIn("summer", tokens)

    def test_section_split_keeps_overlap(self):
        lines = [f"line {index} " + "x" * 80 for index in range(12)]
        chunks = split_section_lines(lines, 10, target_chars=300, overlap_chars=100)
        self.assertGreater(len(chunks), 1)
        self.assertEqual(chunks[0][0], 10)
        self.assertLessEqual(chunks[1][0], chunks[0][1])
        self.assertIn("line 11", chunks[-1][2])

    def test_retrieval_and_gold_scoring(self):
        with tempfile.TemporaryDirectory() as temp:
            vault = Path(temp)
            project = vault / "Projects" / "Tiny"
            project.mkdir(parents=True)
            (project / "Alpha.md").write_text(
                "# Alpha\n\nThe source of truth is the evidence ledger.\n",
                encoding="utf-8",
            )
            (project / "Beta.md").write_text(
                "# Beta\n\nDerived summaries require review before canonical promotion.\n",
                encoding="utf-8",
            )
            manifest = {
                "corpus_roots": [{"project": "Tiny", "path": "Projects/Tiny", "glob": "*.md"}],
                "budgets": {
                    "chunk_chars": 200,
                    "chunk_overlap_chars": 20,
                    "one_shot_chars": 500,
                    "one_shot_chunks": 2,
                    "workspace_chars": 800,
                    "workspace_initial_chunks": 2,
                    "workspace_expansion_chunks": 2,
                    "workspace_materialize_project_chars": 2000,
                    "direct_chars": 2000,
                    "direct_files": 2,
                },
                "cases": [],
            }
            case = {
                "id": "tiny",
                "question": "What is the source of truth and what needs review?",
                "gold": [
                    {"path": "Projects/Tiny/Alpha.md", "contains": "source of truth is the evidence ledger"},
                    {"path": "Projects/Tiny/Beta.md", "contains": "require review before canonical promotion"},
                ],
            }
            manifest["cases"] = [case]
            self.assertEqual(validate_manifest(vault, manifest), [])
            documents, chunks = build_corpus(vault, manifest)
            index = BM25Index(chunks)
            one_shot, _ = one_shot_pack(case["question"], index, manifest["budgets"])
            workspace, trace = workspace_pack(
                case["question"], documents, chunks, index, manifest["budgets"]
            )
            self.assertEqual(score_pack(case, one_shot)["evidence_total"], 2)
            self.assertTrue(score_pack(case, workspace)["case_pass"])
            self.assertEqual(trace["mode"], "materialize_project")

            filtered_documents, _ = build_corpus(
                vault,
                manifest,
                excluded_paths=[project / "Beta.md"],
            )
            self.assertEqual([document.path for document in filtered_documents], ["Projects/Tiny/Alpha.md"])


if __name__ == "__main__":
    unittest.main()
