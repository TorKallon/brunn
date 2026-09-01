import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, truncate, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { BrunnApiClient, BrunnApiError } from "./api-client.js";

const ONE_FAST_RETRY = { retryBackoffMs: [0] as const };
const EXHAUST_FAST_RETRIES = { retryBackoffMs: [0, 0, 0, 0, 0, 0] as const };

test("retry schedule configuration fails closed on malformed or excessive overrides", () => {
  const original = process.env.BRUNN_MCP_RETRY_BACKOFF_MS;
  try {
    for (const value of ["1,,2", "1,2,3,4,5,6,7", "-1,2"]) {
      process.env.BRUNN_MCP_RETRY_BACKOFF_MS = value;
      assert.throws(
        () => new BrunnApiClient("http://brunn.test", "read-token"),
        /BRUNN_MCP_RETRY_BACKOFF_MS/,
      );
    }
  } finally {
    if (original === undefined) {
      delete process.env.BRUNN_MCP_RETRY_BACKOFF_MS;
    } else {
      process.env.BRUNN_MCP_RETRY_BACKOFF_MS = original;
    }
  }
});

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
      evalRun: headers.get("x-brunn-eval-run"),
      evalCase: headers.get("x-brunn-eval-case"),
      body: JSON.parse(String(init?.body)),
    });
    return new Response(JSON.stringify({ status: "complete", data: { session_id: "session:1" } }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  const client = new BrunnApiClient(
    "http://brunn.test/",
    "secret-token",
    fakeFetch,
    {
      "x-brunn-eval-run": "run-1",
      "x-brunn-eval-case": "case-1",
    },
  );

  const response = await client.request("/v1/memory/open", { task: "continue" });

  assert.equal(response.status, 200);
  assert.deepEqual(calls, [{
    url: "http://brunn.test/v1/memory/open",
    authorization: "Bearer secret-token",
    evalRun: "run-1",
    evalCase: "case-1",
    body: { task: "continue" },
  }]);
  assert.equal(response.elapsedMs >= 0, true);
});

test("API client exposes the paginated workspace change feed", async () => {
  const calls: string[] = [];
  const client = new BrunnApiClient(
    "http://brunn.test/",
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
    "http://brunn.test/v1/workspace/changes?since_generation=240&limit=200",
  ]);
});

test("API client preserves structured service failures", async () => {
  const fakeFetch: typeof fetch = async () => new Response(JSON.stringify({
    error: { code: "capability_denied", message: "checkpoint requires write access" },
  }), { status: 403 });
  const client = new BrunnApiClient("http://brunn.test", "read-only", fakeFetch);

  await assert.rejects(
    client.request("/v1/memory/checkpoint", {}),
    (error: unknown) => error instanceof BrunnApiError
      && error.status === 403
      && (error.body.error as { code: string }).code === "capability_denied",
  );
});

test("idempotent checkpoint recovers a commit followed by lost 502 response", async () => {
  const payload = {
    session_id: "session:1",
    idempotency_key: "checkpoint:stable-1",
    state: { objective: "finish the durable handoff" },
    source_refs: ["sources/Work.md"],
  };
  const bodies: string[] = [];
  let durableCommits = 0;
  const committedKeys = new Set<string>();
  const client = new BrunnApiClient(
    "http://brunn.test",
    "write-token",
    async (_input, init) => {
      const serialized = String(init?.body);
      bodies.push(serialized);
      const request = JSON.parse(serialized) as { idempotency_key: string };
      if (!committedKeys.has(request.idempotency_key)) {
        committedKeys.add(request.idempotency_key);
        durableCommits += 1;
        return new Response(JSON.stringify({
          request_id: "request:lost-response",
          upstream_detail: "must not escape",
        }), { status: 502 });
      }
      return new Response(JSON.stringify({
        request_id: "request:recovered-receipt",
        status: "no_op",
        data: { checkpoint_id: "checkpoint:stable", no_op: true },
      }), { status: 200 });
    },
    {},
    undefined,
    ONE_FAST_RETRY,
  );

  const response = await client.request("/v1/workspace/checkpoint", payload);

  assert.equal(response.status, 200);
  assert.equal(response.body.request_id, "request:recovered-receipt");
  assert.equal(durableCommits, 1);
  assert.equal(bodies.length, 2);
  assert.equal(bodies[0], bodies[1]);
  assert.deepEqual(JSON.parse(bodies[1] ?? "{}"), payload);
});

