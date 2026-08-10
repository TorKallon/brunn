import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import type { Server } from "node:http";
import test from "node:test";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";

import { StraylightOAuthProvider } from "./oauth-provider.js";
import {
  createRemoteMcpApp,
  decodeSealingKey,
  verifyRemoteCredential,
} from "./remote.js";

const publicUrl = new URL("https://straylight.example/");
const resourceUrl = new URL("https://straylight.example/mcp");

test("remote credential validation requires dedicated root read/write access", async () => {
  const valid = credentialResponse({});
  await verifyRemoteCredential("https://api.example", "dedicated-token", mockJsonFetch(valid));

  await assert.rejects(
    verifyRemoteCredential(
      "https://api.example",
      "owner-token",
      mockJsonFetch(credentialResponse({ capabilities: [...readWriteCapabilities(), "admin"] })),
    ),
    /dedicated root-scoped read\/write credential/,
  );
  await assert.rejects(
    verifyRemoteCredential(
      "https://api.example",
      "read-only-token",
      mockJsonFetch(credentialResponse({ read_only: true, capabilities: ["open", "query", "read", "status"] })),
    ),
    /dedicated root-scoped read\/write credential/,
  );
  await assert.rejects(
    verifyRemoteCredential(
      "https://api.example",
      "wrong-scope-token",
      mockJsonFetch(credentialResponse({ scopes: [{ id: "scope:other" }] })),
    ),
    /dedicated root-scoped read\/write credential/,
  );
  await assert.rejects(
    verifyRemoteCredential(
      "https://api.example",
      "revoked-token",
      async () => new Response("denied", { status: 401 }),
    ),
    /invalid or revoked/,
  );
});

test("sealing key parser accepts exactly 32 base64 bytes", () => {
  const bytes = randomBytes(32);
  assert.deepEqual(decodeSealingKey(`base64:${bytes.toString("base64")}`), bytes);
  assert.throws(() => decodeSealingKey(randomBytes(31).toString("base64")), /exactly 32 bytes/);
  assert.throws(() => decodeSealingKey("not-base64"), /exactly 32 bytes/);
});

