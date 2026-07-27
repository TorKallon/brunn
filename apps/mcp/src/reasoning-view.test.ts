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
        evidence_refs: ["evidence:1"],
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
  assert.deepEqual(evidence.evidence_refs, ["evidence:1"]);
  assert.deepEqual(compact.coverage, {
    absence_safe: false,
    searched: [{ lane: "lexical", completeness: "best_effort", candidate_count: 12 }],
  });
});

test("query keeps one candidate list instead of the flattened duplicate", () => {
  const candidate = {
    reference: "chunk:1",
    source_ref: "Source.md",
    path: "Source.md",
    content: "Evidence.",
    evidence_refs: ["evidence:1"],
    content_hash: "sha256:noise",
    source_version: "sha256:source-version",
    why_selected: ["semantic_recall"],
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
  const item = (data.items as Array<Record<string, unknown>>)[0];
  const compactCandidate = (item?.results as Array<Record<string, unknown>>)[0];
  assert.equal(compactCandidate?.reference, "chunk:1");
  assert.deepEqual(compactCandidate?.evidence_refs, ["evidence:1"]);
  assert.equal(compactCandidate?.source_ref, undefined);
  assert.equal(compactCandidate?.source_version, undefined);
  assert.equal(compactCandidate?.why_selected, undefined);
});

test("simple query keeps exact entry identity and retrieval lanes without score metadata", () => {
  const compact = compactReasoningResponse("memory.query", {
    status: "complete",
    data: {
      workspace_generation: 42,
      results: [{
        id: "q0",
        query_status: "complete",
        candidates: [{
          entry_id: "019f8530-e5f6-77d3-a373-052ee8cd24bd",
          path: "Trips/Switzerland/Rail Plan.md",
          title: "Rail Plan",
          version: 3,
          content_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
          heading: "Reservation decisions",
          excerpt: "Book the constrained segment first.",
          additional_sections: [{
            heading: "Fallback",
            excerpt: "Keep the flexible train as a backup.",
          }],
          score: 9.81,
          lanes: ["exact", "lexical", "lexical"],
        }],
      }],
    },
  });

  const data = compact.data as Record<string, unknown>;
  const item = (data.items as Array<Record<string, unknown>>)[0];
  const candidate = (item?.results as Array<Record<string, unknown>>)[0];
  assert.deepEqual(candidate, {
    reference: "entry:019f8530-e5f6-77d3-a373-052ee8cd24bd",
    path: "Trips/Switzerland/Rail Plan.md",
    title: "Rail Plan",
    version: 3,
    content_hash: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    heading: "Reservation decisions",
    excerpt: "Book the constrained segment first.",
    additional_sections: [{
      heading: "Fallback",
      excerpt: "Keep the flexible train as a backup.",
    }],
    lanes: ["exact", "lexical"],
  });
  assert.equal(JSON.stringify(candidate).includes("score"), false);
  assert.equal(JSON.stringify(candidate).includes("entry_id"), false);
});

test("open returns complete source text with pointer-only tail leads", () => {
  const compact = compactReasoningResponse("memory.open", {
    status: "complete",
    data: {
      hydrated_sources: Array.from({ length: 13 }, (_, index) => ({
        source_ref: `Source-${index}.md`,
        path: `Source-${index}.md`,
        text: `Evidence ${index}`,
        complete: index < 4,
        ranges: [{ start_line: 1, end_line: 2 }],
        selected_references: [`chunk:${index}`],
      })),
      initial_evidence: Array.from({ length: 13 }, (_, index) => ({
        source_ref: `Source-${index}.md`,
        path: `Source-${index}.md`,
        evidence_refs: [`evidence:${index}`],
      })),
      retrieval_sufficiency: {
        status: "likely_sufficient",
        complete_source_count: 4,
      },
    },
  });
  const data = compact.data as Record<string, unknown>;
  const evidence = data.initial_evidence as Array<Record<string, unknown>>;
  const leads = data.evidence_leads as Array<Record<string, unknown>>;
  assert.equal(evidence.length, 12);
  assert.equal(evidence[0]?.content_scope, "complete_source");
  assert.equal(evidence[0]?.content, "Evidence 0");
  assert.equal(evidence[0]?.path, "Source-0.md");
  assert.deepEqual(evidence[0]?.evidence_refs, ["evidence:0"]);
  assert.equal(evidence[0]?.source_ref, undefined);
  assert.equal(evidence[0]?.reference, undefined);
  assert.equal(evidence[0]?.ranges, undefined);
  assert.equal(evidence[0]?.complete, undefined);
  assert.equal(leads.length, 1);
  assert.equal(leads[0]?.content_scope, "source_lead");
  assert.deepEqual(leads[0]?.evidence_refs, ["evidence:12"]);
  assert.equal(leads[0]?.content, undefined);
});

test("simple open preserves pointer leads and continuation pagination", () => {
  const compact = compactReasoningResponse("memory.open", {
    status: "partial",
    data: {
      evidence: [{
        reference: "entry:019f8530-e5f6-77d3-a373-052ee8cd24bd",
        path: "Work/Current.md",
        title: "Current",
        version: 4,
        representation: "complete_source",
        text: "Current source.",
      }],
      evidence_leads: [{
        reference: "entry:019f8530-e5f6-77d3-a373-052ee8cd24be",
        path: "Work/Related.md",
        title: "Related",
        version: 2,
      }],
      changes_since_checkpoint: [{ generation: 201, path: "Work/Current.md" }],
      changes_truncated: true,
      next_changes_generation: 201,
      checkpoint_text_truncated: false,
    },
  });

  const data = compact.data as Record<string, unknown>;
  assert.deepEqual(data.evidence_leads, [{
    reference: "entry:019f8530-e5f6-77d3-a373-052ee8cd24be",
    path: "Work/Related.md",
    title: "Related",
    version: 2,
  }]);
  assert.equal(data.changes_truncated, true);
  assert.equal(data.next_changes_generation, 201);
  assert.equal(data.checkpoint_text_truncated, false);
});

test("open caps returned evidence text before adding pointer leads", () => {
  const compact = compactReasoningResponse("memory.open", {
    status: "complete",
    data: {
      hydrated_sources: [
        { source_ref: "Large-0.md", text: "a".repeat(20_000), complete: true },
        { source_ref: "Large-1.md", text: "b".repeat(20_000), complete: true },
      ],
      retrieval_sufficiency: { status: "likely_sufficient" },
    },
  });
  const data = compact.data as Record<string, unknown>;
  assert.equal((data.initial_evidence as unknown[]).length, 1);
  assert.equal((data.evidence_leads as unknown[]).length, 1);
  const sufficiency = data.retrieval_sufficiency as Record<string, unknown>;
  assert.equal(sufficiency.pointer_only_sources, 1);
});

test("read keeps exact source text and continuation without audit-only duplication", () => {
  const compact = compactReasoningResponse("memory.read", {
    status: "partial",
    data: {
      items: [{
        reference: "Source.md",
        view: "range",
        status: "partial",
        data: {
          metadata: {
            ref: "document:1",
            source_ref: "Source.md",
            source_version: "v2",
            content_hash: "sha256:noise",
            native_locator: { path: "Source.md" },
          },
          range: {
            start_byte: 0,
            end_byte_exclusive: 12,
            start_line: 1,
            end_line: 2,
          },
          text: "Exact text.",
          instruction_boundary: "evidence",
        },
        error: null,
        truncation: {
          truncated: true,
          returned_tokens: 3,
          limit_tokens: 3,
          continuation_token: "opaque",
        },
      }],
    },
  });
  const item = ((compact.data as Record<string, unknown>).items as Array<Record<string, unknown>>)[0];
  const source = item?.source as Record<string, unknown>;
  assert.equal(item?.text, "Exact text.");
  assert.equal(source.reference, "document:1");
  assert.equal(source.path, "Source.md");
  assert.deepEqual(item?.lines, [1, 2]);
  assert.deepEqual(item?.truncation, {
    truncated: true,
    continuation_token: "opaque",
  });
  assert.equal(JSON.stringify(item).includes("content_hash"), false);
  assert.equal(JSON.stringify(item).includes("start_byte"), false);
});

test("checkpoint receipts keep lineage but drop duplicated write machinery", () => {
  const compact = compactReasoningResponse("memory.checkpoint", {
    session_id: "session:1",
    corpus_revision: "revision:2",
    status: "committed",
    data: {
      checkpoint_id: "checkpoint:2",
      base_corpus_revision: "revision:1",
      resulting_corpus_revision: "revision:2",
      receipt: "commit:1",
      request_hash: "sha256:noise",
      credential_id: "credential:noise",
      items: [{
        details: {
          parent_checkpoint_id: "checkpoint:1",
          source_refs: ["source:1"],
        },
      }],
    },
  });
  const data = compact.data as Record<string, unknown>;
  assert.equal(data.checkpoint_id, "checkpoint:2");
  assert.equal(data.parent_checkpoint_id, "checkpoint:1");
  assert.deepEqual(data.source_refs, ["source:1"]);
  assert.equal(data.request_hash, undefined);
  assert.equal(data.credential_id, undefined);
});

test("non-checkpoint write receipts remain complete", () => {
  const body = { status: "committed", data: { receipt: "commit:1", items: [1, 2] } };
  assert.equal(compactReasoningResponse("memory.save", body), body);
});