test("read requests recover from connection resets", async () => {
  let calls = 0;
  const client = new BrunnApiClient(
    "http://brunn.test",
    "read-token",
    async () => {
      calls += 1;
      if (calls === 1) {
        throw Object.assign(new Error("socket reset after headers"), { code: "ECONNRESET" });
      }
      return new Response(JSON.stringify({
        request_id: "request:after-reset",
        status: "complete",
      }), { status: 200 });
    },
    {},
    undefined,
    ONE_FAST_RETRY,
  );

  const response = await client.request("/v1/status");

  assert.equal(calls, 2);
  assert.equal(response.body.request_id, "request:after-reset");
});

test("plain Railway Application not found responses are transient", async () => {
  let calls = 0;
  const client = new BrunnApiClient(
    "http://brunn.test",
    "read-token",
    async () => {
      calls += 1;
      if (calls === 1) {
        return new Response("Application not found\n", { status: 404 });
      }
      return new Response(JSON.stringify({ status: "complete", request_id: "request:ready" }), {
        status: 200,
      });
    },
    {},
    undefined,
    ONE_FAST_RETRY,
  );

  const response = await client.request("/v1/workspace/open", { task: "continue" });

  assert.equal(calls, 2);
  assert.equal(response.body.request_id, "request:ready");
});

test("Railway Application not found recovers beyond the legacy three-attempt window", async () => {
  let calls = 0;
  const bodies: string[] = [];
  const client = new BrunnApiClient(
    "http://brunn.test",
    "read-token",
    async (_input, init) => {
      calls += 1;
      bodies.push(String(init?.body));
      if (calls <= 4) {
        return new Response("Application not found\n", { status: 404 });
      }
      return new Response(JSON.stringify({ status: "complete", request_id: "request:ready" }), {
        status: 200,
      });
    },
    {},
    undefined,
    EXHAUST_FAST_RETRIES,
  );

  const response = await client.request("/v1/workspace/open", { task: "continue" });

  assert.equal(calls, 5);
  assert.equal(response.body.request_id, "request:ready");
  assert.equal(new Set(bodies).size, 1);
});

test("capture and write retry only with an idempotency key", async () => {
  for (const fixture of [
    {
      path: "/v1/workspace/capture",
      body: { content: "evidence", source: { title: "Evidence" } },
    },
    {
      path: "/v1/workspace/write",
      body: { path: "Work.md", content: "state" },
    },
  ]) {
    let keyedCalls = 0;
    const keyed = new BrunnApiClient(
      "http://brunn.test",
      "write-token",
      async () => {
        keyedCalls += 1;
        return keyedCalls === 1
          ? new Response("gateway unavailable", { status: 503 })
          : new Response(JSON.stringify({ status: "complete" }), { status: 200 });
      },
      {},
      undefined,
      ONE_FAST_RETRY,
    );
    await keyed.request(fixture.path, {
      ...fixture.body,
      idempotency_key: `stable:${fixture.path}`,
    });
    assert.equal(keyedCalls, 2);

    let unkeyedCalls = 0;
    const unkeyed = new BrunnApiClient(
      "http://brunn.test",
      "write-token",
      async () => {
        unkeyedCalls += 1;
        return new Response(JSON.stringify({
          request_id: `request:unkeyed-${unkeyedCalls}`,
          private_upstream_payload: "do not reveal",
        }), { status: 502 });
      },
    );
    await assert.rejects(
      unkeyed.request(fixture.path, fixture.body),
      (error: unknown) => {
        if (!(error instanceof BrunnApiError)) {
          return false;
        }
        const detail = error.body.error as Record<string, unknown>;
        assert.equal(detail.code, "ambiguous_outcome");
        assert.equal(detail.retryable, false);
        assert.equal(detail.attempts, 1);
        assert.equal(error.body.request_id, "request:unkeyed-1");
        assert.equal(JSON.stringify(error.body).includes("do not reveal"), false);
        return true;
      },
    );
    assert.equal(unkeyedCalls, 1);
  }
});

