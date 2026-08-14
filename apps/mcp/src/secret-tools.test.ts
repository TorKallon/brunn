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
  const server = createStraylightMcpServer(
    new StraylightApiClient("https://api.invalid", "test-token", fetchImpl),
  );
  const client = new Client({ name: "secret-tools-test", version: "0.1.0" });
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

test("secret.put posts the named value with its multiline content preserved", async () => {
  const calls: RecordedCall[] = [];
  const response = {
    secret_ref: "secret:019f8800000070008000000000000001",
    name: "deploy-key",
    version: 1,
    status: "stored",
    updated_at: "2026-08-13T00:00:00Z",
  };
  const input = {
    name: "deploy-key",
    value: "-----BEGIN KEY-----\nline-one\nline-two\n-----END KEY-----\n",
    description: "CI deploy key",
  };
  const { client, close } = await connectedPair(calls, response);
  try {
    const result = await client.callTool({ name: "secret.put", arguments: input });
    assert.notEqual(result.isError, true);
    assert.deepEqual(calls, [{
      url: "https://api.invalid/v1/workspace/secrets/put",
      method: "POST",
      body: JSON.stringify(input),
    }]);
    assert.deepEqual(parseToolText(result.content), response);
  } finally {
    await close();
  }
});

test("secret.get posts the exact name and returns the value untouched", async () => {
  const calls: RecordedCall[] = [];
  const response = {
    secret_ref: "secret:019f8800000070008000000000000002",
    name: "datadog-api-key",
    value: "dd-canary-value",
    version: 3,
    updated_at: "2026-08-13T00:00:00Z",
  };
  const { client, close } = await connectedPair(calls, response);
  try {
    const result = await client.callTool({
      name: "secret.get",
      arguments: { name: "datadog-api-key" },
    });
    assert.notEqual(result.isError, true);
    assert.deepEqual(calls, [{
      url: "https://api.invalid/v1/workspace/secrets/get",
      method: "POST",
      body: JSON.stringify({ name: "datadog-api-key" }),
    }]);
    assert.deepEqual(parseToolText(result.content), response);
  } finally {
    await close();
  }
});

test("secret.list reads metadata with a bodiless GET", async () => {
  const calls: RecordedCall[] = [];
  const response = {
    secrets: [{
      secret_ref: "secret:019f8800000070008000000000000002",
      name: "datadog-api-key",
      description: "Datadog production API key",
      version: 3,
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-13T00:00:00Z",
      last_used_at: "2026-08-12T00:00:00Z",
    }],
  };
  const { client, close } = await connectedPair(calls, response);
  try {
    const result = await client.callTool({ name: "secret.list", arguments: {} });
    assert.notEqual(result.isError, true);
    assert.deepEqual(calls, [{
      url: "https://api.invalid/v1/workspace/secrets",
      method: "GET",
      body: undefined,
    }]);
    assert.deepEqual(parseToolText(result.content), response);
  } finally {
    await close();
  }
});

test("secret.delete posts the exact name", async () => {
  const calls: RecordedCall[] = [];
  const response = {
    secret_ref: "secret:019f8800000070008000000000000002",
    name: "datadog-api-key",
    status: "deleted",
  };
  const { client, close } = await connectedPair(calls, response);
  try {
    const result = await client.callTool({
      name: "secret.delete",
      arguments: { name: "datadog-api-key" },
    });
    assert.notEqual(result.isError, true);
    assert.deepEqual(calls, [{
      url: "https://api.invalid/v1/workspace/secrets/delete",
      method: "POST",
      body: JSON.stringify({ name: "datadog-api-key" }),
    }]);
    assert.deepEqual(parseToolText(result.content), response);
  } finally {
    await close();
  }
});

test("malformed secret names are rejected before any HTTP call", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls, { status: "ok" });
  try {
    const result = await client.callTool({
      name: "secret.get",
      arguments: { name: "has space" },
    });
    assert.equal(result.isError, true);
    assert.deepEqual(calls, []);
  } finally {
    await close();
  }
});

test("secret tool annotations separate reads from writes", async () => {
  const { client, close } = await connectedPair([], { status: "ok" });
  try {
    const tools = (await client.listTools()).tools;
    const byName = new Map(tools.map((tool) => [tool.name, tool]));
    assert.equal(byName.get("secret.get")?.annotations?.readOnlyHint, true);
    assert.equal(byName.get("secret.list")?.annotations?.readOnlyHint, true);
    assert.equal(byName.get("secret.put")?.annotations?.readOnlyHint, false);
    assert.equal(byName.get("secret.delete")?.annotations?.readOnlyHint, false);
    assert.equal(byName.get("secret.put")?.annotations?.destructiveHint, false);
    assert.equal(byName.get("secret.put")?.annotations?.openWorldHint, false);
  } finally {
    await close();
  }
});
