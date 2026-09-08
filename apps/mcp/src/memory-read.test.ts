import assert from "node:assert/strict";
import test from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { BrunnApiClient } from "./api-client.js";
import { createBrunnMcpServer } from "./index.js";

async function fixture() {
  const calls: Array<{ url: string; body: unknown }> = [];
  const response = {
    status: "complete",
    data: { items: [{ reference: "entry:019f8800-0000-7000-8000-000000000001", path: "Renamed/Source.md", version: 4, version_ref: "entry-version:019f8800-0000-7000-8000-000000000004", content_hash: "sha256:exact", view: "full", text: "Historical source content.", representation: "complete_source", freshness: { status: "historical", current_version: 6 } }] },
  };
  const api = new BrunnApiClient("https://api.invalid", "fixture-token", async (url, init) => {
    calls.push({ url: String(url), body: JSON.parse(String(init?.body)) });
    return new Response(JSON.stringify(response), { status: 200, headers: { "content-type": "application/json" } });
  });
  const server = createBrunnMcpServer(api, { surface: "remote", includeStructuredContent: true });
  const client = new Client({ name: "historical-read-test", version: "1" });
  const [a, b] = InMemoryTransport.createLinkedPair();
  await server.connect(b);
  await client.connect(a);
  return { client, calls, response, close: async () => { await client.close(); await server.close(); } };
}

test("hosted memory.read schema exposes positive historical versions by tool name", async () => {
  const { client, close } = await fixture();
  try {
    const tool = (await client.listTools()).tools.find((item) => item.name === "memory.read");
    assert.ok(tool);
    const requests = tool.inputSchema.properties?.requests as { items?: { properties?: { version?: { type?: string; exclusiveMinimum?: number; minimum?: number; description?: string } } } };
    const version = requests.items?.properties?.version;
    assert.equal(version?.type, "integer");
    assert.ok(version?.exclusiveMinimum === 0 || version?.minimum === 1);
    assert.match(version?.description ?? "", /historical version/);
  } finally { await close(); }
});

test("hosted memory.read forwards exact reference and path versions without replacing historical metadata", async () => {
  const { client, calls, response, close } = await fixture();
  try {
    const input = { session_id: "session:fixture", requests: [{ ref: "entry:019f8800-0000-7000-8000-000000000001", version: 4, view: "full" }, { path: "Old/Source.md", version: 2, view: "range", start: 3, end: 8 }] };
    const result = await client.callTool({ name: "memory.read", arguments: input });
    assert.notEqual(result.isError, true);
    assert.deepEqual(calls, [{ url: "https://api.invalid/v1/workspace/read", body: input }]);
    assert.deepEqual(result.structuredContent, response);
  } finally { await close(); }
});

test("hosted memory.read refuses invalid versions and exact-version current_truth before HTTP", async () => {
  const { client, calls, close } = await fixture();
  try {
    for (const version of [0, -1, 1.5]) {
      const result = await client.callTool({ name: "memory.read", arguments: { session_id: "session:fixture", requests: [{ path: "Source.md", version }] } });
      assert.equal(result.isError, true);
    }
    const result = await client.callTool({ name: "memory.read", arguments: { session_id: "session:fixture", requests: [{ path: "Source.md", version: 2, view: "current_truth" }] } });
    assert.equal(result.isError, true);
    assert.deepEqual(calls, []);
  } finally { await close(); }
});
