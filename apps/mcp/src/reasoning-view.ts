export type JsonObject = Record<string, unknown>;

const REASONING_OPERATIONS = new Set([
  "memory.open",
  "memory.query",
  "memory.read",
  "memory.compute",
  "memory.verify",
  "memory.checkpoint",
]);
const OPEN_TEXT_SOURCE_LIMIT = 12;
const OPEN_TEXT_TOTAL_CHARS = 32_000;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function compactReasoningResponse(operation: string, body: JsonObject): JsonObject {
  if (!REASONING_OPERATIONS.has(operation)) return body;

  const data = asObject(body.data) ?? {};
  const compact = pick(body, ["request_id", "session_id", "corpus_revision", "status"]);
  if (operation === "memory.open") {
    const freshness = compactFreshness(body.freshness);
    const coverage = compactCoverage(body.coverage);
    if (hasKeys(freshness)) compact.freshness = freshness;
    if (hasKeys(coverage)) compact.coverage = coverage;
  }
  for (const key of ["conflicts", "gaps", "ambiguities"] as const) {
    if (isPresent(body[key])) compact[key] = body[key];
  }
  const truncation = asObject(body.truncation);
  if (truncation?.truncated === true) compact.truncation = truncation;

  switch (operation) {
    case "memory.open":
      compact.data = compactOpenData(data);
      break;
    case "memory.query":
      compact.data = compactQueryData(data);
      break;
    case "memory.read":
      compact.data = compactReadData(data);
      break;
    case "memory.verify":
      compact.data = compactGenericData(data, Array.isArray(data.results) ? ["results"] : ["claims"]);
      break;
    case "memory.checkpoint":
      compact.data = compactCheckpointData(data);
      break;
    default:
      compact.data = compactGenericData(data, ["steps", "rows_returned", "estimated_tokens"]);
  }
  return compact;
}

function compactOpenData(data: JsonObject): JsonObject {
  const compact = compactGenericData(data, [
    "resume_checkpoint",
    "revision_delta",
    "initial_case_file",
  ]);
  if (Array.isArray(data.evidence)) {
    compact.initial_evidence = data.evidence
      .map(asObject)
      .filter((item): item is JsonObject => item !== undefined)
      .map((item) => ({
        ...pick(item, [
          "reference",
          "path",
          "title",
          "version",
          "content_hash",
          "heading",
          "why_selected",
          "superseded_by",
        ]),
        content: typeof item.text === "string" ? item.text : "",
        content_scope: item.representation === "complete_source"
          ? "complete_source"
          : "selected_source_sections",
      }));
    if (Array.isArray(data.evidence_leads) && data.evidence_leads.length > 0) {
      compact.evidence_leads = data.evidence_leads
        .map(asObject)
        .filter((item): item is JsonObject => item !== undefined)
        .map(compactSimpleCandidate);
    }
    for (const key of [
      "checkpoint",
      "changes_since_checkpoint",
      "resume_deltas",
      "changes_truncated",
      "next_changes_generation",
      "checkpoint_text_truncated",
    ] as const) {
      if (isPresent(data[key])) compact[key] = data[key];
    }
    if (isPresent(data.workspace_generation)) {
      compact.workspace_generation = data.workspace_generation;
    }
    const sufficiency = asObject(data.retrieval_sufficiency);
    if (sufficiency) compact.retrieval_sufficiency = sufficiency;
    if (Array.isArray(data.pending_intentions)) {
      compact.pending_intentions = data.pending_intentions;
    }
    return compact;
  }
  const resolvedScope = compactResolvedScope(data.resolved_scope);
  if (hasKeys(resolvedScope)) compact.resolved_scope = resolvedScope;
  const learnedContext = compactLearnedContext(data.learned_context);
  if (hasKeys(learnedContext)) compact.learned_context = learnedContext;

  const evidenceRefsBySource = collectEvidenceRefsBySource(data.initial_evidence);
  const hydrated = Array.isArray(data.hydrated_sources)
    ? data.hydrated_sources
      .map(asObject)
      .filter((item): item is JsonObject => item !== undefined)
    : [];
  const representedSources = new Set<string>();
  let textSourceCount = 0;
  let pointerSourceCount = 0;
  if (hydrated.length > 0) {
    const [textSources, leads] = partitionOpenSources(hydrated);
    textSourceCount = textSources.length;
    pointerSourceCount = leads.length;
    compact.initial_evidence = textSources.map((item) => compactHydratedSource(
      item,
      true,
      evidenceRefsForSource(item, evidenceRefsBySource),
    ));
    for (const item of hydrated) {
      representedSources.add(String(item.source_ref ?? item.path ?? ""));
    }
    if (leads.length > 0) {
      compact.evidence_leads = leads.map((item) => compactHydratedSource(
        item,
        false,
        evidenceRefsForSource(item, evidenceRefsBySource),
      ));
    }
  }

  if (Array.isArray(data.initial_evidence)) {
    const remaining = data.initial_evidence
      .map(asObject)
      .filter((item): item is JsonObject => item !== undefined)
      .filter((item) => !representedSources.has(String(item.source_ref ?? item.path ?? "")))
      .map(compactCandidate);
    if (remaining.length > 0) {
      compact.initial_evidence = [
        ...(Array.isArray(compact.initial_evidence) ? compact.initial_evidence : []),
        ...remaining,
      ];
    }
  }
  const sufficiency = asObject(data.retrieval_sufficiency);
  if (sufficiency) {
    compact.retrieval_sufficiency = {
      ...sufficiency,
      returned_text_sources: textSourceCount,
      pointer_only_sources: pointerSourceCount,
    };
  }
  const corpusMap = asObject(data.corpus_map);
  if (corpusMap) {
    compact.corpus_map = pick(corpusMap, [
      "record_counts",
      "profile_counts",
      "available_views",
      "truncated",
    ]);
  }
  return compact;
}

