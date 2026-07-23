import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

test("stdio server negotiates and exposes the complete typed memory surface", async (context) => {
  const environment = Object.fromEntries(
    Object.entries(process.env).filter((entry): entry is [string, string] => entry[1] !== undefined),
  );
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [fileURLToPath(new URL("./index.js", import.meta.url))],
    env: {
      ...environment,
      STRAYLIGHT_API_TOKEN: "protocol-test-token",
      STRAYLIGHT_API_URL: "http://127.0.0.1:1",
    },
  });
  const client = new Client({ name: "straylight-adapter-test", version: "0.1.0" });
  context.after(async () => {
    await client.close();
  });

  await client.connect(transport);
  const response = await client.listTools();

  const names = response.tools.map((tool) => tool.name).sort();
  assert.deepEqual(names, [
    "memory.capture",
    "memory.checkpoint",
    "memory.compute",
    "memory.open",
    "memory.query",
    "memory.read",
    "memory.save",
    "memory.stage",
    "memory.status",
    "memory.verify",
  ]);
  assert.equal(response.tools.every((tool) => tool.inputSchema.type === "object"), true);
  const query = response.tools.find((tool) => tool.name === "memory.query");
  assert.ok(query);
  const queries = query.inputSchema.properties?.queries as {
    items?: { properties?: { limit?: { default?: number } } };
  } | undefined;
  assert.equal(queries?.items?.properties?.limit?.default, 8);
  const read = response.tools.find((tool) => tool.name === "memory.read");
  assert.ok(read);
  const requests = read.inputSchema.properties?.requests as {
    items?: { properties?: Record<string, unknown> };
  } | undefined;
  assert.ok(requests?.items?.properties?.before);
  assert.ok(requests?.items?.properties?.after);

  const call = await client.callTool({ name: "memory.status", arguments: {} });
  assert.equal(call.isError, true);
  assert.equal(call.structuredContent, undefined);
  assert.equal(Array.isArray(call.content), true);
  const text = (call.content as Array<{ type: string; text?: string }>)[0];
  assert.equal(text?.type, "text");
  if (text?.type === "text" && text.text) {
    assert.equal(JSON.parse(text.text).error.code, "adapter_error");
  }
});
