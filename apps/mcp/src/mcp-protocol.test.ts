import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
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
    "asset.fetch",
    "asset.list",
    "asset.metadata",
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
  const open = response.tools.find((tool) => tool.name === "memory.open");
  assert.ok(open);
  const resumeCheckpointRef = open.inputSchema.properties?.resume_checkpoint_ref as {
    description?: string;
  } | undefined;
  assert.match(resumeCheckpointRef?.description ?? "", /Omit this field/);
  assert.match(resumeCheckpointRef?.description ?? "", /never invent/);
  const query = response.tools.find((tool) => tool.name === "memory.query");
  assert.ok(query);
  assert.match(query.description ?? "", /Exact mode accepts record references/);
  const queries = query.inputSchema.properties?.queries as {
    items?: {
      properties?: {
        limit?: { default?: number };
        modes?: { description?: string };
        where?: {
          properties?: Record<string, unknown>;
          additionalProperties?: boolean;
        };
      };
    };
  } | undefined;
  assert.equal(queries?.items?.properties?.limit?.default, 8);
  assert.match(queries?.items?.properties?.modes?.description ?? "", /Read a known file path/);
  const where = queries?.items?.properties?.where as {
    properties?: Record<string, { description?: string }>;
    additionalProperties?: boolean;
  } | undefined;
  assert.deepEqual(Object.keys(where?.properties ?? {}).sort(), [
    "authority",
    "canonicality",
    "predicate",
    "record_kind",
    "scope_root",
    "type_profile",
  ]);
  assert.equal(where?.additionalProperties, false);
  assert.match(
    where?.properties?.scope_root?.description ?? "",
    /not the authorization scope/i,
  );
  const read = response.tools.find((tool) => tool.name === "memory.read");
  assert.ok(read);
  assert.match(read.description ?? "", /verbatim/);
  assert.match(read.description ?? "", /never infer/);
  const requests = read.inputSchema.properties?.requests as {
    items?: {
      properties?: {
        before?: unknown;
        after?: unknown;
        ref?: { description?: string };
        path?: { description?: string };
      };
    };
  } | undefined;
  assert.ok(requests?.items?.properties?.before);
  assert.ok(requests?.items?.properties?.after);
  assert.match(requests?.items?.properties?.ref?.description ?? "", /verbatim/);
  assert.match(requests?.items?.properties?.path?.description ?? "", /Never synthesize/);
  const compute = response.tools.find((tool) => tool.name === "memory.compute");
  assert.ok(compute);
  const steps = compute.inputSchema.properties?.steps as {
    items?: { properties?: { op?: { enum?: string[] } } };
  } | undefined;
  assert.equal(steps?.items?.properties?.op?.enum?.includes("arithmetic.evaluate"), false);
  assert.equal(steps?.items?.properties?.op?.enum?.includes("gateRollup"), true);
  const checkpoint = response.tools.find((tool) => tool.name === "memory.checkpoint");
  assert.ok(checkpoint);
  const sourceRefs = checkpoint.inputSchema.properties?.source_refs as {
    items?: { description?: string };
  } | undefined;
  assert.match(sourceRefs?.items?.description ?? "", /evidence:/);
  assert.match(sourceRefs?.items?.description ?? "", /source_episode:/);
  const save = response.tools.find((tool) => tool.name === "memory.save");
  assert.ok(save);
  const saveSourceRefs = save.inputSchema.properties?.source_refs as {
    items?: {
      properties?: {
        span?: {
          items?: unknown;
          minItems?: number;
          maxItems?: number;
        };
      };
    };
  } | undefined;
  const saveSpan = saveSourceRefs?.items?.properties?.span;
  assert.equal(Array.isArray(saveSpan?.items), false);
  assert.equal(saveSpan?.minItems, 2);
  assert.equal(saveSpan?.maxItems, 2);
  const assetList = response.tools.find((tool) => tool.name === "asset.list");
  assert.ok(assetList);
  assert.deepEqual(
    [...(assetList.inputSchema.required ?? [])].sort(),
    ["session_id"],
  );
  assert.equal(
    (assetList.inputSchema.properties?.offset as { default?: number } | undefined)
      ?.default,
    0,
  );
  assert.equal(
    (assetList.inputSchema.properties?.limit as { default?: number } | undefined)
      ?.default,
    100,
  );
  const assetMetadata = response.tools.find((tool) => tool.name === "asset.metadata");
  assert.ok(assetMetadata);
  assert.deepEqual(
    [...(assetMetadata.inputSchema.required ?? [])].sort(),
    ["asset_ref", "session_id"],
  );
  const assetFetch = response.tools.find((tool) => tool.name === "asset.fetch");
  assert.ok(assetFetch);
  assert.match(assetFetch.description ?? "", /bytes and base64 are never returned/);
  assert.deepEqual(
    [...(assetFetch.inputSchema.required ?? [])].sort(),
    ["asset_ref", "session_id"],
  );

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