function partitionOpenSources(sources: JsonObject[]): [JsonObject[], JsonObject[]] {
  const textSources: JsonObject[] = [];
  const leads: JsonObject[] = [];
  let textChars = 0;
  for (const source of sources) {
    const sourceChars = typeof source.text === "string" ? source.text.length : 0;
    const withinCount = textSources.length < OPEN_TEXT_SOURCE_LIMIT;
    const withinBudget = textChars + sourceChars <= OPEN_TEXT_TOTAL_CHARS;
    if (withinCount && (withinBudget || textSources.length === 0)) {
      textSources.push(source);
      textChars += sourceChars;
    } else {
      leads.push(source);
    }
  }
  return [textSources, leads];
}

function compactQueryData(data: JsonObject): JsonObject {
  const compact = compactGenericData(data, []);
  if (
    Array.isArray(data.results)
    && data.results.some((item) => asObject(item)?.candidates !== undefined)
  ) {
    compact.items = data.results
      .map(asObject)
      .filter((item): item is JsonObject => item !== undefined)
      .map((item) => ({
        id: item.id,
        status: item.query_status,
        results: Array.isArray(item.candidates)
          ? item.candidates
            .map(asObject)
            .filter((candidate): candidate is JsonObject => candidate !== undefined)
            .map(compactSimpleCandidate)
          : [],
        ...(isPresent(item.lane_failures) ? { gaps: item.lane_failures } : {}),
      }));
    if (isPresent(data.workspace_generation)) {
      compact.workspace_generation = data.workspace_generation;
    }
    return compact;
  }
  if (Array.isArray(data.items)) {
    compact.items = data.items
      .map(asObject)
      .filter((item): item is JsonObject => item !== undefined)
      .map(compactQueryItem);
  } else if (Array.isArray(data.results)) {
    compact.results = data.results
      .map(asObject)
      .filter((item): item is JsonObject => item !== undefined)
      .map(compactCandidate);
  }
  return compact;
}

