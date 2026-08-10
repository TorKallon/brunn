#!/usr/bin/env node

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { appendFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { z } from "zod/v4";

import { type ApiResponse, StraylightApiClient, StraylightApiError } from "./api-client.js";
import { compactReasoningResponse } from "./reasoning-view.js";

const reference = z.string().min(1);
const assetReference = z.string()
  .regex(/^entry:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i)
  .describe("Exact entry:... binary reference copied from a Straylight response.");
const jsonObject = z.record(z.string(), z.unknown());
const MAX_CHECKPOINT_BYTES = 4 * 1024 * 1024;
const MAX_CHECKPOINT_ITEMS = 4_096;
const checkpointIdentityReference = printableUtf8String(256);
const checkpointStateReference = printableUtf8String(4_096);
const checkpointSourceReference = printableUtf8String(4_096).describe(
  "An exact entry:... reference or relative Markdown path returned by search/read.",
);
const checkpointIdempotencyKey = z.string().min(1).max(256).refine(
  (value) => Buffer.byteLength(value, "utf8") <= 256
    && !/[\u0000-\u001f\u007f-\u009f]/u.test(value),
  "idempotency_key must contain at most 256 UTF-8 bytes and no control characters",
).describe(
  "Stable replay identity. Reuse this exact key with an identical checkpoint payload after an ambiguous outcome.",
);
const checkpointText = z.string().max(MAX_CHECKPOINT_BYTES).refine(
  (value) => Buffer.byteLength(value, "utf8") <= MAX_CHECKPOINT_BYTES,
  "checkpoint strings are limited to 4 MiB of UTF-8 text",
);
const checkpointStructuredItem = jsonObject.refine(
  (value) => serializedUtf8Length(value) <= MAX_CHECKPOINT_BYTES,
  "structured checkpoint items are limited to 4 MiB of serialized UTF-8 JSON",
);
const checkpointState = z.object({
  objective: checkpointText.min(1),
  current_state: z.union([
    checkpointText,
    z.array(checkpointText).max(MAX_CHECKPOINT_ITEMS),
  ]).optional(),
  decisions: z.array(checkpointText).max(MAX_CHECKPOINT_ITEMS).optional(),
  open_questions: z.array(checkpointText).max(MAX_CHECKPOINT_ITEMS).optional(),
  next_actions: z.array(checkpointText).max(MAX_CHECKPOINT_ITEMS).optional(),
  artifacts: z.array(checkpointText).max(MAX_CHECKPOINT_ITEMS).optional(),
  ordered_goals: z.array(
    z.union([checkpointText, checkpointStructuredItem]),
  ).max(MAX_CHECKPOINT_ITEMS).optional(),
  state_refs: z.array(checkpointStateReference).max(MAX_CHECKPOINT_ITEMS).optional(),
  acceptance_gates: z.array(
    z.union([checkpointText, checkpointStructuredItem]),
  ).max(MAX_CHECKPOINT_ITEMS).optional(),
}).refine(
  (value) => serializedUtf8Length(value) <= MAX_CHECKPOINT_BYTES,
  "checkpoint state is limited to 4 MiB of serialized UTF-8 JSON",
);

const queryItem = z.object({
  id: z.string().optional(),
  goal: z.string().optional(),
  query: z.string().min(1),
  modes: z.array(
    z.enum(["exact", "lexical", "semantic"]),
  ).optional().describe(
    "Omit for hybrid search. Use exact for a literal path or title, lexical for words, or semantic for meaning.",
  ),
  limit: z.number().int().min(1).max(50).default(8),
});

const editionDate = z.string().regex(/^\d{4}-\d{2}-\d{2}$/).describe(
  "Exact edition date YYYY-MM-DD.",
);
const storyKey = z.string().regex(/^[a-z0-9][a-z0-9-]{2,79}$/);
const storyUrl = z.string().min(1).max(2_048);

const briefingStoryRef = z.object({
  key: storyKey.describe(
    "Lowercase story slug. When briefing.dedupe returned this story, copy its story_key " +
    "verbatim; never invent a variant of a key the ledger already has. Mint a new slug only " +
    "for a story with no dedupe match.",
  ),
  urls: z.array(storyUrl).max(8).optional().describe(
    "Canonical source URLs for the story; the service canonicalizes and hashes them for dedupe.",
  ),
  title: z.string().max(500).optional(),
  entities: z.array(z.string().max(120)).max(16).optional(),
  event_at: z.string().regex(/^\d{4}-\d{2}-\d{2}$/).optional().describe(
    "Exact date the underlying event happened, YYYY-MM-DD. Omit this field when unknown; never guess.",
  ),
});

const briefingTimes = z.object({
  published_at: z.string().optional(),
  event_at: z.string().optional(),
  first_seen_at: z.string().optional(),
});

const briefingItem = z.object({
  id: z.string().regex(/^[a-z0-9][a-z0-9-]{1,63}$/).describe(
    "Lowercase item slug, unique within the edition. Reuse the same id when republishing an " +
    "unchanged or revised item so revision deltas stay accurate.",
  ),
  kind: z.enum(["news", "metric", "health", "ops", "digest", "tracker", "schedule"]),
  headline_md: z.string().max(500),
  body_md: z.string().max(4_000).optional(),
  why_it_matters: z.string().max(1_000).optional(),
  detail_md: z.string().max(16_000).optional(),
  what_changed: z.string().max(1_000).optional(),
  delta: z.enum(["new", "update", "corroboration"]).optional().describe(
    "Omit this field for a first delivery; the service records new. Use update or corroboration " +
    "only when briefing.dedupe showed the story was already delivered.",
  ),
  story: briefingStoryRef.optional(),
  times: briefingTimes.optional(),
});

const briefingSection = z.object({
  topic: z.string().max(80).describe("Exact topic slug from briefing.topics; never invent one."),
  title: z.string().max(200),
  items: z.array(briefingItem).max(32),
});

const briefingOmission = z.object({
  story_key: storyKey.optional().describe(
    "Story key for the omitted story; copy it verbatim from the briefing.dedupe result that " +
    "identified the duplicate when one exists.",
  ),
  urls: z.array(storyUrl).max(8).optional(),
  reason: z.string().min(1).max(1_000),
});

const dedupeCandidate = z.object({
  urls: z.array(storyUrl).max(8).optional(),
  title: z.string().max(500).optional(),
  summary: z.string().max(4_000).optional(),
  event_at: z.string().regex(/^\d{4}-\d{2}-\d{2}$/).optional().describe(
    "Exact date the underlying event happened, YYYY-MM-DD. Omit this field when unknown; never guess.",
  ),
  topic: z.string().max(80).optional(),
  story_key: storyKey.optional().describe(
    "Story key to look up exactly. Copy keys verbatim from prior briefing.dedupe or " +
    "briefing.publish results when checking a known story; a key absent from the ledger " +
    "simply returns no match.",
  ),
});

const notificationSource = z.object({
  type: z.string().min(1).max(64),
  ref: z.string().min(1).max(500),
  version_ref: z.string().min(1).max(500).optional(),
});

const notificationTarget = z.discriminatedUnion("type", [
  z.object({ type: z.literal("notification") }),
  z.object({ type: z.literal("today") }),
  z.object({
    type: z.literal("briefing"),
    date: editionDate,
    edition: z.string().min(1).max(64),
    item_id: z.string().min(1).max(200).optional(),
  }),
  z.object({
    type: z.literal("entry"),
    entry_ref: z.string().min(1).max(500).describe(
      "Exact entry:... reference returned by Straylight; never infer one from a title or path.",
    ),
  }),
]);

function createReadItem(maxChars: number) {
  return z.object({
    ref: reference.optional().describe(
      "Exact record reference copied verbatim from a CarryState response. Never infer or invent a reference.",
    ),
    path: z.string().min(1).optional().describe(
      "Exact source path copied verbatim from a CarryState response. Never synthesize a filename from a title or topic.",
    ),
    view: z.enum([
      "current_state",
      "current_truth",
      "outline",
      "full",
      "range",
    ]).optional(),
    start: z.number().int().min(1).optional(),
    end: z.number().int().min(1).optional(),
    max_chars: z.number().int().min(1).max(maxChars).optional(),
  }).refine((value) => value.ref !== undefined || value.path !== undefined, {
    message: "read request requires ref or path",
  });
}

export interface StraylightMcpServerOptions {
  surface?: "local" | "remote";
  includeStructuredContent?: boolean;
  maxReadChars?: number;
}

export function createStraylightMcpServer(
  client: StraylightApiClient,
  options: StraylightMcpServerOptions = {},
): McpServer {
  const surface = options.surface ?? "local";
  const includeStructuredContent = options.includeStructuredContent
    ?? process.env.STRAYLIGHT_MCP_INCLUDE_STRUCTURED_CONTENT === "1";
  const maxReadChars = options.maxReadChars ?? (surface === "remote" ? 120_000 : 500_000);
  const server = new McpServer({
    name: "straylight",
    version: "0.1.0",
  }, surface === "remote" ? {
    instructions:
      "Straylight is the durable context store. Start substantive work with memory.open for the actual task, " +
      "then use memory.query and memory.read only for relevant evidence. Persist source material with " +
      "memory.capture, durable current state or corrections with memory.write, and resumable work with " +
      "memory.checkpoint. If Straylight is unavailable, fail closed instead of inventing or substituting context.",
  } : {});

  function registerJsonTool<Shape extends z.ZodRawShape>(
    name: string,
    description: string,
    inputSchema: Shape,
    invoke: (input: z.infer<z.ZodObject<Shape>>) => Promise<ApiResponse>,
  ): void {
    registerJsonToolOnServer(
      server,
      includeStructuredContent,
      name,
      description,
      inputSchema,
      invoke,
    );
  }

registerJsonTool(
  "memory.open",
  "Open or resume the workspace and receive bounded, coherent source documents relevant to the task.",
  {
    task: z.string().min(1),
    hints: z.object({
      authorization_scope: z.string().optional(),
      root_refs: z.array(reference).optional(),
      open_object_refs: z.array(reference).optional(),
    }).optional(),
    resume_checkpoint_ref: reference.optional().describe(
      "Exact checkpoint:... reference supplied by the caller. Omit this field when no exact " +
      "checkpoint reference was supplied; never invent one or use placeholders such as latest.",
    ),
    token_budget: z.number().int().min(1).optional(),
    modes: z.array(
      z.enum(["exact", "lexical", "semantic"]),
    ).optional().describe(
      "Omit for the server policy. Evaluation arms may explicitly restrict open to exact and lexical retrieval.",
    ),
  },
  (input) => client.request("/v1/workspace/open", input),
);

registerJsonTool(
  "memory.query",
  "Search current workspace files by exact path or title, full text, and semantic similarity.",
  {
    session_id: reference,
    queries: z.array(queryItem).min(1).max(16),
    token_budget: z.number().int().min(1).optional().describe(
      "Optional total search response budget in tokens; the service converts it to a bounded character cap when budget-contracted retrieval is enabled.",
    ),
  },
  (input) => client.request("/v1/workspace/search", input),
);

registerJsonTool(
  "memory.read",
  "Batch exact reads of current Markdown files or checkpoints by returned entry reference or path.",
  {
    session_id: reference,
    requests: z.array(createReadItem(maxReadChars)).min(1).max(32),
  },
  (input) => client.request("/v1/workspace/read", input),
);

registerJsonTool(
  "memory.changes",
  "Page through workspace changes after an exact generation cursor.",
  {
    since_generation: z.number().int().nonnegative().default(0),
    limit: z.number().int().min(1).max(2_000).default(200),
  },
  (input) => client.workspaceChanges(input.since_generation, input.limit),
);

registerJsonTool(
  "memory.capture",
  "Persist ordinary source-backed context as a durable Markdown capture.",
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
    intent: z.string().min(1).optional(),
    idempotency_key: z.string().min(1).max(240).optional(),
  },
  (input) => client.request("/v1/workspace/capture", input),
);

registerJsonTool(
  "memory.write",
  "Create or update one Markdown workspace file. Supply expected_version only when preventing a known stale overwrite matters.",
  {
    path: z.string().min(1).max(1_024),
    content: z.string().max(4 * 1024 * 1024),
    media_type: z.enum(["text/markdown", "text/plain"]).default("text/markdown"),
    expected_version: z.number().int().nonnegative().optional(),
    idempotency_key: z.string().min(1).max(240).optional(),
    metadata: jsonObject.optional(),
  },
  (input) => client.request("/v1/workspace/write", input),
);

registerJsonTool(
  "memory.checkpoint",
  "Write a deterministic checkpoint Markdown file with exact file/version/hash references and a workspace generation.",
  {
    session_id: checkpointIdentityReference,
    parent_checkpoint_id: checkpointIdentityReference.optional(),
    idempotency_key: checkpointIdempotencyKey,
    state: checkpointState,
    source_refs: z.array(checkpointSourceReference).max(64).optional(),
  },
  (input) => client.request("/v1/workspace/checkpoint", input),
);

if (surface === "local") {
  registerJsonTool(
    "memory.stage",
    "Upload binary files from the adapter's sandboxed import root without placing bytes in model context.",
    {
      scope: z.string().min(1),
      stable_import_id: z.string().min(1).max(240).optional(),
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
}

registerJsonTool(
  "memory.status",
  "Inspect current service and dependency status.",
  {},
  () => client.request("/v1/status"),
);

registerJsonTool(
  "asset.list",
  "List current binary workspace entries and their exact hashes, versions, sizes, and description metadata.",
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
  "Read metadata for one exact binary workspace entry and optional historical version without downloading bytes.",
  {
    session_id: reference.describe(
      "Exact session:... reference returned by memory.open and retained for workspace continuity.",
    ),
    asset_ref: assetReference,
    version: z.number().int().positive().optional().describe(
      "Optional exact historical version. Omit it to read the current version.",
    ),
  },
  (input) => client.assetMetadata(input.asset_ref, input.session_id, input.version),
);

if (surface === "local") {
  registerJsonTool(
    "asset.fetch",
    "Download one exact binary workspace entry into the MCP adapter's private asset root. " +
    "The tool verifies the streamed size and SHA-256 and returns only a local path plus integrity metadata; " +
    "asset bytes and base64 are never returned to model context.",
    {
      session_id: reference.describe(
        "Exact session:... reference returned by memory.open and retained for workspace continuity.",
      ),
      asset_ref: assetReference,
      version: z.number().int().positive().optional().describe(
        "Optional exact historical version. Metadata and bytes are both fetched at this version.",
      ),
    },
    (input) => client.fetchAsset(input.asset_ref, input.session_id, input.version),
  );
}

registerJsonTool(
  "briefing.publish",
  "Publish or revise one typed briefing edition; Straylight renders the canonical Markdown entry " +
  "and updates the delivered-story ledger. Republishing the same date and edition revises the " +
  "same entry.",
  {
    date: editionDate,
    edition: z.string().regex(/^[a-z0-9][a-z0-9-]{1,31}$/).describe(
      "Lowercase edition slug such as morning.",
    ),
    timezone: z.string().max(64).optional().describe(
      "IANA timezone name used to render generated-at times. Omit this field for the service default.",
    ),
    generated_at: z.string().max(64).optional().describe(
      "Exact RFC3339 timestamp when the briefing content was generated. Omit this field to use the publish time.",
    ),
    summary_md: z.array(z.string().max(1_000)).max(12).optional().describe(
      "30-second version: one Markdown bullet per line, most important first.",
    ),
    sections: z.array(briefingSection).max(24).optional(),
    omitted: z.array(briefingOmission).max(64).optional().describe(
      "Stories researched but deliberately not delivered, with the reason; recorded as suppressions in the ledger.",
    ),
    idempotency_key: z.string().min(1).max(240).optional(),
    expected_version: z.number().int().nonnegative().optional().describe(
      "Supply expected_version only when preventing a known stale overwrite matters.",
    ),
  },
  (input) => client.request("/v1/workspace/briefings/publish", input),
);

registerJsonTool(
  "briefing.dedupe",
  "Check candidate stories against the delivered-story ledger before publishing; returns exact " +
  "URL and story-key matches with delivery history, near matches, and a verdict hint per candidate.",
  {
    candidates: z.array(dedupeCandidate).min(1).max(64),
  },
  (input) => client.request("/v1/workspace/briefings/dedupe-check", input),
);

registerJsonTool(
  "briefing.topics",
  "Read the parsed briefing topics snapshot plus pending go-deeper requests and the recent feedback tail.",
  {
    session_id: reference.describe(
      "Exact session:... reference returned by memory.open.",
    ),
  },
  () => client.request("/v1/workspace/briefings/topics"),
);

registerJsonTool(
  "notification.publish",
  "Publish one durable user alert for the authenticated owner. Straylight deduplicates by " +
  "event_key, records the private inbox detail, and independently queues eligible device deliveries.",
  {
    event_key: z.string().min(1).max(200).describe(
      "Stable semantic identity shared by Codex and Aether. Reuse it only for the same alert content.",
    ),
    correlation_id: z.string().min(1).max(200).describe(
      "Stable correlation identity for the producing run, briefing, incident, or decision chain.",
    ),
    kind: z.enum(["briefing_ready", "news_alert", "correction", "operational"]),
    importance: z.enum(["normal", "important"]),
    title: z.string().min(1).max(240),
    body: z.string().min(1).max(20_000),
    source: notificationSource.optional().describe(
      "Exact durable source and optional pinned version supporting this attention decision.",
    ),
    target: notificationTarget.describe(
      "Typed in-app destination. Use briefing or entry when an exact durable target exists.",
    ),
    occurred_at: z.string().min(1).max(64).optional(),
    expires_at: z.string().min(1).max(64).optional().describe(
      "Optional RFC3339 expiry, no more than seven days after occurred_at. Omit for the 24-hour default.",
    ),
  },
  (input) => client.request("/v1/workspace/notifications/publish", input),
);

  return server;
}

function registerJsonToolOnServer<Shape extends z.ZodRawShape>(
  server: McpServer,
  includeStructuredContent: boolean,
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
  const readOnly = !new Set([
    "memory.capture",
    "memory.write",
    "memory.checkpoint",
    "memory.stage",
    "briefing.publish",
    "notification.publish",
  ]).has(name);
  const idempotent = readOnly
    || name === "memory.checkpoint"
    || name === "notification.publish";
  server.registerTool(name, {
    description,
    inputSchema,
    annotations: {
      readOnlyHint: readOnly,
      destructiveHint: false,
      idempotentHint: idempotent,
      openWorldHint: false,
    },
  }, callback as never);
}

async function runStdioServer(): Promise<void> {
  const client = new StraylightApiClient(
    process.env.STRAYLIGHT_API_URL ?? "http://api:18110",
    requiredEnvironment("STRAYLIGHT_API_TOKEN"),
    fetch,
    evaluationHeaders(),
  );
  await createStraylightMcpServer(client).connect(new StdioServerTransport());
}

if (
  process.argv[1] !== undefined
  && import.meta.url === pathToFileURL(process.argv[1]).href
) {
  await runStdioServer();
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

function printableUtf8String(maxBytes: number) {
  return z.string().min(1).max(maxBytes).refine(
    (value) => Buffer.byteLength(value, "utf8") <= maxBytes
      && !/[\u0000-\u001f\u007f-\u009f]/u.test(value),
    `value must contain at most ${maxBytes} UTF-8 bytes and no control characters`,
  );
}

function serializedUtf8Length(value: unknown): number {
  try {
    return Buffer.byteLength(JSON.stringify(value), "utf8");
  } catch {
    return Number.POSITIVE_INFINITY;
  }
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