test("asset MCP tools return metadata or a verified local path, never payload bytes", async () => {
  const assetRef = "asset:019f8530-e5f6-77d3-a373-052ee8cd24bd";
  const sessionId = "session:019f8531-06fa-7fe0-9050-0648d7e8553e";
  const bytes = Buffer.from("literal receipt payload that must stay outside model context");
  const base64 = bytes.toString("base64");
  const digest = createHash("sha256").update(bytes).digest("hex");
  const assetRoot = await mkdtemp(join(tmpdir(), "carrystate-mcp-protocol-assets-"));
  const requests: string[] = [];
  const httpServer = createServer((request, response) => {
    const requestUrl = new URL(request.url ?? "/", "http://127.0.0.1");
    const requestPath = decodeURIComponent(requestUrl.pathname);
    requests.push(requestUrl.pathname + requestUrl.search);
    if (
      requestPath === `/v1/assets/${assetRef}`
      && requestUrl.searchParams.get("session_id") === sessionId
    ) {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({
        asset_ref: assetRef,
        version: 2,
        content_hash: `sha256:${digest}`,
        size_bytes: bytes.byteLength,
        media_type: "image/jpeg",
        path: "Trips/receipt.jpg",
      }));
      return;
    }
    if (
      requestPath === `/v1/assets/${assetRef}/versions/2/content`
      && requestUrl.searchParams.get("session_id") === sessionId
    ) {
      response.writeHead(200, {
        "content-length": String(bytes.byteLength),
        "content-type": "image/jpeg",
        "x-carrystate-asset-ref": assetRef,
        "x-carrystate-asset-version": "2",
        "x-carrystate-sha256": digest,
      });
      response.write(bytes.subarray(0, 11));
      response.end(bytes.subarray(11));
      return;
    }
    response.writeHead(404, { "content-type": "application/json" });
    response.end(JSON.stringify({ error: { code: "not_found", message: "not found" } }));
  });
  await new Promise<void>((resolve, reject) => {
    httpServer.once("error", reject);
    httpServer.listen(0, "127.0.0.1", () => resolve());
  });
  const address = httpServer.address();
  assert.ok(address && typeof address === "object");

  const environment = Object.fromEntries(
    Object.entries(process.env).filter((entry): entry is [string, string] => entry[1] !== undefined),
  );
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [fileURLToPath(new URL("./index.js", import.meta.url))],
    env: {
      ...environment,
      STRAYLIGHT_API_TOKEN: "protocol-test-token",
      STRAYLIGHT_API_URL: `http://127.0.0.1:${address.port}`,
      CARRYSTATE_MCP_ASSET_ROOT: assetRoot,
    },
  });
  const client = new Client({ name: "carrystate-asset-test", version: "0.1.0" });

  try {
    await client.connect(transport);
    const metadataCall = await client.callTool({
      name: "asset.metadata",
      arguments: { asset_ref: assetRef, session_id: sessionId },
    });
    assert.equal(metadataCall.isError, undefined);
    const metadata = parseToolText(metadataCall.content);
    assert.equal(metadata.asset_ref, assetRef);
    assert.equal(metadata.content_hash, `sha256:${digest}`);

    const fetchCall = await client.callTool({
      name: "asset.fetch",
      arguments: { asset_ref: assetRef, session_id: sessionId, version: 2 },
    });
    assert.equal(fetchCall.isError, undefined);
    assert.equal(fetchCall.structuredContent, undefined);
    const fetched = parseToolText(fetchCall.content);
    assert.deepEqual(Object.keys(fetched).sort(), [
      "content_hash",
      "local_path",
      "media_type",
      "size_bytes",
    ]);
    assert.equal(fetched.content_hash, `sha256:${digest}`);
    assert.equal(fetched.size_bytes, bytes.byteLength);
    assert.equal(fetched.media_type, "image/jpeg");
    assert.deepEqual(await readFile(String(fetched.local_path)), bytes);
    const rendered = JSON.stringify(fetchCall);
    assert.equal(rendered.includes(bytes.toString()), false);
    assert.equal(rendered.includes(base64), false);
    assert.deepEqual(requests, [
      `/v1/assets/${encodeURIComponent(assetRef)}?session_id=${encodeURIComponent(sessionId)}`,
      `/v1/assets/${encodeURIComponent(assetRef)}?session_id=${encodeURIComponent(sessionId)}`,
      `/v1/assets/${encodeURIComponent(assetRef)}/versions/2/content`
        + `?session_id=${encodeURIComponent(sessionId)}`,
    ]);
  } finally {
    await client.close().catch(() => undefined);
    await new Promise<void>((resolve) => httpServer.close(() => resolve()));
    await rm(assetRoot, { recursive: true, force: true });
  }
});

function parseToolText(content: unknown): Record<string, unknown> {
  assert.ok(Array.isArray(content));
  const first = content[0] as { type?: string; text?: string } | undefined;
  assert.equal(first?.type, "text");
  if (typeof first?.text !== "string") {
    throw new Error("MCP tool response did not contain text");
  }
  return JSON.parse(first.text) as Record<string, unknown>;
}