function compactQueryItem(item: JsonObject): JsonObject {
  const compact = pick(item, ["id", "status"]);
  if (Array.isArray(item.results)) {
    compact.results = item.results
      .map(asObject)
      .filter((candidate): candidate is JsonObject => candidate !== undefined)
      .map(compactCandidate);
  }
  for (const key of ["conflicts", "gaps", "ambiguities"] as const) {
    if (isPresent(item[key])) compact[key] = item[key];
  }
  const coverage = compactCoverage(item.coverage);
  if (hasKeys(coverage) && (item.status !== "complete" || isPresent(coverage.unsearched))) {
    compact.coverage = coverage;
  }
  return compact;
}

function compactReadData(data: JsonObject): JsonObject {
  const compact = compactGenericData(data, []);
  if (Array.isArray(data.items)) {
    compact.items = data.items
      .map(asObject)
      .filter((item): item is JsonObject => item !== undefined)
      .map(compactReadItem);
  }
  return compact;
}

function compactReadItem(item: JsonObject): JsonObject {
  if (typeof item.text === "string") {
    return pick(item, [
      "reference",
      "path",
      "title",
      "version",
      "version_ref",
      "content_hash",
      "media_type",
      "view",
      "status",
      "text",
      "metadata",
      "supersession_chain",
      "supersession_warning",
      "current_truth_notice",
    ]);
  }
  const compact = pick(item, ["reference", "view", "status"]);
  const error = asObject(item.error);
  if (error && hasKeys(error)) compact.error = error;
  const data = asObject(item.data);
  if (data && typeof data.text === "string") {
    const metadata = asObject(data.metadata) ?? {};
    const locator = asObject(metadata.native_locator) ?? {};
    const source: JsonObject = {};
    const sourceValues: Record<string, unknown> = {
      reference: metadata.ref,
      path: locator.path ?? metadata.source_ref,
      title: metadata.title,
      version: metadata.source_version ?? metadata.version,
      media_type: metadata.media_type,
      representation: metadata.representation,
    };
    for (const [key, value] of Object.entries(sourceValues)) {
      if (isPresent(value)) source[key] = value;
    }
    if (hasKeys(source)) compact.source = source;
    const range = asObject(data.range);
    if (range) compact.lines = [range.start_line, range.end_line];
    compact.text = data.text;
  } else if (data && Array.isArray(data.chunks)) {
    if (isPresent(data.anchor_ref)) compact.anchor_reference = data.anchor_ref;
    compact.chunks = data.chunks
      .map(asObject)
      .filter((chunk): chunk is JsonObject => chunk !== undefined)
      .map(compactNeighborChunk);
  } else if (data && hasKeys(data)) {
    compact.data = data;
  }
  const truncation = asObject(item.truncation);
  if (truncation?.truncated === true) {
    compact.truncation = pick(truncation, ["truncated", "continuation_token"]);
  }
  return compact;
}

function compactNeighborChunk(chunk: JsonObject): JsonObject {
  const compact = pick(chunk, ["ref", "ordinal", "heading_path", "content", "is_anchor"]);
  const locator = asObject(chunk.locator);
  if (locator) compact.lines = [locator.start_line, locator.end_line];
  return compact;
}

function compactCheckpointData(data: JsonObject): JsonObject {
  const compact = pick(data, [
    "checkpoint_id",
    "id",
    "base_corpus_revision",
    "resulting_corpus_revision",
    "receipt",
    "replayed",
    "review_required",
    "search_status",
    "indexing",
    "checkpoint_ref",
    "path",
    "workspace_generation",
    "source_entries",
    "write",
  ]);
  let parentCheckpoint: unknown;
  const sourceRefs: string[] = [];
  if (Array.isArray(data.items)) {
    for (const rawItem of data.items) {
      const item = asObject(rawItem);
      const details = item ? asObject(item.details) : undefined;
      if (!details) continue;
      parentCheckpoint ??= details.parent_checkpoint_id;
      if (!Array.isArray(details.source_refs)) continue;
      for (const sourceRef of details.source_refs) {
        if (typeof sourceRef === "string" && !sourceRefs.includes(sourceRef)) {
          sourceRefs.push(sourceRef);
        }
      }
    }
  }
  if (isPresent(parentCheckpoint)) compact.parent_checkpoint_id = parentCheckpoint;
  if (sourceRefs.length > 0) compact.source_refs = sourceRefs;
  return compact;
}

