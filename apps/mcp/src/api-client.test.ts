import assert from "node:assert/strict";
import { mkdtemp, rm, truncate, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { StraylightApiClient, StraylightApiError } from "./api-client.js";

test("API client binds the configured bearer token without persisting it", async () => {
  const calls: Array<{ url: string; authorization: string | null; body: unknown }> = [];
  const fakeFetch: typeof fetch = async (input, init) => {
    calls.push({
      url: String(input),
      authorization: new Headers(init?.headers).get("authorization"),
      body: JSON.parse(String(init?.body)),
    });
    return new Response(JSON.stringify({ status: "complete", data: { session_id: "session:1" } }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  const client = new StraylightApiClient("http://straylight.test/", "secret-token", fakeFetch);

  const response = await client.request("/v1/memory/open", { task: "continue" });

  assert.equal(response.status, 200);
  assert.deepEqual(calls, [{
    url: "http://straylight.test/v1/memory/open",
    authorization: "Bearer secret-token",
    body: { task: "continue" },
  }]);
});

test("API client preserves structured service failures", async () => {
  const fakeFetch: typeof fetch = async () => new Response(JSON.stringify({
    error: { code: "capability_denied", message: "checkpoint requires write access" },
  }), { status: 403 });
  const client = new StraylightApiClient("http://straylight.test", "read-only", fakeFetch);

  await assert.rejects(
    client.request("/v1/memory/checkpoint", {}),
    (error: unknown) => error instanceof StraylightApiError
      && error.status === 403
      && (error.body.error as { code: string }).code === "capability_denied",
  );
});

test("staging rejects oversized files before reading or sending them", async () => {
  const importRoot = await mkdtemp(join(tmpdir(), "straylight-mcp-stage-"));
  const oversizedPath = join(importRoot, "oversized.bin");
  const previousImportRoot = process.env.STRAYLIGHT_MCP_IMPORT_ROOT;
  let fetchCalls = 0;

  try {
    await writeFile(oversizedPath, "");
    await truncate(oversizedPath, (64 * 1024 * 1024) + 1);
    process.env.STRAYLIGHT_MCP_IMPORT_ROOT = importRoot;
    const fakeFetch: typeof fetch = async () => {
      fetchCalls += 1;
      throw new Error("fetch must not run for an invalid stage request");
    };
    const client = new StraylightApiClient("http://straylight.test", "write-token", fakeFetch);

    await assert.rejects(
      client.stage("scope:primary", undefined, [{ path: "oversized.bin" }]),
      /staged files are limited to 67108864 bytes each/,
    );
    assert.equal(fetchCalls, 0);
  } finally {
    if (previousImportRoot === undefined) {
      delete process.env.STRAYLIGHT_MCP_IMPORT_ROOT;
    } else {
      process.env.STRAYLIGHT_MCP_IMPORT_ROOT = previousImportRoot;
    }
    await rm(importRoot, { recursive: true, force: true });
  }
});