test("notification event identity makes transient publication retry-safe", async () => {
  let calls = 0;
  const bodies: string[] = [];
  const client = new BrunnApiClient(
    "http://brunn.test",
    "write-token",
    async (_input, init) => {
      calls += 1;
      bodies.push(String(init?.body));
      return calls === 1
        ? new Response("temporary gateway", { status: 504 })
        : new Response(JSON.stringify({ status: "complete", request_id: "request:notified" }), {
            status: 200,
        });
    },
    {},
    undefined,
    ONE_FAST_RETRY,
  );
  const payload = {
    event_key: "incident:stable-event",
    correlation_id: "incident:1",
    kind: "operational",
    importance: "important",
    title: "Recovered",
    body: "The service recovered.",
    target: { type: "notification" },
  };

  const response = await client.request("/v1/workspace/notifications/publish", payload);

  assert.equal(calls, 2);
  assert.equal(response.body.request_id, "request:notified");
  assert.equal(bodies[0], bodies[1]);
});

test("exhausted idempotent mutation returns sanitized ambiguous outcome", async () => {
  let calls = 0;
  const secret = "private Railway response payload";
  const client = new BrunnApiClient(
    "http://brunn.test",
    "write-token",
    async () => {
      calls += 1;
      return new Response(JSON.stringify({
        request_id: `request:attempt-${calls}`,
        error: { message: secret },
      }), { status: 503 });
    },
    {},
    undefined,
    EXHAUST_FAST_RETRIES,
  );

  await assert.rejects(
    client.request("/v1/workspace/checkpoint", {
      session_id: "session:1",
      idempotency_key: "checkpoint:exhausted",
      state: { objective: "persist state" },
    }),
    (error: unknown) => {
      if (!(error instanceof BrunnApiError)) {
        return false;
      }
      const detail = error.body.error as Record<string, unknown>;
      assert.equal(detail.code, "ambiguous_outcome");
      assert.equal(detail.outcome, "unknown");
      assert.equal(detail.retryable, true);
      assert.equal(detail.attempts, 7);
      assert.match(String(detail.message), /identical request/);
      assert.match(String(detail.message), /identical idempotency key or event identity/);
      assert.equal(error.body.request_id, "request:attempt-7");
      assert.equal(JSON.stringify(error.body).includes(secret), false);
      return true;
    },
  );
  assert.equal(calls, 7);
});

test("exhausted reads return sanitized upstream unavailable errors", async () => {
  let calls = 0;
  const secret = "private upstream diagnostic payload";
  const client = new BrunnApiClient(
    "http://brunn.test",
    "read-token",
    async () => {
      calls += 1;
      return new Response(JSON.stringify({
        diagnostic: secret,
      }), {
        status: 504,
        headers: { "x-request-id": `request:read-${calls}` },
      });
    },
    {},
    undefined,
    EXHAUST_FAST_RETRIES,
  );

  await assert.rejects(
    client.request("/v1/workspace/search", {
      session_id: "session:1",
      queries: [{ query: "stable service" }],
    }),
    (error: unknown) => {
      if (!(error instanceof BrunnApiError)) {
        return false;
      }
      const detail = error.body.error as Record<string, unknown>;
      assert.equal(detail.code, "upstream_unavailable");
      assert.equal(detail.retryable, true);
      assert.equal(detail.attempts, 7);
      assert.equal(error.body.request_id, "request:read-7");
      assert.equal(JSON.stringify(error.body).includes(secret), false);
      return true;
    },
  );
  assert.equal(calls, 7);
});

test("oversized declared JSON responses are rejected before their body is read", async () => {
  let calls = 0;
  const secret = "declared oversized private response";
  const client = new BrunnApiClient(
    "http://brunn.test",
    "write-token",
    async () => {
      calls += 1;
      return new Response(secret, {
        status: 200,
        headers: { "content-length": String(32 * 1024 * 1024 + 1) },
      });
    },
  );

  await assert.rejects(
    client.request("/v1/workspace/unknown-mutation", { value: "unsafe" }),
    (error: unknown) => {
      if (!(error instanceof BrunnApiError)) {
        return false;
      }
      assert.equal((error.body.error as Record<string, unknown>).code, "ambiguous_outcome");
      assert.equal(JSON.stringify(error.body).includes(secret), false);
      return true;
    },
  );
  assert.equal(calls, 1);
});

