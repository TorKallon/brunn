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
const checkpointSourceReference = reference.describe(
  "An exact entry:... reference or relative Markdown path returned by search/read.",
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
      state_refs: z.array(reference).optional(),
      acceptance_gates: z.array(z.union([z.string(), jsonObject])).optional(),
    }),
    source_refs: z.array(checkpointSourceReference).optional(),
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
  ]).has(name);
  server.registerTool(name, {
    description,
    inputSchema,
    annotations: {
      readOnlyHint: readOnly,
      destructiveHint: false,
      idempotentHint: readOnly,
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