function compactCandidate(candidate: JsonObject): JsonObject {
  const compact = pick(candidate, [
    "reference",
    "path",
    "heading",
    "content",
    "authority",
    "canonicality",
    "recorded_at",
    "valid_time",
  ]);
  if (isPresent(candidate.source_ref) && candidate.source_ref !== candidate.path) {
    compact.source_ref = candidate.source_ref;
  }
  const evidenceRefs = uniqueStrings(candidate.evidence_refs);
  if (evidenceRefs.length > 0) compact.evidence_refs = evidenceRefs;
  return compact;
}

function compactSimpleCandidate(candidate: JsonObject): JsonObject {
  const compact = pick(candidate, [
    "path",
    "title",
    "version",
    "heading",
    "representation",
    "excerpt",
    "text",
    "additional_sections",
    "verbatim_matches",
    "superseded_by",
    "annotation",
    "delta_omitted_reason",
  ]);
  const reference = typeof candidate.reference === "string"
    ? candidate.reference
    : typeof candidate.entry_ref === "string"
      ? candidate.entry_ref
      : typeof candidate.entry_id === "string" && UUID.test(candidate.entry_id)
        ? `entry:${candidate.entry_id}`
        : undefined;
  if (reference !== undefined) compact.reference = reference;
  const contentHash = candidate.content_hash ?? (
    typeof candidate.content_sha256 === "string"
      ? candidate.content_sha256.startsWith("sha256:")
        ? candidate.content_sha256
        : `sha256:${candidate.content_sha256}`
      : undefined
  );
  if (isPresent(contentHash)) compact.content_hash = contentHash;
  const lanes = uniqueStrings(candidate.lanes);
  if (lanes.length > 0) compact.lanes = lanes;
  if (
    candidate.representation === "pointer_lead"
    && typeof candidate.score === "number"
  ) {
    compact.score = candidate.score;
  }
  return compact;
}

function compactHydratedSource(
  source: JsonObject,
  includeText: boolean,
  evidenceRefs: string[] = [],
): JsonObject {
  const compact = pick(source, [
    "path",
    "title",
    "authority",
    "canonicality",
    "recorded_at",
  ]);
  if (isPresent(source.source_ref) && source.source_ref !== source.path) {
    compact.source_ref = source.source_ref;
  }
  if (evidenceRefs.length > 0) compact.evidence_refs = evidenceRefs;
  if (
    source.complete !== true
    && Array.isArray(source.selected_references)
    && source.selected_references.length > 0
  ) {
    compact.reference = source.selected_references[0];
  }
  if (source.complete !== true && Array.isArray(source.ranges) && source.ranges.length > 0) {
    compact.ranges = source.ranges
      .map(asObject)
      .filter((range): range is JsonObject => range !== undefined)
      .map((range) => [range.start_line, range.end_line]);
  }
  if (includeText) {
    compact.content = typeof source.text === "string" ? source.text : "";
    compact.content_scope = source.complete === true
      ? "complete_source"
      : "selected_source_sections";
  } else {
    compact.content_scope = "source_lead";
  }
  return compact;
}