test("oversized chunked JSON responses are cancelled at the byte boundary", async () => {
  let calls = 0;
  let cancelled = false;
  const chunk = new Uint8Array(1024 * 1024);
  const client = new BrunnApiClient(
    "http://brunn.test",
    "write-token",
    async () => {
      calls += 1;
      return new Response(new ReadableStream<Uint8Array>({
        pull(controller) {
          controller.enqueue(chunk);
        },
        cancel() {
          cancelled = true;
        },
      }), { status: 200 });
    },
  );

  await assert.rejects(
    client.request("/v1/workspace/unknown-mutation", { value: "unsafe" }),
    (error: unknown) => error instanceof BrunnApiError
      && (error.body.error as Record<string, unknown>).code === "ambiguous_outcome",
  );
  assert.equal(calls, 1);
  assert.equal(cancelled, true);
});

test("malformed successful and nontransient responses never expose upstream text", async () => {
  for (const status of [200, 400, 500]) {
    let calls = 0;
    const secret = `private malformed response ${status}`;
    const client = new BrunnApiClient(
      "http://brunn.test",
      "read-token",
      async () => {
        calls += 1;
        return new Response(secret, {
          status,
          headers: { "x-request-id": `request:malformed-${status}-${calls}` },
        });
      },
      {},
      undefined,
      EXHAUST_FAST_RETRIES,
    );

    await assert.rejects(
      client.request("/v1/status"),
      (error: unknown) => {
        if (!(error instanceof BrunnApiError)) {
          return false;
        }
        const detail = error.body.error as Record<string, unknown>;
        assert.equal(
          detail.code,
          status === 200 ? "upstream_unavailable" : "invalid_upstream_response",
        );
        assert.equal(detail.attempts, status === 200 ? 7 : undefined);
        assert.equal(
          error.body.request_id,
          `request:malformed-${status}-${status === 200 ? 7 : 1}`,
        );
        assert.equal(JSON.stringify(error.body).includes(secret), false);
        return true;
      },
    );
    assert.equal(calls, status === 200 ? 7 : 1);
  }
});

test("an empty successful response cannot masquerade as a JSON envelope", async () => {
  let calls = 0;
  const client = new BrunnApiClient(
    "http://brunn.test",
    "read-token",
    async () => {
      calls += 1;
      return new Response(null, {
        status: 200,
        headers: { "x-request-id": `request:empty-${calls}` },
      });
    },
    {},
    undefined,
    EXHAUST_FAST_RETRIES,
  );

  await assert.rejects(
    client.request("/v1/status"),
    (error: unknown) => error instanceof BrunnApiError
      && (error.body.error as Record<string, unknown>).code === "upstream_unavailable"
      && error.body.request_id === "request:empty-7",
  );
  assert.equal(calls, 7);
});

test("mixed transient attempts retain the last observed request ID", async () => {
  let calls = 0;
  const client = new BrunnApiClient(
    "http://brunn.test",
    "read-token",
    async () => {
      calls += 1;
      if (calls === 1) {
        return new Response("temporary", {
          status: 503,
          headers: { "x-request-id": "request:first-observed" },
        });
      }
      throw Object.assign(new Error("connection reset"), { code: "ECONNRESET" });
    },
    {},
    undefined,
    EXHAUST_FAST_RETRIES,
  );

  await assert.rejects(
    client.request("/v1/status"),
    (error: unknown) => error instanceof BrunnApiError
      && error.body.request_id === "request:first-observed",
  );
  assert.equal(calls, 7);
});

test("a stalled response body is deadline-bounded, aborts fetch, and retains header identity", async () => {
  let requestSignal: AbortSignal | undefined;
  const client = new BrunnApiClient(
    "http://brunn.test",
    "read-token",
    async (_input, init) => {
      requestSignal = init?.signal ?? undefined;
      return new Response(new ReadableStream<Uint8Array>({
        pull() {
          return new Promise<void>(() => undefined);
        },
      }), {
        status: 200,
        headers: { "x-request-id": "request:headers-before-stall" },
      });
    },
    {},
    undefined,
    { requestMs: 15 },
  );

  const started = performance.now();
  await assert.rejects(
    client.request("/v1/status"),
    (error: unknown) => error instanceof BrunnApiError
      && error.body.request_id === "request:headers-before-stall",
  );
  assert.equal(performance.now() - started < 200, true);
  assert.equal(requestSignal?.aborted, true);
});

