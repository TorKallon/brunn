#!/usr/bin/env node

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { appendFile } from "node:fs/promises";
import { z } from "zod/v4";

import { type ApiResponse, StraylightApiClient, StraylightApiError } from "./api-client.js";
import { compactReasoningResponse } from "./reasoning-view.js";

const includeStructuredContent =
  process.env.STRAYLIGHT_MCP_INCLUDE_STRUCTURED_CONTENT === "1";

const reference = z.string().min(1);
const assetReference = z.string()
  .regex(/^asset:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i)
  .describe("Exact asset:... reference copied from a CarryState response.");
const jsonObject = z.record(z.string(), z.unknown());
const structuredQueryFilter = z.object({
  scope_root: z.string().min(1).optional().describe(
    "Optional graph root record reference, such as object:..., claim:..., or source_episode:.... " +
    "This is not the authorization scope returned by memory.open; omit it unless narrowing results " +
    "to a known record and its graph neighborhood.",
  ),
  type_profile: z.string().min(1).optional(),
  predicate: z.string().min(1).optional(),
  record_kind: z.string().min(1).optional(),
  authority: z.string().min(1).optional(),
  canonicality: z.string().min(1).optional(),
}).strict();
const stateFilter = z.object({
  machine_ref: reference,
  states: z.array(z.string().min(1)).optional(),
  valid_at: z.literal("latest").optional(),
}).strict();
const queryExpansion = z.object({
  parents: z.boolean().optional(),
  neighbors: z.number().int().min(0).optional(),
  relations: z.array(z.string().min(1)).optional(),
}).strict();
const computeOperator = z.enum([
  "catalog",
  "query",
  "search",
  "read",
  "batchRead",
  "neighbors",
  "timeline",
  "history",
  "diff",
  "group",
  "aggregate",
  "traverse",
  "resolveIdentity",
  "expandRecurrence",
  "stateHistory",
  "impact",
  "compareApplicability",
  "proximity",
  "gateRollup",
]);
const checkpointSourceReference = reference.describe(
  "A source-bearing record ID: use evidence:... from evidence_refs or " +
  "source_episode:... from memory.read source.reference. Do not pass a file path here.",
);

const queryItem = z.object({
  id: z.string().optional(),
  goal: z.string().optional(),
  query: z.string().min(1),
  scope: z.string().optional().describe(
    "Optional canonical scope ID returned by memory.open. Omit this field rather than using a human display label.",
  ),
  modes: z.array(
    z.enum(["exact", "structured", "lexical", "semantic", "temporal", "relations"]),
  ).optional().describe(
    "Use exact only when query is an exact record reference. Read a known file path with " +
    "memory.read; use lexical or semantic modes for title or topic text.",
  ),
  where: structuredQueryFilter.optional(),
  state_filter: stateFilter.optional(),
  expand: queryExpansion.optional(),
  limit: z.number().int().min(1).max(100).default(8),
});

const readItem = z.object({
  ref: reference.optional().describe(
    "Exact record reference copied verbatim from a CarryState response. Never infer or invent a reference.",
  ),
  path: z.string().min(1).optional().describe(
    "Exact source path copied verbatim from a CarryState response. Never synthesize a filename from a title or topic.",
  ),
  view: z.enum([
    "current_state",
    "structured",
    "outline",
    "full",
    "range",
    "neighbors",
    "relationships",
    "history",
    "diff",
    "last_known_good",
    "materialize_scope",
  ]).optional(),
  start: z.number().int().min(1).optional(),
  end: z.number().int().min(1).optional(),
  before: z.number().int().min(0).max(20).optional(),
  after: z.number().int().min(0).max(20).optional(),
  max_chars: z.number().int().min(1).max(500_000).optional(),
}).refine((value) => value.ref !== undefined || value.path !== undefined, {
  message: "read request requires ref or path",
});

const client = new StraylightApiClient(
  process.env.STRAYLIGHT_API_URL ?? "http://api:18110",
  requiredEnvironment("STRAYLIGHT_API_TOKEN"),
  fetch,
  evaluationHeaders(),
);

const server = new McpServer({
  name: "straylight",
  version: "0.1.0",
});

registerJsonTool(
  "memory.open",
  "Open or resume a corpus-revision-pinned reasoning session and receive a bounded context map.",
  {
    task: z.string().min(1),
    hints: z.object({
      authorization_scope: z.string().optional(),
      root_refs: z.array(reference).optional(),
      open_object_refs: z.array(reference).optional(),
    }).optional(),
    as_of: z.string().optional(),
    mode: z.enum(["continuation", "exploration"]).optional(),
    resume_checkpoint_ref: reference.optional().describe(
      "Exact checkpoint:... reference supplied by the caller. Omit this field when no exact " +
      "checkpoint reference was supplied; never invent one or use placeholders such as latest.",
    ),
    token_budget: z.number().int().min(1).optional(),
  },
  (input) => client.request("/v1/memory/open", input),
);

