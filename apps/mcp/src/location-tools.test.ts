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
  response: { status: number; body: Record<string, unknown> } = {
    status: 200,
    body: { status: "complete", data: { ok: true } },
  },
): Promise<{
  client: Client;
  close: () => Promise<void>;
}> {
  const fetchImpl: typeof fetch = async (input, init) => {
    calls.push({
      url: String(input),
      method: init?.method ?? "GET",
      body: typeof init?.body === "string" ? init.body : undefined,
    });
    return new Response(JSON.stringify(response.body), {
      status: response.status,
      headers: { "content-type": "application/json" },
    });
  };
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const server = createBrunnMcpServer(
    new BrunnApiClient("https://api.invalid", "test-token", fetchImpl),
  );
  const client = new Client({ name: "location-tools-test", version: "0.1.0" });
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

test("location tools expose only presence and rederive with the approved route contracts", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls);
  try {
    const tools = (await client.listTools()).tools
      .filter((tool) => tool.name.startsWith("location."));
    assert.deepEqual(tools.map((tool) => tool.name).sort(), [
      "location.presence",
      "location.rederive",
    ]);

    const presence = tools.find((tool) => tool.name === "location.presence");
    const rederive = tools.find((tool) => tool.name === "location.rederive");
    assert.equal(presence?.annotations?.readOnlyHint, true);
    assert.equal(presence?.annotations?.idempotentHint, true);
    assert.equal(rederive?.annotations?.readOnlyHint, false);
    assert.equal(rederive?.annotations?.destructiveHint, false);
    assert.equal(rederive?.annotations?.idempotentHint, true);
    assert.deepEqual(rederive?.inputSchema.required ?? [], []);
    assert.deepEqual(Object.keys(rederive?.inputSchema.properties ?? {}).sort(), ["from", "to"]);

    await client.callTool({ name: "location.presence", arguments: {} });
    await client.callTool({
      name: "location.rederive",
      arguments: {
        from: "2026-08-20T00:00:00-07:00",
        to: "2026-09-01T23:59:00-07:00",
      },
    });
    assert.deepEqual(calls, [
      {
        url: "https://api.invalid/v1/location/presence",
        method: "GET",
        body: undefined,
      },
      {
        url: "https://api.invalid/v1/location/rederive",
        method: "POST",
        body: JSON.stringify({
          from: "2026-08-20T00:00:00-07:00",
          to: "2026-09-01T23:59:00-07:00",
        }),
      },
    ]);
  } finally {
    await close();
  }
});

test("location.presence translates only the API no-row 404 into status none", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls, {
    status: 404,
    body: {
      error: {
        code: "location_presence_not_found",
        message: "location presence not found",
      },
    },
  });
  try {
    const result = await client.callTool({ name: "location.presence", arguments: {} });
    assert.notEqual(result.isError, true);
    assert.deepEqual(parseToolText(result.content), { status: "none" });
    assert.deepEqual(calls, [{
      url: "https://api.invalid/v1/location/presence",
      method: "GET",
      body: undefined,
    }]);
  } finally {
    await close();
  }

  const otherCalls: RecordedCall[] = [];
  const other = await connectedPair(otherCalls, {
    status: 404,
    body: {
      error: {
        code: "route_not_found",
        message: "route not found",
      },
    },
  });
  try {
    const result = await other.client.callTool({
      name: "location.presence",
      arguments: {},
    });
    assert.equal(result.isError, true);
    assert.deepEqual(parseToolText(result.content), {
      error: { code: "route_not_found", message: "route not found" },
    });
  } finally {
    await other.close();
  }
});