test("the absolute deadline includes retry backoff and aborts the final attempt", async () => {
  let calls = 0;
  const signals: AbortSignal[] = [];
  const client = new BrunnApiClient(
    "http://brunn.test",
    "read-token",
    async (_input, init) => {
      calls += 1;
      if (init?.signal !== null && init?.signal !== undefined) {
        signals.push(init.signal);
      }
      return calls === 1
        ? new Response("temporary", { status: 503 })
        : new Promise<Response>(() => undefined);
    },
    {},
    undefined,
    { requestMs: 80, retryBackoffMs: [1] },
  );

  const started = performance.now();
  await assert.rejects(client.request("/v1/status"), BrunnApiError);
  assert.equal(calls, 2);
  assert.equal(performance.now() - started < 200, true);
  assert.equal(signals.at(-1)?.aborted, true);
});

test("unknown mutations are never retried even when their body resembles an idempotency key", async () => {
  let calls = 0;
  const client = new BrunnApiClient(
    "http://brunn.test",
    "write-token",
    async () => {
      calls += 1;
      return new Response("temporary", { status: 503 });
    },
  );

  await assert.rejects(
    client.request("/v1/workspace/unknown-mutation", { idempotency_key: "looks-safe" }),
    (error: unknown) => error instanceof BrunnApiError
      && (error.body.error as Record<string, unknown>).retryable === false,
  );
  assert.equal(calls, 1);
});

test("briefing publication retries only when carrying its stable idempotency key", async () => {
  for (const keyed of [true, false]) {
    let calls = 0;
    const client = new BrunnApiClient(
      "http://brunn.test",
      "write-token",
      async () => {
        calls += 1;
        return calls === 1
          ? new Response("temporary", { status: 503 })
          : new Response(JSON.stringify({ status: "complete" }), { status: 200 });
      },
      {},
      undefined,
      ONE_FAST_RETRY,
    );
    const request = {
      date: "2026-08-09",
      edition: "morning",
      ...(keyed ? { idempotency_key: "briefing:2026-08-09:morning" } : {}),
    };

    if (keyed) {
      await client.request("/v1/workspace/briefings/publish", request);
      assert.equal(calls, 2);
    } else {
      await assert.rejects(
        client.request("/v1/workspace/briefings/publish", request),
        BrunnApiError,
      );
      assert.equal(calls, 1);
    }
  }
});

test("API client fetches one exact historical version without returning bytes", async () => {
  const assetRoot = await mkdtemp(join(tmpdir(), "brunn-state-client-assets-"));
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
        "x-brunn-state-asset-ref": assetRef,
        "x-brunn-state-asset-version": "3",
        "x-brunn-state-sha256": digest,
      },
    });
  };
  const client = new BrunnApiClient(
    "http://brunn.test",
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
        url: `http://brunn.test/v1/workspace/binaries/${encodeURIComponent(assetRef)}`
          + "?version=3",
        authorization: "Bearer read-token",
        hasDeadline: true,
      },
      {
        url: `http://brunn.test/v1/workspace/binaries/${encodeURIComponent(assetRef)}`
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
  const client = new BrunnApiClient("http://brunn.test", "read-token", fakeFetch);

  await assert.rejects(
    client.fetchAsset(assetRef, "session:1", 2),
    /returned version 3 for requested version 2/,
  );
  assert.equal(fetchCalls, 1);
  assert.equal(
    requestedUrl,
    `http://brunn.test/v1/workspace/binaries/${encodeURIComponent(assetRef)}?version=2`,
  );
});

test("ordinary API requests have a bounded deadline", async () => {
  const fakeFetch: typeof fetch = async () => new Promise<Response>(() => undefined);
  const client = new BrunnApiClient(
    "http://brunn.test",
    "read-token",
    fakeFetch,
    {},
    undefined,
    { requestMs: 10 },
  );

  const started = performance.now();
  await assert.rejects(
    client.request("/v1/status"),
    (error: unknown) => {
      if (!(error instanceof BrunnApiError)) {
        return false;
      }
      const detail = error.body.error as Record<string, unknown>;
      assert.equal(detail.code, "upstream_unavailable");
      assert.equal(detail.retryable, true);
      assert.equal(detail.attempts, 1);
      return true;
    },
  );
  assert.equal(performance.now() - started < 200, true);
});

