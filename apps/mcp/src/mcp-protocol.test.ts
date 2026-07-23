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
});
