import assert from "node:assert/strict";
import test from "node:test";

import { compactReasoningResponse } from "./reasoning-view.js";

test("open removes corpus samples and ranking mechanics while preserving evidence", () => {
  const compact = compactReasoningResponse("memory.open", {
    request_id: "request:1",
    session_id: "session:1",
    corpus_revision: "revision:1",
    status: "complete",
    freshness: {
      source_updated_at: "2026-07-22T00:00:00Z",
      normalized_at: "2026-07-22T00:00:01Z",
    },
    coverage: {
      searched: [{
        lane: "lexical",
        completeness: "best_effort",
        candidate_count: 12,
        searched_count: 1000,
        index_revision: "revision:1",
      }],
      unsearched: [],
      absence_safe: false,
    },
    data: {
      session_id: "session:1",
      corpus_revision: "revision:1",
      corpus_map: {
        record_counts: { chunk: 1000 },
        available_views: ["full"],
        records: { chunk: [{ ref: "chunk:noise" }] },
        truncated: true,
      },
      initial_evidence: [{
        reference: "chunk:1",
        path: "Source.md",
        content: "Exact evidence.",
        content_hash: "sha256:noise",
        lane_scores: { lexical: 10 },
        score: 0.4,
      }],
    },
  });

  const data = compact.data as Record<string, unknown>;
  const corpusMap = data.corpus_map as Record<string, unknown>;
  const evidence = (data.initial_evidence as Array<Record<string, unknown>>)[0];
  assert.ok(evidence);
  assert.equal(corpusMap.records, undefined);
  assert.equal(evidence.content_hash, undefined);
  assert.equal(evidence.score, undefined);
  assert.equal(evidence.content, "Exact evidence.");
  assert.deepEqual(compact.coverage, {
    absence_safe: false,
    searched: [{ lane: "lexical", completeness: "best_effort", candidate_count: 12 }],
  });
});

test("query keeps one candidate list instead of the flattened duplicate", () => {
  const candidate = {
    reference: "chunk:1",
    path: "Source.md",
    content: "Evidence.",
    content_hash: "sha256:noise",
  };
  const compact = compactReasoningResponse("memory.query", {
    status: "complete",
    data: {
      results: [candidate],
      items: [{ id: "q0", status: "complete", results: [candidate] }],
    },
  });
  const data = compact.data as Record<string, unknown>;
  assert.equal(data.results, undefined);
  assert.equal((data.items as unknown[]).length, 1);
});

test("write receipts remain complete", () => {
  const body = { status: "committed", data: { receipt: "commit:1", items: [1, 2] } };
  assert.equal(compactReasoningResponse("memory.checkpoint", body), body);
});