test("remote gateway completes OAuth and serves the hosted-safe MCP profile", async () => {
  const upstreamAuthorizations: string[] = [];
  const upstreamFetch: typeof fetch = async (_input, init) => {
    const authorization = new Headers(init?.headers).get("authorization");
    if (authorization) {
      upstreamAuthorizations.push(authorization);
    }
    return new Response(JSON.stringify({ status: "ok", source: "remote-test" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  const provider = new StraylightOAuthProvider({
    secret: randomBytes(32),
    resourceUrl,
    verifyUpstreamToken: async (token) => token === "dedicated-upstream-token",
  });
  const app = createRemoteMcpApp({
    publicUrl,
    apiUrl: "https://api.example",
    provider,
    fetchImpl: upstreamFetch,
  });
  const server = await listen(app);
  const address = server.address();
  assert.ok(address && typeof address === "object");
  const baseUrl = `http://127.0.0.1:${address.port}`;

  try {
    const unauthorized = await fetch(`${baseUrl}/mcp`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} }),
    });
    assert.equal(unauthorized.status, 401);
    assert.match(
      unauthorized.headers.get("www-authenticate") ?? "",
      /resource_metadata="https:\/\/straylight\.example\/\.well-known\/oauth-protected-resource\/mcp"/,
    );

    const resourceMetadata = await json(await fetch(`${baseUrl}/.well-known/oauth-protected-resource/mcp`));
    assert.equal(resourceMetadata.resource, resourceUrl.href);
    assert.deepEqual(resourceMetadata.authorization_servers, [publicUrl.href]);
    const rootMetadata = await json(await fetch(`${baseUrl}/.well-known/oauth-protected-resource`));
    assert.equal(rootMetadata.resource, resourceUrl.href);
    const authorizationMetadata = await json(await fetch(`${baseUrl}/.well-known/oauth-authorization-server`));
    assert.equal(authorizationMetadata.issuer, publicUrl.href);
    assert.equal(authorizationMetadata.registration_endpoint, `${publicUrl.href}register`);

    const registration = await json(await fetch(`${baseUrl}/register`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        redirect_uris: ["https://client.example/callback"],
        token_endpoint_auth_method: "none",
        grant_types: ["authorization_code", "refresh_token"],
        response_types: ["code"],
        client_name: "Remote gateway test",
      }),
    }));
    const clientId = requiredString(registration.client_id);

    const verifier = "remote-gateway-test-verifier-that-is-long-enough-for-pkce-0123456789";
    const challenge = createHash("sha256").update(verifier, "ascii").digest("base64url");
    const authorize = new URL(`${baseUrl}/authorize`);
    const authorizationFields = {
      client_id: clientId,
      redirect_uri: "https://client.example/callback",
      response_type: "code",
      code_challenge: challenge,
      code_challenge_method: "S256",
      scope: "mcp:tools",
      state: "test-state",
      resource: resourceUrl.href,
    };
    for (const [key, value] of Object.entries(authorizationFields)) {
      authorize.searchParams.set(key, value);
    }
    const approval = await fetch(authorize);
    assert.equal(approval.status, 200);
    assert.match(
      approval.headers.get("content-security-policy") ?? "",
      /form-action 'self' https:\/\/client\.example/u,
    );
    const approvalHtml = await approval.text();
    assert.match(approvalHtml, /Connect Straylight/);
    assert.equal(approvalHtml.includes("dedicated-upstream-token"), false);

    const approvalPost = await fetch(`${baseUrl}/authorize`, {
      method: "POST",
      redirect: "manual",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        ...authorizationFields,
        straylight_token: "dedicated-upstream-token",
      }),
    });
    assert.equal(approvalPost.status, 302);
    assert.match(
      approvalPost.headers.get("content-security-policy") ?? "",
      /form-action 'self' https:\/\/client\.example/u,
    );
    const callback = new URL(requiredString(approvalPost.headers.get("location")));
    assert.equal(callback.origin + callback.pathname, "https://client.example/callback");
    assert.equal(callback.searchParams.get("state"), "test-state");
    const code = requiredString(callback.searchParams.get("code"));

    const tokenResponse = await json(await fetch(`${baseUrl}/token`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "authorization_code",
        client_id: clientId,
        code,
        code_verifier: verifier,
        redirect_uri: "https://client.example/callback",
        resource: resourceUrl.href,
      }),
    }));
    const accessToken = requiredString(tokenResponse.access_token);
    assert.equal(tokenResponse.token_type, "Bearer");
    assert.equal(tokenResponse.scope, "mcp:tools");

    const transport = new StreamableHTTPClientTransport(new URL(`${baseUrl}/mcp`), {
      requestInit: { headers: { authorization: `Bearer ${accessToken}` } },
    });
    const client = new Client({ name: "remote-http-test", version: "0.1.0" });
    try {
      // SDK 1.29's concrete HTTP transport declaration is not compatible with
      // its exact-optional base declaration under this repository's TS mode.
      await client.connect(transport as never);
      const tools = await client.listTools();
      assert.equal(tools.tools.length, 14);
      assert.equal(tools.tools.some((tool) => tool.name === "memory.stage"), false);
      assert.equal(tools.tools.some((tool) => tool.name === "asset.fetch"), false);
      assert.equal(
        tools.tools.some((tool) => tool.name === "notification.publish"),
        true,
      );
      const status = await client.callTool({ name: "memory.status", arguments: {} });
      assert.equal(status.isError, undefined);
      const text = (status.content as Array<{ type: string; text?: string }>)[0]?.text;
      assert.ok(text);
      assert.equal((JSON.parse(text) as { status?: string }).status, "ok");
      const largeCheckpoint = await client.callTool({
        name: "memory.checkpoint",
        arguments: {
          session_id: "session:remote-large-checkpoint",
          idempotency_key: "remote-large-checkpoint",
          state: { objective: "x".repeat(128 * 1024) },
          source_refs: [],
        },
      });
      assert.equal(
        largeCheckpoint.isError,
        undefined,
        "remote MCP must accept valid tool payloads above Express's 100 KiB default",
      );
    } finally {
      await client.close().catch(() => undefined);
    }
    assert.deepEqual(upstreamAuthorizations, [
      "Bearer dedicated-upstream-token",
      "Bearer dedicated-upstream-token",
    ]);

    const replay = await fetch(`${baseUrl}/token`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "authorization_code",
        client_id: clientId,
        code,
        code_verifier: verifier,
        redirect_uri: "https://client.example/callback",
        resource: resourceUrl.href,
      }),
    });
    assert.equal(replay.status, 400);
    assert.equal((await json(replay)).error, "invalid_grant");
  } finally {
    await close(server);
  }
});

function credentialResponse(overrides: Record<string, unknown>): Record<string, unknown> {
  return {
    read_only: false,
    scopes: [{ id: "scope:root", scope_ref: "scope:root" }],
    capabilities: readWriteCapabilities(),
    ...overrides,
  };
}

function readWriteCapabilities(): string[] {
  return ["open", "query", "read", "save", "checkpoint", "status"];
}

function mockJsonFetch(body: Record<string, unknown>): typeof fetch {
  return async () => new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

async function listen(app: ReturnType<typeof createRemoteMcpApp>): Promise<Server> {
  return await new Promise<Server>((resolve, reject) => {
    const server = app.listen(0, "127.0.0.1", () => resolve(server));
    server.once("error", reject);
  });
}

async function close(server: Server): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    server.close((error) => error ? reject(error) : resolve());
  });
}

async function json(response: Response): Promise<Record<string, unknown>> {
  const value: unknown = await response.json();
  assert.ok(typeof value === "object" && value !== null);
  return value as Record<string, unknown>;
}

function requiredString(value: unknown): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error("expected a non-empty string");
  }
  return value;
}
