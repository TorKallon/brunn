import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, truncate, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { StraylightApiClient, StraylightApiError } from "./api-client.js";

test("API client binds credentials and optional evaluation headers", async () => {
  const calls: Array<{
    url: string;
    authorization: string | null;
    evalRun: string | null;
    evalCase: string | null;
    body: unknown;
  }> = [];
  const fakeFetch: typeof fetch = async (input, init) => {
    const headers = new Headers(init?.headers);
    calls.push({
      url: String(input),
      authorization: headers.get("authorization"),
      evalRun: headers.get("x-straylight-eval-run"),
      evalCase: headers.get("x-straylight-eval-case"),
      body: JSON.parse(String(init?.body)),
    });
    return new Response(JSON.stringify({ status: "complete", data: { session_id: "session:1" } }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  const client = new StraylightApiClient(
    "http://straylight.test/",
    "secret-token",
    fakeFetch,
    {
      "x-straylight-eval-run": "run-1",
      "x-straylight-eval-case": "case-1",
    },
  );

  const response = await client.request("/v1/memory/open", { task: "continue" });

  assert.equal(response.status, 200);
  assert.deepEqual(calls, [{
    url: "http://straylight.test/v1/memory/open",
    authorization: "Bearer secret-token",
    evalRun: "run-1",
    evalCase: "case-1",
    body: { task: "continue" },
  }]);
  assert.equal(response.elapsedMs >= 0, true);
});

test("API client exposes the paginated workspace change feed", async () => {
  const calls: string[] = [];
  const client = new StraylightApiClient(
    "http://straylight.test/",
    "read-token",
    async (input) => {
      calls.push(String(input));
      return new Response(JSON.stringify({
        status: "complete",
        data: {
          since_generation: 240,
          workspace_generation: 500,
          changes: [],
          truncated: true,
          next_generation: 440,
        },
      }), { status: 200 });
    },
  );

  const response = await client.workspaceChanges(240, 200);

  assert.equal(response.status, 200);
  assert.deepEqual(calls, [
    "http://straylight.test/v1/workspace/changes?since_generation=240&limit=200",
  ]);
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

test("API client fetches one exact historical version without returning bytes", async () => {
  const assetRoot = await mkdtemp(join(tmpdir(), "carrystate-client-assets-"));
  const assetRef = "entry:019f8505-da09-7d14-afd0-a9e27e47fdb7";
  const sessionId = "session:019f8505-fad9-7150-8f07-722426dab1db";
  const bytes = Buffer.from([0, 255, 100, 0, 33, 17, 42, 99]);
  const digest = createHash("sha256").update(bytes).digest("hex");
  const calls: Array<{
    url: string;
    authorization: string | null;
    hasDeadline: boolean;
  }> = [];
  const fakeFetch: typeof fetch = async (input, init) => {
    calls.push({
      url: String(input),
      authorization: new Headers(init?.headers).get("authorization"),
      hasDeadline: init?.signal instanceof AbortSignal,
    });
    if (calls.length === 1) {
      return new Response(JSON.stringify({
        entry_ref: assetRef,
        version: 3,
        content_hash: `sha256:${digest}`,
        size_bytes: bytes.byteLength,
        media_type: "application/octet-stream",
        path: "data/factory.sqlite",
      }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }
    return new Response(bytes, {
      status: 200,
      headers: {
        "content-length": String(bytes.byteLength),
        "content-type": "application/octet-stream",
        "x-carrystate-asset-ref": assetRef,
        "x-carrystate-asset-version": "3",
        "x-carrystate-sha256": digest,
      },
    });
  };
  const client = new StraylightApiClient(
    "http://straylight.test",
    "read-token",
    fakeFetch,
    {},
    assetRoot,
  );

  try {
    const response = await client.fetchAsset(assetRef, sessionId, 3);
    assert.equal(response.status, 200);
    assert.deepEqual(Object.keys(response.body).sort(), [
      "content_hash",
      "local_path",
      "media_type",
      "size_bytes",
    ]);
    const localPath = String(response.body.local_path);
    assert.deepEqual(await readFile(localPath), bytes);
    const rendered = JSON.stringify(response.body);
    assert.equal(rendered.includes(bytes.toString("base64")), false);
    assert.deepEqual(calls, [
      {
        url: `http://straylight.test/v1/workspace/binaries/${encodeURIComponent(assetRef)}`
          + "?version=3",
        authorization: "Bearer read-token",
        hasDeadline: true,
      },
      {
        url: `http://straylight.test/v1/workspace/binaries/${encodeURIComponent(assetRef)}`
          + "/content?version=3",
        authorization: "Bearer read-token",
        hasDeadline: true,
      },
    ]);
  } finally {
    await rm(assetRoot, { recursive: true, force: true });
  }
});

test("API client rejects metadata that does not match the requested version", async () => {
  const assetRef = "entry:019f8505-da09-7d14-afd0-a9e27e47fdb7";
  let fetchCalls = 0;
  let requestedUrl = "";
  const fakeFetch: typeof fetch = async (input) => {
    fetchCalls += 1;
    requestedUrl = String(input);
    return new Response(JSON.stringify({
      entry_ref: assetRef,
      version: 3,
      content_hash: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      size_bytes: 1,
      media_type: "application/octet-stream",
    }), { status: 200 });
  };
  const client = new StraylightApiClient("http://straylight.test", "read-token", fakeFetch);

  await assert.rejects(
    client.fetchAsset(assetRef, "session:1", 2),
    /returned version 3 for requested version 2/,
  );
  assert.equal(fetchCalls, 1);
  assert.equal(
    requestedUrl,
    `http://straylight.test/v1/workspace/binaries/${encodeURIComponent(assetRef)}?version=2`,
  );
});

test("ordinary API requests have a bounded deadline", async () => {
  const fakeFetch: typeof fetch = async (_input, init) => new Promise<Response>(
    (_resolve, reject) => {
      const signal = init?.signal;
      assert.ok(signal);
      const watchdog = setTimeout(
        () => reject(new Error("request deadline did not fire")),
        250,
      );
      const rejectWithReason = () => {
        clearTimeout(watchdog);
        reject(signal.reason);
      };
      if (signal.aborted) {
        rejectWithReason();
      } else {
        signal.addEventListener("abort", rejectWithReason, { once: true });
      }
    },
  );
  const client = new StraylightApiClient(
    "http://straylight.test",
    "read-token",
    fakeFetch,
    {},
    undefined,
    { requestMs: 10 },
  );

  await assert.rejects(
    client.request("/v1/status"),
    (error: unknown) => error instanceof DOMException
      && (error.name === "TimeoutError" || error.name === "AbortError"),
  );
});

test("asset fetch failures never echo upstream payloads", async () => {
  const assetRef = "entry:019f8505-da09-7d14-afd0-a9e27e47fdb7";
  const secret = Buffer.from("upstream binary payload").toString("base64");
  const metadataFailureClient = new StraylightApiClient(
    "http://straylight.test",
    "read-token",
    async () => new Response(JSON.stringify({
      error: { code: "upstream", message: secret },
      bytes: secret,
    }), { status: 500 }),
  );
  await assert.rejects(
    metadataFailureClient.fetchAsset(assetRef, "session:1"),
    (error: unknown) => {
      if (!(error instanceof StraylightApiError)) {
        return false;
      }
      assert.equal(JSON.stringify(error.body).includes(secret), false);
      assert.deepEqual(error.body, {
        error: {
          code: "asset_metadata_failed",
          message: "CarryState asset metadata request returned HTTP 500",
        },
      });
      return true;
    },
  );

  let calls = 0;
  const downloadFailureClient = new StraylightApiClient(
    "http://straylight.test",
    "read-token",
    async () => {
      calls += 1;
      if (calls === 1) {
        return new Response(JSON.stringify({
          entry_ref: assetRef,
          version: 1,
          content_hash: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
          size_bytes: 1,
          media_type: "application/octet-stream",
        }), { status: 200 });
      }
      return new Response(secret, { status: 502 });
    },
  );
  await assert.rejects(
    downloadFailureClient.fetchAsset(assetRef, "session:1"),
    (error: unknown) => {
      if (!(error instanceof StraylightApiError)) {
        return false;
      }
      assert.equal(JSON.stringify(error.body).includes(secret), false);
      assert.deepEqual(error.body, {
        error: {
          code: "asset_download_failed",
          message: "CarryState asset download request returned HTTP 502",
        },
      });
      return true;
    },
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

test("staging preserves nested logical paths and requests binary descriptions", async () => {
  const importRoot = await mkdtemp(join(tmpdir(), "straylight-mcp-paths-"));
  const previousImportRoot = process.env.STRAYLIGHT_MCP_IMPORT_ROOT;
  const nested = join(importRoot, "Trips", "Receipts");
  const filePath = join(nested, "scan.png");
  let form: FormData | undefined;
  let idempotencyKey: string | null = null;
  let hadDeadline = false;
  try {
    await mkdir(nested, { recursive: true });
    await writeFile(filePath, Buffer.from("fixture"));
    process.env.STRAYLIGHT_MCP_IMPORT_ROOT = importRoot;
    const client = new StraylightApiClient(
      "http://straylight.test",
      "write-token",
      async (_input, init) => {
        form = init?.body as FormData;
        idempotencyKey = new Headers(init?.headers).get("idempotency-key");
        hadDeadline = init?.signal instanceof AbortSignal;
        return new Response(JSON.stringify({ status: "complete", data: { entry_ref: "entry:1" } }), {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      },
    );

    await client.stage(
      "scope:primary",
      "vault:fixture",
      [{ path: "Trips/Receipts/scan.png", media_type: "image/png" }],
    );

    assert.ok(form);
    assert.match(String(form.get("limitations")), /immutable binary bytes/);
    assert.deepEqual(form.getAll("path"), ["Trips/Receipts/scan.png"]);
    assert.equal(form.get("media_type"), "image/png");
    assert.equal(
      form.get("expected_content_hash"),
      "sha256:f16d05ec6b29248d2c61adb1e9263f78e4f7bace1b955014a2d17872cfe4064d",
    );
    assert.equal((form.get("file") as File).name, "scan.png");
    const expectedIdempotencyKey = createHash("sha256")
      .update("vault:fixture")
      .update("\0")
      .update("scope:primary")
      .update("\0")
      .update("Trips/Receipts/scan.png")
      .digest("hex");
    assert.equal(idempotencyKey, `stage:${expectedIdempotencyKey}`);
    assert.equal(hadDeadline, true);
  } finally {
    if (previousImportRoot === undefined) {
      delete process.env.STRAYLIGHT_MCP_IMPORT_ROOT;
    } else {
      process.env.STRAYLIGHT_MCP_IMPORT_ROOT = previousImportRoot;
    }
    await rm(importRoot, { recursive: true, force: true });
  }
});