function collectEvidenceRefsBySource(value: unknown): Map<string, string[]> {
  const bySource = new Map<string, string[]>();
  if (!Array.isArray(value)) return bySource;
  for (const rawCandidate of value) {
    const candidate = asObject(rawCandidate);
    if (!candidate) continue;
    const refs = uniqueStrings(candidate.evidence_refs);
    for (const key of [candidate.source_ref, candidate.path]) {
      if (typeof key !== "string" || key.length === 0) continue;
      const existing = bySource.get(key) ?? [];
      for (const ref of refs) {
        if (!existing.includes(ref)) existing.push(ref);
      }
      bySource.set(key, existing);
    }
  }
  return bySource;
}

function evidenceRefsForSource(
  source: JsonObject,
  bySource: Map<string, string[]>,
): string[] {
  const refs: string[] = [];
  for (const key of [source.source_ref, source.path]) {
    if (typeof key !== "string") continue;
    for (const ref of bySource.get(key) ?? []) {
      if (!refs.includes(ref)) refs.push(ref);
    }
  }
  return refs;
}

function uniqueStrings(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return [...new Set(value.filter(
    (item): item is string => typeof item === "string" && item.length > 0,
  ))];
}

function compactResolvedScope(value: unknown): JsonObject {
  const scope = asObject(value);
  return scope
    ? pick(scope, ["authorization_scope", "root_objects", "requested_lenses", "ambiguities"])
    : {};
}

function compactLearnedContext(value: unknown): JsonObject {
  const learned = asObject(value);
  if (!learned) return {};
  if (!Array.isArray(learned.items) || learned.items.length === 0) {
    return pick(learned, ["status", "reason"]);
  }
  return pick(learned, [
    "status",
    "authority",
    "pinned_corpus_revision",
    "items",
    "latest_candidate",
  ]);
}

function compactGenericData(data: JsonObject, keys: string[]): JsonObject {
  const compact = pick(data, keys);
  const projection = compactProjection(data.projection);
  if (hasKeys(projection)) compact.projection = projection;
  return compact;
}

function compactProjection(value: unknown): JsonObject {
  const projection = asObject(value);
  return projection
    ? pick(projection, [
      "policy_ref",
      "policy_version",
      "audience",
      "purpose",
      "audit_receipt",
      "withheld",
      "transforms",
    ])
    : {};
}

function compactFreshness(value: unknown): JsonObject {
  const freshness = asObject(value);
  return freshness
    ? pick(freshness, ["source_updated_at", "lexical_index_updated_at", "semantic_index_updated_at"])
    : {};
}

function compactCoverage(value: unknown): JsonObject {
  const coverage = asObject(value);
  if (!coverage) return {};
  const compact: JsonObject = { absence_safe: coverage.absence_safe === true };
  for (const key of ["searched", "unsearched"] as const) {
    if (!Array.isArray(coverage[key]) || coverage[key].length === 0) continue;
    const rows = coverage[key]
      .map(asObject)
      .filter((row): row is JsonObject => row !== undefined)
      .filter((row) => {
        const candidateCount = typeof row.candidate_count === "number"
          ? row.candidate_count
          : 0;
        return candidateCount > 0
          || isPresent(row.failure_reason)
          || ["degraded", "failed", "partial", "unsearched"].includes(String(row.completeness));
      })
      .map((row) => pick(row, ["lane", "completeness", "candidate_count", "failure_reason"]));
    if (rows.length > 0) compact[key] = rows;
  }
  return compact;
}

function pick(source: JsonObject, keys: readonly string[]): JsonObject {
  const selected: JsonObject = {};
  for (const key of keys) {
    if (isPresent(source[key])) selected[key] = source[key];
  }
  return selected;
}

function asObject(value: unknown): JsonObject | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as JsonObject
    : undefined;
}

function isPresent(value: unknown): boolean {
  if (value === null || value === undefined) return false;
  if (Array.isArray(value)) return value.length > 0;
  if (typeof value === "object") return Object.keys(value).length > 0;
  return true;
}

function hasKeys(value: JsonObject): boolean {
  return Object.keys(value).length > 0;
}