test("asset fetch failures never echo upstream payloads", async () => {
  const assetRef = "entry:019f8505-da09-7d14-afd0-a9e27e47fdb7";
  const secret = Buffer.from("upstream binary payload").toString("base64");
  const metadataFailureClient = new BrunnApiClient(
    "http://brunn.test",
    "read-token",
    async () => new Response(JSON.stringify({
      error: { code: "upstream", message: secret },
      bytes: secret,
    }), { status: 500 }),
  );
  await assert.rejects(
    metadataFailureClient.fetchAsset(assetRef, "session:1"),
    (error: unknown) => {
      if (!(error instanceof BrunnApiError)) {
        return false;
      }
      assert.equal(JSON.stringify(error.body).includes(secret), false);
      assert.deepEqual(error.body, {
        error: {
          code: "asset_metadata_failed",
          message: "Brunn State asset metadata request returned HTTP 500",
        },
      });
      return true;
    },
  );

  let calls = 0;
  const downloadFailureClient = new BrunnApiClient(
    "http://brunn.test",
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
      if (!(error instanceof BrunnApiError)) {
        return false;
      }
      assert.equal(JSON.stringify(error.body).includes(secret), false);
      assert.deepEqual(error.body, {
        error: {
          code: "asset_download_failed",
          message: "Brunn State asset download request returned HTTP 502",
        },
      });
      return true;
    },
  );
});

test("staging rejects oversized files before reading or sending them", async () => {
  const importRoot = await mkdtemp(join(tmpdir(), "brunn-mcp-stage-"));
  const oversizedPath = join(importRoot, "oversized.bin");
  const previousImportRoot = process.env.BRUNN_MCP_IMPORT_ROOT;
  let fetchCalls = 0;

  try {
    await writeFile(oversizedPath, "");
    await truncate(oversizedPath, (64 * 1024 * 1024) + 1);
    process.env.BRUNN_MCP_IMPORT_ROOT = importRoot;
    const fakeFetch: typeof fetch = async () => {
      fetchCalls += 1;
      throw new Error("fetch must not run for an invalid stage request");
    };
    const client = new BrunnApiClient("http://brunn.test", "write-token", fakeFetch);

    await assert.rejects(
      client.stage("scope:primary", undefined, [{ path: "oversized.bin" }]),
      /staged files are limited to 67108864 bytes each/,
    );
    assert.equal(fetchCalls, 0);
  } finally {
    if (previousImportRoot === undefined) {
      delete process.env.BRUNN_MCP_IMPORT_ROOT;
    } else {
      process.env.BRUNN_MCP_IMPORT_ROOT = previousImportRoot;
    }
    await rm(importRoot, { recursive: true, force: true });
  }
});

test("staging preserves nested logical paths and requests binary descriptions", async () => {
  const importRoot = await mkdtemp(join(tmpdir(), "brunn-mcp-paths-"));
  const previousImportRoot = process.env.BRUNN_MCP_IMPORT_ROOT;
  const nested = join(importRoot, "Trips", "Receipts");
  const filePath = join(nested, "scan.png");
  let form: FormData | undefined;
  let idempotencyKey: string | null = null;
  let hadDeadline = false;
  try {
    await mkdir(nested, { recursive: true });
    await writeFile(filePath, Buffer.from("fixture"));
    process.env.BRUNN_MCP_IMPORT_ROOT = importRoot;
    const client = new BrunnApiClient(
      "http://brunn.test",
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
      delete process.env.BRUNN_MCP_IMPORT_ROOT;
    } else {
      process.env.BRUNN_MCP_IMPORT_ROOT = previousImportRoot;
    }
    await rm(importRoot, { recursive: true, force: true });
  }
});

