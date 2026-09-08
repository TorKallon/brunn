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

test("location tools expose required named historical and current route contracts", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls);
  try {
    const tools = (await client.listTools()).tools
      .filter((tool) => tool.name.startsWith("location."));
    for (const name of ["location.presence", "location.rederive", "location.evidence"]) {
      assert.ok(tools.some((tool) => tool.name === name), `missing ${name}`);
    }

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

test("location.presence preserves current position separately from historical visit", async () => {
  const calls: RecordedCall[] = [];
  const body = {
    status: "approximate",
    at_home: false,
    place: null,
    position: { lat: 47.6, lon: -122.2, accuracy_m: 700, observed_at: "2026-09-06T10:00:00Z", age_seconds: 120, approximate: true },
    last_seen: "2026-09-06T10:00+00:00",
    last_contact: "2026-09-06T10:01+00:00",
    visit: { label: "Home", kind: "home", confidence: "high", since: "2026-09-06T08:00+00:00" },
  };
  const { client, close } = await connectedPair(calls, { status: 200, body });
  try {
    const result = await client.callTool({ name: "location.presence", arguments: {} });
    assert.deepEqual(parseToolText(result.content), body);
    assert.equal(calls.length, 1);
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

test("location.evidence exposes bounded read-only historical evidence and preserves completeness", async () => {
  const calls: RecordedCall[] = [];
  const body = { completeness: false, reasons: ["raw_retention_boundary"], canonical: [{ entry_ref: "entry:day", version: 3 }], raw: [{ report_id: "fixture", first_received_at: null }] };
  const { client, close } = await connectedPair(calls, { status: 200, body });
  try {
    const tool = (await client.listTools()).tools.find((item) => item.name === "location.evidence");
    assert.ok(tool);
    assert.equal(tool.annotations?.readOnlyHint, true);
    assert.equal(tool.annotations?.idempotentHint, true);
    assert.deepEqual(tool.inputSchema.required?.slice().sort(), ["from", "timezone", "to"]);
    assert.match(tool.description ?? "", /Requires Save/);
    const input = { from: "2025-11-02T00:00:00-07:00", to: "2025-11-03T00:00:00-08:00", timezone: "America/Los_Angeles" };
    const result = await client.callTool({ name: "location.evidence", arguments: input });
    assert.notEqual(result.isError, true);
    assert.deepEqual(parseToolText(result.content), body);
    assert.deepEqual(calls, [{ url: `https://api.invalid/v1/location/evidence?${new URLSearchParams(input).toString()}`, method: "GET", body: undefined }]);
  } finally { await close(); }
});

test("location.evidence rejects open, oversized, and malformed historical windows before HTTP", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls);
  try {
    for (const input of [
      { from: "2025-09-07T00:00:00Z", to: "2025-09-07T00:00:00Z" },
      { from: "2025-09-07T00:00:00Z", to: "2025-09-06T00:00:00Z" },
      { from: "2025-09-07T00:00:00Z", to: "2025-09-09T00:00:00Z" },
      { from: "2099-09-07T00:00:00Z", to: "2099-09-08T00:00:00Z" },
      { from: "2025-09-07T00:00:00", to: "2025-09-08T00:00:00Z" },
    ]) {
      const result = await client.callTool({ name: "location.evidence", arguments: { ...input, timezone: "America/Los_Angeles" } });
      assert.equal(result.isError, true);
    }
    assert.deepEqual(calls, []);
  } finally { await close(); }
});