registerJsonTool(
  "memory.query",
  "Run one or more exact, structured, lexical, or semantic queries against a pinned session. " +
  "Exact mode accepts record references, not file paths or titles. The query `scope` field accepts " +
  "the canonical authorization scope from memory.open. `where.scope_root` instead accepts a known " +
  "graph record reference; never copy the authorization scope into it.",
  {
    session_id: reference,
    queries: z.array(queryItem).min(1).max(32),
  },
  (input) => client.request("/v1/memory/query", input),
);

registerJsonTool(
  "memory.read",
  "Batch exact source, evidence, object, relation, claim, checkpoint, or revision reads. " +
  "Copy each ref or path verbatim from a prior CarryState response; never infer a filename from a title or topic. " +
  "Use range or neighbors when a candidate excerpt is incomplete.",
  {
    session_id: reference,
    requests: z.array(readItem).min(1).max(32),
  },
  (input) => client.request("/v1/memory/read", input),
);

registerJsonTool(
  "memory.compute",
  "Run supported bounded graph, temporal, spatial, aggregation, or acceptance-gate operations. " +
  "This is not a general arithmetic calculator; do ordinary arithmetic directly.",
  {
    session_id: reference,
    steps: z.array(z.object({
      id: z.string().min(1),
      op: computeOperator,
      input: jsonObject,
    })).min(1).max(32),
    max_rows: z.number().int().min(1).max(10_000).optional(),
    token_budget: z.number().int().min(1).optional(),
  },
  (input) => client.request("/v1/memory/compute", input),
);

registerJsonTool(
  "memory.verify",
  "Classify support, contradiction, supersession, and temporal applicability when those " +
  "questions remain unresolved. Do not re-verify an already complete authoritative source.",
  {
    session_id: reference,
    claims: z.array(z.object({
      id: z.string().min(1),
      claim: z.string().min(1),
      evidence_refs: z.array(reference).optional(),
      about_ref: reference.optional(),
      predicate: z.string().optional(),
      value: z.unknown().optional(),
      coverage_ref: reference.optional(),
    })).min(1).max(32),
    check_for: z.array(z.enum([
      "newer_evidence",
      "contradictions",
      "superseded_sources",
      "unsupported_claims",
      "identity_ambiguity",
      "named_state_mismatch",
      "recurrence_or_occurrence_loss",
      "incomplete_collection_coverage",
      "temporal_ambiguity",
    ])).optional(),
  },
  (input) => client.request("/v1/memory/verify", input),
);

registerJsonTool(
  "memory.capture",
  "Turn ordinary source content into source-linked durable records, committing low-risk captures and returning a draft when consequential details are ambiguous.",
  {
    content: z.string().min(1).max(256_000),
    source: z.object({
      ref: reference.optional(),
      external_ref: z.string().min(1).max(2_000).optional(),
      title: z.string().min(1).max(500).optional(),
      kind: z.string().min(1).max(120).optional(),
      origin: z.enum(["user", "external", "agent", "tool", "system"]).optional(),
      media_type: z.string().optional(),
      locator: jsonObject.optional(),
      metadata: jsonObject.optional(),
      content_hash: z.string().optional(),
    }).refine((value) => value.ref !== undefined || value.title !== undefined, {
      message: "capture source requires ref or title",
    }),
    scope: z.string().min(1),
    root_refs: z.array(reference).max(64).optional(),
    intent: z.string().min(1).optional(),
    idempotency_key: z.string().min(1).max(240).optional(),
    base_corpus_revision: reference.optional(),
    mode: z.enum(["auto", "draft"]).optional(),
  },
  (input) => client.request("/v1/memory/capture", input),
);

registerJsonTool(
  "memory.save",
  "Atomically save source-bearing objects, claims, relations, states, corrections, policies, or checkpoints.",
  {
    intent: z.string().min(1),
    scope: z.string().min(1),
    root_refs: z.array(reference).optional(),
    source_refs: z.array(z.object({
      ref: reference,
      span: z.tuple([z.number().int().min(0), z.number().int().min(0)]).optional(),
      content_hash: z.string().optional(),
    })).optional(),
    base_corpus_revision: reference.optional(),
    idempotency_key: z.string().min(1),
    operation_id: z.string().optional(),
    confirmation_token: z.string().optional(),
    items: z.array(jsonObject).min(1).max(128),
  },
  (input) => client.request("/v1/memory/save", input),
);

