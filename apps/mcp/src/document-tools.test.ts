import assert from "node:assert/strict";
import test from "node:test";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";

import { StraylightApiClient } from "./api-client.js";
import { createStraylightMcpServer } from "./index.js";

interface RecordedCall {
  url: string;
  method: string;
  body: string | undefined;
}

function recordingFetch(
  calls: RecordedCall[],
  status: number,
  responseBody: Record<string, unknown>,
): typeof fetch {
  return async (input, init) => {
    calls.push({
      url: String(input),
      method: init?.method ?? "GET",
      body: typeof init?.body === "string" ? init.body : undefined,
    });
    return new Response(JSON.stringify(responseBody), {
      status,
      headers: { "content-type": "application/json" },
    });
  };
}

async function connectedPair(fetchImpl: typeof fetch): Promise<{
  client: Client;
  close: () => Promise<void>;
}> {
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const server = createStraylightMcpServer(
    new StraylightApiClient("https://api.invalid", "test-token", fetchImpl),
    { includeStructuredContent: false },
  );
  const client = new Client({ name: "document-tools-test", version: "0.1.0" });
  await server.connect(serverTransport);
  await client.connect(clientTransport);
  return {
    client,
    close: async () => {
      await client.close().catch(() => undefined);
      await server.close().catch(() => undefined);
    },
  };
}

function parseToolText(content: unknown): Record<string, unknown> {
  assert.ok(Array.isArray(content));
  const first = content[0] as { type?: string; text?: string } | undefined;
  assert.equal(first?.type, "text");
  if (typeof first?.text !== "string") {
    throw new Error("MCP tool response did not contain text");
  }
  return JSON.parse(first.text) as Record<string, unknown>;
}

test("document.publish posts curated Markdown verbatim and preserves direct links", async () => {
  const calls: RecordedCall[] = [];
  const envelope = {
    status: "committed",
    data: {
      slug: "europe-summer-plan",
      title: "Europe summer plan",
      path: "Documents/europe-summer-plan.md",
      entry_ref: "entry:019f8800-0000-7000-8000-000000000001",
      version: 2,
      url: "https://straylight.example/documents/europe-summer-plan",
      version_url: "https://straylight.example/documents/europe-summer-plan?version=2",
    },
  };
  const { client, close } = await connectedPair(recordingFetch(calls, 200, envelope));
  const input = {
    slug: "europe-summer-plan",
    title: "Europe summer plan",
    body_md: "## Route\n\nA polished, source-backed itinerary.",
    summary: "A two-week rail itinerary.",
    sources: [
      {
        label: "Current trip record",
        entry_ref: "entry:019f8800-0000-7000-8000-000000000002",
      },
      { label: "Rail operator", url: "https://example.com/timetables" },
    ],
    idempotency_key: "document:europe-summer-plan:2",
    expected_version: 1,
  };

  try {
    const result = await client.callTool({ name: "document.publish", arguments: input });
    assert.notEqual(result.isError, true);
    assert.equal(calls.length, 1);
    assert.equal(calls[0]?.url, "https://api.invalid/v1/workspace/documents/publish");
    assert.equal(calls[0]?.method, "POST");
    assert.deepEqual(JSON.parse(calls[0]?.body ?? ""), input);
    assert.deepEqual(parseToolText(result.content), envelope);
  } finally {
    await close();
  }
});