test("explicit HTTP methods are preserved for bodyless GET and JSON POST PATCH PUT", async () => {
  const calls: Array<{ method: string; url: string; body: string | undefined }> = [];
  const client = new BrunnApiClient(
    "http://brunn.test",
    "task-token",
    async (input, init) => {
      calls.push({
        method: init?.method ?? "GET",
        url: String(input),
        body: typeof init?.body === "string" ? init.body : undefined,
      });
      return new Response(JSON.stringify({ status: "complete" }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    },
  );

  await client.request("GET", "/v1/workspace/tasks/candidates?view=next");
  await client.request("POST", "/v1/workspace/tasks/capture", {
    idempotency_key: "capture:1",
  });
  await client.request("PATCH", "/v1/workspace/tasks/019f8800-0000-7000-8000-000000000001", {
    expected_version: 1,
    idempotency_key: "update:1",
    operation: {
      type: "complete",
      source: "agent:codex",
      completed_via: "agent:codex",
    },
  });
  await client.request("PUT", "/v1/workspace/projects/brunn/interest", {
    expected_version: 1,
    idempotency_key: "interest:1",
    interest: "hot",
  });

  assert.deepEqual(calls, [
    {
      method: "GET",
      url: "http://brunn.test/v1/workspace/tasks/candidates?view=next",
      body: undefined,
    },
    {
      method: "POST",
      url: "http://brunn.test/v1/workspace/tasks/capture",
      body: JSON.stringify({ idempotency_key: "capture:1" }),
    },
    {
      method: "PATCH",
      url: "http://brunn.test/v1/workspace/tasks/019f8800-0000-7000-8000-000000000001",
      body: JSON.stringify({
        expected_version: 1,
        idempotency_key: "update:1",
        operation: {
          type: "complete",
          source: "agent:codex",
          completed_via: "agent:codex",
        },
      }),
    },
    {
      method: "PUT",
      url: "http://brunn.test/v1/workspace/projects/brunn/interest",
      body: JSON.stringify({
        expected_version: 1,
        idempotency_key: "interest:1",
        interest: "hot",
      }),
    },
  ]);
});

test("explicit task mutations retry only with a durable idempotency key", async () => {
  for (const fixture of [
    {
      method: "POST" as const,
      path: "/v1/workspace/tasks/capture",
      body: { idempotency_key: "capture:retry", items: [] },
    },
    {
      method: "PATCH" as const,
      path: "/v1/workspace/tasks/019f8800-0000-7000-8000-000000000001",
      body: {
        expected_version: 1,
        idempotency_key: "update:retry",
        operation: {
          type: "complete",
          source: "agent:codex",
          completed_via: "agent:codex",
        },
      },
    },
    {
      method: "PUT" as const,
      path: "/v1/workspace/contexts/available/agent",
      body: { idempotency_key: "contexts:retry", contexts_available: ["online"] },
    },
  ]) {
    let keyedCalls = 0;
    const keyed = new BrunnApiClient(
      "http://brunn.test",
      "task-token",
      async () => {
        keyedCalls += 1;
        return keyedCalls === 1
          ? new Response("temporary gateway", { status: 503 })
          : new Response(JSON.stringify({ status: "committed" }), { status: 200 });
      },
      {},
      undefined,
      ONE_FAST_RETRY,
    );
    await keyed.request(fixture.method, fixture.path, fixture.body);
    assert.equal(keyedCalls, 2, `${fixture.method} ${fixture.path} should retry with a key`);

    let unkeyedCalls = 0;
    const unkeyed = new BrunnApiClient(
      "http://brunn.test",
      "task-token",
      async () => {
        unkeyedCalls += 1;
        return new Response("temporary gateway", { status: 503 });
      },
      {},
      undefined,
      ONE_FAST_RETRY,
    );
    const { idempotency_key: _omitted, ...unkeyedBody } = fixture.body;
    await assert.rejects(
      unkeyed.request(fixture.method, fixture.path, unkeyedBody),
      (error: unknown) => {
        if (!(error instanceof BrunnApiError)) {
          return false;
        }
        const detail = error.body.error as Record<string, unknown>;
        assert.equal(detail.code, "ambiguous_outcome");
        assert.equal(detail.retryable, false);
        return true;
      },
    );
    assert.equal(unkeyedCalls, 1, `${fixture.method} ${fixture.path} must not retry without a key`);
  }
});
