import assert from "node:assert/strict";
import test from "node:test";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";

import { BrunnApiClient } from "./api-client.js";
import { createBrunnMcpServer } from "./index.js";

interface RecordedCall {
  url: string;
  method: string;
  body: string | undefined;
}

async function connectedPair(
  calls: RecordedCall[],
  responseBody: Record<string, unknown>,
): Promise<{ client: Client; close: () => Promise<void> }> {
  const fetchImpl: typeof fetch = async (input, init) => {
    calls.push({
      url: String(input),
      method: init?.method ?? "GET",
      body: typeof init?.body === "string" ? init.body : undefined,
    });
    return new Response(JSON.stringify(responseBody), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const server = createBrunnMcpServer(
    new BrunnApiClient("https://api.invalid", "test-token", fetchImpl),
  );
  const client = new Client({ name: "notification-tools-test", version: "0.1.0" });
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
  assert.equal(typeof first?.text, "string");
  return JSON.parse(first?.text ?? "") as Record<string, unknown>;
}

test("notification.publish posts the typed attention decision verbatim", async () => {
  const calls: RecordedCall[] = [];
  const response = {
    notification: {
      notification_ref: "notification:019f8800000070008000000000000001",
      kind: "news_alert",
      importance: "important",
      title: "A material update",
      body: "The underlying fact changed.",
      target: {
        type: "briefing",
        date: "2026-08-02",
        edition: "morning",
        item_id: "material-update",
      },
      occurred_at: "2026-08-02T18:00:00Z",
      deliveries: [],
    },
    replayed: false,
    delivery_count: 0,
  };
  const input = {
    event_key: "story:material-update:fact-v2",
    correlation_id: "briefing:2026-08-02:morning",
    kind: "news_alert",
    importance: "important",
    title: "A material update",
    body: "The underlying fact changed.",
    source: {
      type: "briefing_item",
      ref: "entry:019f8800-0000-7000-8000-000000000001",
      version_ref: "entry:019f8800-0000-7000-8000-000000000001@2",
    },
    target: {
      type: "briefing",
      date: "2026-08-02",
      edition: "morning",
      item_id: "material-update",
    },
    occurred_at: "2026-08-02T18:00:00Z",
    expires_at: "2026-08-03T18:00:00Z",
  };
  const { client, close } = await connectedPair(calls, response);

  try {
    const result = await client.callTool({ name: "notification.publish", arguments: input });
    assert.notEqual(result.isError, true);
    assert.deepEqual(calls, [{
      url: "https://api.invalid/v1/workspace/notifications/publish",
      method: "POST",
      body: JSON.stringify(input),
    }]);
    assert.deepEqual(parseToolText(result.content), response);
  } finally {
    await close();
  }
});

test("notification.publish is a retry-safe non-destructive write tool", async () => {
  const { client, close } = await connectedPair([], { status: "ok" });
  try {
    const tool = (await client.listTools()).tools.find(
      (candidate) => candidate.name === "notification.publish",
    );
    assert.equal(tool?.annotations?.readOnlyHint, false);
    assert.equal(tool?.annotations?.destructiveHint, false);
    assert.equal(tool?.annotations?.idempotentHint, true);
    assert.equal(tool?.annotations?.openWorldHint, false);
  } finally {
    await close();
  }
});