registerJsonTool(
  "memory.checkpoint",
  "Commit one immutable child checkpoint linked to its session, corpus revision, parent, " +
  "state, gates, and evidence. Use evidence_refs returned by open/query or the " +
  "source_episode reference returned by read as source_refs.",
  {
    session_id: reference,
    parent_checkpoint_id: reference.optional(),
    idempotency_key: z.string().min(1),
    state: z.object({
      objective: z.string().min(1),
      current_state: z.union([z.string(), z.array(z.string())]).optional(),
      decisions: z.array(z.string()).optional(),
      open_questions: z.array(z.string()).optional(),
      next_actions: z.array(z.string()).optional(),
      artifacts: z.array(z.string()).optional(),
      ordered_goals: z.array(z.union([z.string(), jsonObject])).optional(),
      state_refs: z.array(reference.describe(
        "A durable state or object record ID. Do not put file paths, chunk IDs, or evidence IDs here.",
      )).optional(),
      acceptance_gates: z.array(z.union([z.string(), jsonObject])).optional(),
    }),
    source_refs: z.array(checkpointSourceReference).optional(),
  },
  (input) => client.request("/v1/memory/checkpoint", input),
);

registerJsonTool(
  "memory.stage",
  "Stage files from the adapter's sandboxed import root without placing binary content in model context.",
  {
    scope: z.string().min(1),
    stable_import_id: z.string().min(1).optional(),
    describe_binaries: z.boolean().default(true).describe(
      "Generate searchable, explicitly non-authoritative descriptions for native files.",
    ),
    files: z.array(z.object({
      path: z.string().min(1).describe(
        "Path below STRAYLIGHT_MCP_IMPORT_ROOT; it is retained as the logical vault path unless name is supplied.",
      ),
      name: z.string().min(1).optional().describe(
        "Optional logical vault path override. This is not merely a basename.",
      ),
      media_type: z.string().optional(),
    })).min(1).max(32),
  },
  (input) => client.stage(
    input.scope,
    input.stable_import_id,
    input.files,
    input.describe_binaries,
  ),
);

registerJsonTool(
  "memory.status",
  "Inspect the credential's current corpus and embedding service status.",
  {},
  () => client.request("/v1/status"),
);

registerJsonTool(
  "asset.list",
  "List native assets visible in one session-pinned corpus revision. Returns paths, exact asset references, versions, hashes, sizes, media types, description status, and usage metadata without returning binary bytes.",
  {
    session_id: reference.describe(
      "Exact session:... reference returned by memory.open.",
    ),
    offset: z.number().int().nonnegative().default(0),
    limit: z.number().int().min(1).max(500).default(100),
  },
  (input) => client.listAssets(input.session_id, input.offset, input.limit),
);

registerJsonTool(
  "asset.metadata",
  "Read session-pinned metadata for one exact CarryState asset reference without downloading its bytes.",
  {
    session_id: reference.describe(
      "Exact session:... reference returned by memory.open. Asset access is pinned to this session.",
    ),
    asset_ref: assetReference,
  },
  (input) => client.assetMetadata(input.asset_ref, input.session_id),
);

registerJsonTool(
  "asset.fetch",
  "Download one session-pinned CarryState asset into the MCP adapter's private asset root. " +
  "The tool verifies the streamed size and SHA-256 and returns only a local path plus integrity metadata; " +
  "asset bytes and base64 are never returned to model context.",
  {
    session_id: reference.describe(
      "Exact session:... reference returned by memory.open. Asset access is pinned to this session.",
    ),
    asset_ref: assetReference,
    version: z.number().int().positive().optional().describe(
      "Optional version copied from asset.metadata. When supplied, it must match the session-pinned version.",
    ),
  },
  (input) => client.fetchAsset(input.asset_ref, input.session_id, input.version),
);

await server.connect(new StdioServerTransport());

function registerJsonTool<Shape extends z.ZodRawShape>(
  name: string,
  description: string,
  inputSchema: Shape,
  invoke: (input: z.infer<z.ZodObject<Shape>>) => Promise<ApiResponse>,
): void {
  const callback = async (input: z.infer<z.ZodObject<Shape>>) => {
    try {
      const response = await invoke(input);
      const body = compactReasoningResponse(name, response.body);
      await traceOperation(name, response.status, response.elapsedMs, response.body, body);
      return {
        content: [{ type: "text" as const, text: JSON.stringify(body) }],
        ...(includeStructuredContent ? { structuredContent: body } : {}),
      };
    } catch (error) {
      const body = error instanceof StraylightApiError
        ? error.body
        : { error: { code: "adapter_error", message: errorMessage(error) } };
      await traceOperation(
        name,
        error instanceof StraylightApiError ? error.status : 0,
        0,
        body,
        body,
      );
      return {
        isError: true,
        content: [{ type: "text" as const, text: JSON.stringify(body) }],
        ...(includeStructuredContent ? { structuredContent: body } : {}),
      };
    }
  };
  // McpServer validates the raw shape before calling this function. Its generic
  // callback type does not preserve a reusable helper's Zod shape inference.
  server.registerTool(name, { description, inputSchema }, callback as never);
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) {
    process.stderr.write(`${name} is required\n`);
    process.exit(78);
  }
  return value;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function evaluationHeaders(): Record<string, string> {
  const headers: Record<string, string> = {};
  if (process.env.STRAYLIGHT_EVAL_RUN) {
    headers["x-straylight-eval-run"] = process.env.STRAYLIGHT_EVAL_RUN;
  }
  if (process.env.STRAYLIGHT_EVAL_CASE) {
    headers["x-straylight-eval-case"] = process.env.STRAYLIGHT_EVAL_CASE;
  }
  return headers;
}

