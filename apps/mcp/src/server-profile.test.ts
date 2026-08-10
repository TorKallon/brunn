import assert from "node:assert/strict";
import test from "node:test";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";

import { StraylightApiClient } from "./api-client.js";
import { createStraylightMcpServer } from "./index.js";

test("remote profile exposes only hosted-safe tools with bounded reads", async () => {
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const apiClient = new StraylightApiClient(
    "https://api.invalid",
    "test-token",
    async () => new Response(JSON.stringify({ status: "ok" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    }),
  );
  const server = createStraylightMcpServer(apiClient, {
    surface: "remote",
    includeStructuredContent: true,
  });
  const client = new Client({ name: "remote-profile-test", version: "0.1.0" });

  try {
    await server.connect(serverTransport);
    await client.connect(clientTransport);
    assert.match(client.getInstructions() ?? "", /Start substantive work with memory\.open/);
    assert.match(client.getInstructions() ?? "", /memory\.checkpoint/);
    const response = await client.listTools();
    assert.deepEqual(response.tools.map((tool) => tool.name).sort(), [
      "asset.list",
      "asset.metadata",
      "briefing.dedupe",
      "briefing.publish",
      "briefing.topics",
      "memory.capture",
      "memory.changes",
      "memory.checkpoint",
      "memory.open",
      "memory.query",
      "memory.read",
      "memory.status",
      "memory.write",
      "notification.publish",
    ]);
    assert.equal(response.tools.some((tool) => tool.name === "memory.stage"), false);
    assert.equal(response.tools.some((tool) => tool.name === "asset.fetch"), false);

    const read = response.tools.find((tool) => tool.name === "memory.read");
    assert.ok(read);
    const requests = read.inputSchema.properties?.requests as {
      items?: { properties?: { max_chars?: { maximum?: number } } };
    } | undefined;
    assert.equal(requests?.items?.properties?.max_chars?.maximum, 120_000);

    const write = response.tools.find((tool) => tool.name === "memory.write");
    const query = response.tools.find((tool) => tool.name === "memory.query");
    const checkpoint = response.tools.find((tool) => tool.name === "memory.checkpoint");
    assert.equal(write?.annotations?.readOnlyHint, false);
    assert.equal(write?.annotations?.destructiveHint, false);
    assert.equal(checkpoint?.annotations?.readOnlyHint, false);
    assert.equal(checkpoint?.annotations?.idempotentHint, true);
    assert.equal(query?.annotations?.readOnlyHint, true);
    assert.equal(query?.annotations?.openWorldHint, false);
  } finally {
    await client.close().catch(() => undefined);
    await server.close().catch(() => undefined);
  }
});
