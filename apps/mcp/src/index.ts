#!/usr/bin/env node

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod/v4";

import { StraylightApiClient, StraylightApiError } from "./api-client.js";

const reference = z.string().min(1);
const jsonObject = z.record(z.string(), z.unknown());

const queryItem = z.object({
  id: z.string().optional(),
  goal: z.string().optional(),
  query: z.string().min(1),
  scope: z.string().optional(),
  modes: z.array(z.enum(["exact", "structured", "lexical", "semantic", "temporal", "relations"])).optional(),
  where: jsonObject.optional(),
  state_filter: jsonObject.optional(),
  expand: jsonObject.optional(),
  limit: z.number().int().min(1).max(100).optional(),
});

const readItem = z.object({
  ref: reference.optional(),
  path: z.string().min(1).optional(),
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
  max_chars: z.number().int().min(1).max(500_000).optional(),
}).refine((value) => value.ref !== undefined || value.path !== undefined, {
  message: "read request requires ref or path",
});

const client = new StraylightApiClient(
  process.env.STRAYLIGHT_API_URL ?? "http://api:18110",
  requiredEnvironment("STRAYLIGHT_API_TOKEN"),
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
    resume_checkpoint_ref: reference.optional(),
    token_budget: z.number().int().min(1).optional(),
  },
  (input) => client.request("/v1/memory/open", input),
);

registerJsonTool(
  "memory.query",
  "Run one or more exact, structured, lexical, or semantic queries against a pinned session.",
  {
    session_id: reference,
    queries: z.array(queryItem).min(1).max(32),
  },
  (input) => client.request("/v1/memory/query", input),
);

registerJsonTool(
  "memory.read",
  "Read exact source, evidence, object, relation, claim, checkpoint, or revision ranges by stable reference.",
  {
    session_id: reference,
    requests: z.array(readItem).min(1).max(32),
  },
  (input) => client.request("/v1/memory/read", input),
);

registerJsonTool(
  "memory.compute",
  "Run bounded declarative graph, temporal, spatial, unit, aggregation, or acceptance-gate operations.",
  {
    session_id: reference,
    steps: z.array(z.object({
      id: z.string().min(1),
      op: z.string().min(1),
      input: jsonObject,
    })).min(1).max(32),
    max_rows: z.number().int().min(1).max(10_000).optional(),
    token_budget: z.number().int().min(1).optional(),
  },
  (input) => client.request("/v1/memory/compute", input),
);

registerJsonTool(
  "memory.verify",
  "Classify support, contradiction, supersession, and temporal applicability using source-bearing evidence.",
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
  "Commit an immutable child checkpoint linked to its session, corpus revision, parent, state, gates, and evidence.",
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
    source_refs: z.array(reference).optional(),
  },
  (input) => client.request("/v1/memory/checkpoint", input),
);

registerJsonTool(
  "memory.stage",
  "Stage files from the adapter's sandboxed import root without placing binary content in model context.",
  {
    scope: z.string().min(1),
    stable_import_id: z.string().min(1).optional(),
    files: z.array(z.object({
      path: z.string().min(1),
      name: z.string().min(1).optional(),
      media_type: z.string().optional(),
    })).min(1).max(32),
  },
  (input) => client.stage(input.scope, input.stable_import_id, input.files),
);

registerJsonTool(
  "memory.status",
  "Inspect the credential's current corpus and embedding service status.",
  {},
  () => client.request("/v1/status"),
);

await server.connect(new StdioServerTransport());

function registerJsonTool<Shape extends z.ZodRawShape>(
  name: string,
  description: string,
  inputSchema: Shape,
  invoke: (input: z.infer<z.ZodObject<Shape>>) => Promise<{ body: Record<string, unknown> }>,
): void {
  const callback = async (input: z.infer<z.ZodObject<Shape>>) => {
    try {
      const response = await invoke(input);
      return {
        content: [{ type: "text" as const, text: JSON.stringify(response.body) }],
        structuredContent: response.body,
      };
    } catch (error) {
      const body = error instanceof StraylightApiError
        ? error.body
        : { error: { code: "adapter_error", message: errorMessage(error) } };
      return {
        isError: true,
        content: [{ type: "text" as const, text: JSON.stringify(body) }],
        structuredContent: body,
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