async function traceOperation(
  operation: string,
  httpStatus: number,
  elapsedMs: number,
  response: Record<string, unknown>,
  rendered: Record<string, unknown>,
): Promise<void> {
  const tracePath = process.env.STRAYLIGHT_MCP_TRACE_PATH;
  if (!tracePath) {
    return;
  }
  const renderedText = JSON.stringify(rendered);
  const sourceTextChars = countSourceTextChars(rendered);
  const binaryBytes = operation === "asset.fetch"
    ? Number(findField(rendered, ["size_bytes"]) ?? 0)
    : 0;
  const sourcePaths = collectSourcePaths(rendered);
  const record = {
    at: new Date().toISOString(),
    operation,
    http_status: httpStatus,
    elapsed_ms: Math.round(elapsedMs * 1_000) / 1_000,
    result_chars: renderedText.length,
    source_text_chars: sourceTextChars,
    metadata_chars: Math.max(0, renderedText.length - sourceTextChars),
    request_id: findField(response, ["request_id"]),
    service_status: findField(response, ["status"]),
    session_id: findField(response, ["session_id"]),
    corpus_revision: findField(response, ["corpus_revision", "revision_id"]),
    checkpoint_id: findField(response, ["checkpoint_id"]),
    http_calls: operation === "asset.fetch" ? 2 : 1,
    binary_bytes: Number.isSafeInteger(binaryBytes) && binaryBytes >= 0
      ? binaryBytes
      : 0,
    source_paths: sourcePaths,
    asset_ref: (operation === "asset.fetch" || operation === "asset.metadata")
      ? findField(rendered, ["asset_ref"])
      : undefined,
    asset_version: (operation === "asset.fetch" || operation === "asset.metadata")
      ? findField(rendered, ["version"])
      : undefined,
    asset_content_hash: (operation === "asset.fetch" || operation === "asset.metadata")
      ? findField(rendered, ["content_hash"])
      : undefined,
    asset_size_bytes: (operation === "asset.fetch" || operation === "asset.metadata")
      ? findField(rendered, ["size_bytes"])
      : undefined,
    asset_local_path: operation === "asset.fetch"
      ? findField(rendered, ["local_path"])
      : undefined,
  };
  try {
    await appendFile(tracePath, `${JSON.stringify(record)}\n`, { encoding: "utf8", mode: 0o600 });
  } catch {
    // Evaluation telemetry must never alter the result of a memory operation.
  }
}

function findField(value: unknown, names: string[]): unknown {
  if (Array.isArray(value)) {
    for (const child of value) {
      const match = findField(child, names);
      if (match !== undefined && match !== null) {
        return match;
      }
    }
    return undefined;
  }
  if (typeof value !== "object" || value === null) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  for (const name of names) {
    if (record[name] !== undefined && record[name] !== null) {
      return record[name];
    }
  }
  for (const child of Object.values(record)) {
    const match = findField(child, names);
    if (match !== undefined && match !== null) {
      return match;
    }
  }
  return undefined;
}

function countSourceTextChars(value: unknown): number {
  if (Array.isArray(value)) {
    return value.reduce((total, child) => total + countSourceTextChars(child), 0);
  }
  if (typeof value !== "object" || value === null) {
    return 0;
  }
  let total = 0;
  for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
    if ((key === "content" || key === "text") && typeof child === "string") {
      total += child.length;
    } else {
      total += countSourceTextChars(child);
    }
  }
  return total;
}

function collectSourcePaths(value: unknown): string[] {
  const paths = new Set<string>();
  const visit = (child: unknown): void => {
    if (Array.isArray(child)) {
      child.forEach(visit);
      return;
    }
    if (typeof child !== "object" || child === null) {
      return;
    }
    for (const [key, nested] of Object.entries(child as Record<string, unknown>)) {
      if (
        key === "path"
        && typeof nested === "string"
        && nested.length > 0
        && !nested.startsWith("/")
      ) {
        paths.add(nested.replace(/^\.\//, ""));
      }
      visit(nested);
    }
  };
  visit(value);
  return [...paths].sort();
}
