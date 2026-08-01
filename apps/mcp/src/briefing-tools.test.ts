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
  const client = new Client({ name: "briefing-tools-test", version: "0.1.0" });
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

test("briefing.publish posts the typed edition verbatim and returns the envelope", async () => {
  const calls: RecordedCall[] = [];
  const envelope = {
    status: "complete",
    data: {
      path: "Briefings/2026/Morning briefing - 2026-08-01.md",
      entry_ref: "entry:019f8800-0000-7000-8000-000000000001",
      version_ref: "entry:019f8800-0000-7000-8000-000000000001@2",
      version: 2,
      content_hash: "sha256:2222222222222222222222222222222222222222222222222222222222222222",
      delta: { added: ["example-launch"], changed: [], removed: [] },
    },
  };
  const { client, close } = await connectedPair(recordingFetch(calls, 200, envelope));
  const publishInput = {
    date: "2026-08-01",
    edition: "morning",
    timezone: "America/New_York",
    generated_at: "2026-08-01T10:30:00Z",
    summary_md: ["**[Example](https://example.com/a)** shipped a launch."],
    sections: [{
      topic: "frontier-labs",
      title: "Frontier labs",
      items: [{
        id: "example-launch",
        kind: "news",
        headline_md: "**[Example](https://example.com/a)** shipped a launch.",
        body_md: "Launch details.",
        why_it_matters: "It changes the default.",
        delta: "new",
        story: {
          key: "example-launch-2026",
          urls: ["https://example.com/a?utm_source=x"],
          title: "Example launch",
          event_at: "2026-07-31",
        },
        times: { published_at: "2026-07-31T18:00:00Z" },
      }],
    }],
    omitted: [{ story_key: "old-story-key", reason: "delivered 2026-07-30; no new facts" }],
    idempotency_key: "briefing-2026-08-01-morning-1",
    expected_version: 1,
  };

  try {
    const result = await client.callTool({ name: "briefing.publish", arguments: publishInput });
    assert.notEqual(result.isError, true);
    assert.equal(calls.length, 1);
    assert.equal(calls[0]?.url, "https://api.invalid/v1/workspace/briefings/publish");
    assert.equal(calls[0]?.method, "POST");
    assert.deepEqual(JSON.parse(calls[0]?.body ?? ""), publishInput);
    assert.deepEqual(parseToolText(result.content), envelope);
  } finally {
    await close();
  }
});

test("briefing.dedupe posts candidates and trims near lanes while keeping exact history", async () => {
  const calls: RecordedCall[] = [];
  const exactHit = {
    story_key: "example-launch-2026",
    title: "Example launch",
    last_delivered_date: "2026-07-30",
    last_delivered_edition_ref: "entry:019f8800-0000-7000-8000-000000000001",
    last_delivered_headline: "**[Example](https://example.com/a)** shipped a launch.",
    delivery_count: 2,
    suppression_count: 0,
    matched_by: ["url", "story_key"],
  };
  const envelope = {
    status: "complete",
    corpus_revision: "generation:42",
    data: {
      candidates: [{
        exact: [exactHit],
        near: [
          {
            lane: "ledger_titles",
            story_key: "example-beta",
            title: "Example beta program",
            last_delivered_date: "2026-07-20",
            delivery_count: 1,
          },
          {
            lane: "workspace_lexical",
            path: "Briefings/2026/Morning briefing - 2026-07-30.md",
            title: "Morning briefing - 2026-07-30",
            heading: "Frontier labs",
            score: 12.5,
          },
        ],
        verdict_hint: "duplicate",
      }],
      workspace_generation: 42,
    },
  };
  const { client, close } = await connectedPair(recordingFetch(calls, 200, envelope));
  const dedupeInput = {
    candidates: [{
      urls: ["https://example.com/a?utm_source=x"],
      title: "Example launch",
      summary: "Example shipped a launch.",
      event_at: "2026-07-31",
      topic: "frontier-labs",
      story_key: "example-launch-2026",
    }],
  };

  try {
    const result = await client.callTool({ name: "briefing.dedupe", arguments: dedupeInput });
    assert.notEqual(result.isError, true);
    assert.equal(calls.length, 1);
    assert.equal(calls[0]?.url, "https://api.invalid/v1/workspace/briefings/dedupe-check");
    assert.equal(calls[0]?.method, "POST");
    assert.deepEqual(JSON.parse(calls[0]?.body ?? ""), dedupeInput);
    assert.deepEqual(parseToolText(result.content), {
      status: "complete",
      corpus_revision: "generation:42",
      data: {
        workspace_generation: 42,
        candidates: [{
          verdict_hint: "duplicate",
          exact: [exactHit],
          near: [
            {
              lane: "ledger_titles",
              story_key: "example-beta",
              title: "Example beta program",
              last_delivered_date: "2026-07-20",
            },
            {
              lane: "workspace_lexical",
              title: "Morning briefing - 2026-07-30",
              path: "Briefings/2026/Morning briefing - 2026-07-30.md",
            },
          ],
        }],
      },
    });
  } finally {
    await close();
  }
});

test("briefing.topics issues a bodyless GET to the topics snapshot", async () => {
  const calls: RecordedCall[] = [];
  const envelope = {
    status: "complete",
    data: {
      topics: [{ slug: "frontier-labs", name: "Frontier labs", mode: "every_briefing" }],
      pending_requests: [],
      feedback_path: "Briefings/Feedback/2026-08.md",
      feedback_tail: "",
      workspace_generation: 42,
    },
  };
  const { client, close } = await connectedPair(recordingFetch(calls, 200, envelope));

  try {
    const result = await client.callTool({
      name: "briefing.topics",
      arguments: { session_id: "session:019f8800-0000-7000-8000-00000000000a" },
    });
    assert.notEqual(result.isError, true);
    assert.equal(calls.length, 1);
    assert.equal(calls[0]?.url, "https://api.invalid/v1/workspace/briefings/topics");
    assert.equal(calls[0]?.method, "GET");
    assert.equal(calls[0]?.body, undefined);
    assert.deepEqual(parseToolText(result.content), envelope);
  } finally {
    await close();
  }
});

test("briefing tools preserve structured upstream failures", async () => {
  const failure = {
    error: {
      code: "entry_version_conflict",
      message: "expected version 1 but the entry is at version 5",
    },
  };
  const { client, close } = await connectedPair(recordingFetch([], 409, failure));

  try {
    const result = await client.callTool({
      name: "briefing.publish",
      arguments: { date: "2026-08-01", edition: "morning", expected_version: 1 },
    });
    assert.equal(result.isError, true);
    assert.deepEqual(parseToolText(result.content), failure);
  } finally {
    await close();
  }
});

test("briefing.publish is the only briefing write tool by annotation", async () => {
  const { client, close } = await connectedPair(recordingFetch([], 200, { status: "ok" }));

  try {
    const tools = (await client.listTools()).tools;
    const publish = tools.find((tool) => tool.name === "briefing.publish");
    assert.equal(publish?.annotations?.readOnlyHint, false);
    assert.equal(publish?.annotations?.destructiveHint, false);
    assert.equal(publish?.annotations?.idempotentHint, false);
    const dedupe = tools.find((tool) => tool.name === "briefing.dedupe");
    assert.equal(dedupe?.annotations?.readOnlyHint, true);
    assert.equal(dedupe?.annotations?.destructiveHint, false);
    const topics = tools.find((tool) => tool.name === "briefing.topics");
    assert.equal(topics?.annotations?.readOnlyHint, true);
    assert.equal(topics?.annotations?.openWorldHint, false);
  } finally {
    await close();
  }
});