test("document.get issues bodyless current and historical GET requests", async () => {
  const calls: RecordedCall[] = [];
  const envelope = {
    status: "complete",
    data: {
      slug: "feature-specification",
      url: "https://straylight.example/documents/feature-specification",
      version_url: "https://straylight.example/documents/feature-specification?version=3",
    },
  };
  const { client, close } = await connectedPair(recordingFetch(calls, 200, envelope));

  try {
    const current = await client.callTool({
      name: "document.get",
      arguments: { slug: "feature-specification" },
    });
    const historical = await client.callTool({
      name: "document.get",
      arguments: { slug: "feature-specification", version: 3 },
    });

    assert.notEqual(current.isError, true);
    assert.notEqual(historical.isError, true);
    assert.deepEqual(calls, [
      {
        url: "https://api.invalid/v1/workspace/documents/feature-specification",
        method: "GET",
        body: undefined,
      },
      {
        url: "https://api.invalid/v1/workspace/documents/feature-specification?version=3",
        method: "GET",
        body: undefined,
      },
    ]);
    assert.deepEqual(parseToolText(current.content), envelope);
    assert.deepEqual(parseToolText(historical.content), envelope);
  } finally {
    await close();
  }
});

test("document tools expose the request-directed publication boundary and safe annotations", async () => {
  const { client, close } = await connectedPair(recordingFetch([], 200, { status: "ok" }));

  try {
    const tools = (await client.listTools()).tools;
    const publish = tools.find((tool) => tool.name === "document.publish");
    assert.ok(publish);
    assert.match(publish.description ?? "", /user asks to show, open, or read/);
    assert.match(publish.description ?? "", /Do not use it for routine replies, raw imports/);
    assert.match(publish.description ?? "", /stable latest-document link/);
    assert.match(publish.description ?? "", /stable `url` field/);
    assert.deepEqual(
      [...(publish.inputSchema.required ?? [])].sort(),
      ["body_md", "slug", "title"],
    );
    assert.equal(
      (publish.inputSchema.properties?.body_md as { maxLength?: number } | undefined)?.maxLength,
      4 * 1024 * 1024,
    );
    assert.equal(
      (publish.inputSchema.properties?.sources as { maxItems?: number } | undefined)?.maxItems,
      32,
    );
    assert.equal(publish.annotations?.readOnlyHint, false);
    assert.equal(publish.annotations?.destructiveHint, false);
    assert.equal(publish.annotations?.idempotentHint, false);

    const get = tools.find((tool) => tool.name === "document.get");
    assert.ok(get);
    assert.match(get.description ?? "", /Omit version for the stable latest document/);
    assert.match(get.description ?? "", /`version_url` only/);
    assert.deepEqual([...(get.inputSchema.required ?? [])], ["slug"]);
    assert.equal(get.annotations?.readOnlyHint, true);
    assert.equal(get.annotations?.destructiveHint, false);
    assert.equal(get.annotations?.idempotentHint, true);
  } finally {
    await close();
  }
});

test("document tools reject unsafe slugs and ambiguous provenance before the API call", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(recordingFetch(calls, 200, { status: "ok" }));
  const base = {
    title: "Feature specification",
    body_md: "## Scope\n\nA polished specification.",
  };

  try {
    for (const arguments_ of [
      { ...base, slug: "feature-specification-" },
      { ...base, slug: `a${"-".repeat(79)}a` },
      {
        ...base,
        slug: "feature-specification",
        sources: [{
          label: "Ambiguous source",
          entry_ref: "entry:019f8800-0000-7000-8000-000000000002",
          url: "https://example.com/source",
        }],
      },
      {
        ...base,
        slug: "feature-specification",
        sources: [{ label: "Unsafe source", url: "https://user:secret@example.com/source" }],
      },
    ]) {
      const result = await client.callTool({
        name: "document.publish",
        arguments: arguments_,
      });
      assert.equal(result.isError, true);
      assert.match(JSON.stringify(result.content), /Invalid arguments/);
    }
    assert.equal(calls.length, 0);
  } finally {
    await close();
  }
});

test("document tools preserve structured upstream failures", async () => {
  const failure = {
    error: {
      code: "document_not_found",
      message: "published document not found",
    },
  };
  const { client, close } = await connectedPair(recordingFetch([], 404, failure));

  try {
    const result = await client.callTool({
      name: "document.get",
      arguments: { slug: "missing-document" },
    });
    assert.equal(result.isError, true);
    assert.deepEqual(parseToolText(result.content), failure);
  } finally {
    await close();
  }
});
