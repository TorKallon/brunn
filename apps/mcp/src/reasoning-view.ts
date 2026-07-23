export type JsonObject = Record<string, unknown>;

const REASONING_OPERATIONS = new Set([
  "memory.open",
  "memory.query",
  "memory.read",
  "memory.compute",
  "memory.verify",
]);

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
      compact.data = compactGenericData(data, ["items"]);
      break;
    case "memory.verify":
      compact.data = compactGenericData(data, Array.isArray(data.results) ? ["results"] : ["claims"]);
      break;
    default:
      compact.data = compactGenericData(data, ["steps", "rows_returned", "estimated_tokens"]);
  }
  return compact;
}

function compactOpenData(data: JsonObject): JsonObject {
  const compact = compactGenericData(data, [
    "resolved_scope",
    "resume_checkpoint",
    "revision_delta",
    "learned_context",
    "initial_case_file",
  ]);
  if (Array.isArray(data.initial_evidence)) {
    compact.initial_evidence = data.initial_evidence
      .map(asObject)
      .filter((item): item is JsonObject => item !== undefined)
      .map(compactCandidate);
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

function compactQueryData(data: JsonObject): JsonObject {
  const compact = compactGenericData(data, []);
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

function compactCandidate(candidate: JsonObject): JsonObject {
  return pick(candidate, [
    "reference",
    "source_ref",
    "path",
    "heading",
    "content",
    "source_version",
    "authority",
    "canonicality",
    "recorded_at",
    "valid_time",
    "why_selected",
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
      "output_hash",
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
    compact[key] = coverage[key]
      .map(asObject)
      .filter((row): row is JsonObject => row !== undefined)
      .map((row) => pick(row, ["lane", "completeness", "candidate_count", "failure_reason"]));
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
