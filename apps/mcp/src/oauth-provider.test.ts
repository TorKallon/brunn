import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import {
  AccessDeniedError,
  InvalidClientMetadataError,
  InvalidGrantError,
  InvalidScopeError,
  InvalidTargetError,
  InvalidTokenError,
} from "@modelcontextprotocol/sdk/server/auth/errors.js";
import type {
  AuthorizationParams,
  OAuthServerProvider,
} from "@modelcontextprotocol/sdk/server/auth/provider.js";
import type { OAuthClientInformationFull } from "@modelcontextprotocol/sdk/shared/auth.js";

import {
  StatelessOAuthClientsStore,
  BrunnOAuthProvider,
  UPSTREAM_TOKEN_EXTRA_KEY,
} from "./oauth-provider.js";

const SECRET = Buffer.alloc(32, 0x73);
const OTHER_SECRET = Buffer.alloc(32, 0x21);
const RESOURCE = new URL("https://memory.example/mcp");
const REDIRECT_URI = "https://chat.example/oauth/callback";
const VERIFIER = "a".repeat(64);
const CHALLENGE = createHash("sha256").update(VERIFIER, "ascii").digest("base64url");

type OAuthResponse = Parameters<OAuthServerProvider["authorize"]>[2];

interface MockResponseState {
  headers: Map<string, string>;
  statusCode?: number;
  type?: string;
  body?: string;
  redirectStatus?: number;
  redirectUrl?: string;
}

test("dynamic client registrations round-trip statelessly and reject tampering", async () => {
  let now = Date.UTC(2026, 6, 31);
  const first = new StatelessOAuthClientsStore({ secret: SECRET, now: () => now });
  const publicClient = await registerPublicClient(first, REDIRECT_URI, "Public client");
  assert.equal(publicClient.token_endpoint_auth_method, "none");
  assert.equal(publicClient.client_secret, undefined);

  now += 1_000;
  const confidentialClient = await first.registerClient({
    redirect_uris: [REDIRECT_URI],
    token_endpoint_auth_method: "client_secret_post",
    grant_types: ["authorization_code", "refresh_token"],
    response_types: ["code"],
    client_name: "Confidential client",
    client_secret: "generated-secret",
    client_secret_expires_at: Math.floor(now / 1_000) + 3_600,
  });

  const restarted = new StatelessOAuthClientsStore({ secret: SECRET, now: () => now });
  assert.deepEqual(await restarted.getClient(publicClient.client_id), publicClient);
  assert.deepEqual(await restarted.getClient(confidentialClient.client_id), confidentialClient);
  assert.equal(
    (await restarted.getClient(confidentialClient.client_id))?.client_secret,
    "generated-secret",
  );

  const finalCharacter = publicClient.client_id.at(-1);
  const tampered = `${publicClient.client_id.slice(0, -1)}${finalCharacter === "A" ? "B" : "A"}`;
  assert.equal(await restarted.getClient(tampered), undefined);
  const wrongKey = new StatelessOAuthClientsStore({ secret: OTHER_SECRET });
  assert.equal(await wrongKey.getClient(publicClient.client_id), undefined);
});

test("dynamic client registrations reject noncanonical base64url tail-bit aliases", async () => {
  const store = new StatelessOAuthClientsStore({ secret: SECRET });
  let canonicalClient: OAuthClientInformationFull | undefined;
  let alias: string | undefined;
  for (let suffixLength = 0; suffixLength < 3; suffixLength += 1) {
    const candidate = await registerPublicClient(
      store,
      REDIRECT_URI,
      `Tail alias fixture${"x".repeat(suffixLength)}`,
    );
    const candidateAlias = noncanonicalBase64urlTailAlias(candidate.client_id);
    if (candidateAlias !== undefined) {
      canonicalClient = candidate;
      alias = candidateAlias;
      break;
    }
  }

  assert.ok(canonicalClient, "one of three adjacent plaintext lengths must leave base64 tail bits");
  assert.ok(alias);
  assert.notEqual(alias, canonicalClient.client_id);
  const canonicalSegment = canonicalClient.client_id.split(".").at(-1);
  const aliasSegment = alias.split(".").at(-1);
  assert.ok(canonicalSegment);
  assert.ok(aliasSegment);
  assert.deepEqual(
    Buffer.from(aliasSegment, "base64url"),
    Buffer.from(canonicalSegment, "base64url"),
    "the alternate text must decode to the exact same authenticated packed bytes",
  );
  assert.deepEqual(await store.getClient(canonicalClient.client_id), canonicalClient);
  assert.equal(await store.getClient(alias), undefined);
});

test("client registration permits HTTPS and loopback HTTP redirects only", async () => {
  const store = new StatelessOAuthClientsStore({ secret: SECRET });
  for (const uri of [
    "https://example.test/callback",
    "http://localhost:45999/callback",
    "http://127.0.0.19:45999/callback?native=1",
    "http://[::1]:45999/callback",
  ]) {
    const client = await registerPublicClient(store, uri);
    assert.equal(client.redirect_uris[0], uri);
  }

  for (const uri of [
    "http://example.test/callback",
    "ftp://example.test/callback",
    "https://user:password@example.test/callback",
    "https://example.test/callback#fragment",
    "http://localhost.example.test/callback",
  ]) {
    await assert.rejects(
      registerPublicClient(store, uri),
      InvalidClientMetadataError,
    );
  }
});

function noncanonicalBase64urlTailAlias(value: string): string | undefined {
  const separator = value.lastIndexOf(".");
  assert.notEqual(separator, -1);
  const prefix = value.slice(0, separator + 1);
  const encoded = value.slice(separator + 1);
  const remainder = encoded.length % 4;
  if (remainder === 0) {
    return undefined;
  }
  assert.ok(remainder === 2 || remainder === 3);
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  const finalCharacter = encoded.at(-1);
  assert.ok(finalCharacter);
  const canonicalIndex = alphabet.indexOf(finalCharacter);
  assert.notEqual(canonicalIndex, -1);
  const unusedBitCount = remainder === 2 ? 4 : 2;
  const unusedMask = (1 << unusedBitCount) - 1;
  assert.equal(canonicalIndex & unusedMask, 0, "canonical base64url has zero unused tail bits");
  const aliasIndex = canonicalIndex | 1;
  return `${prefix}${encoded.slice(0, -1)}${alphabet[aliasIndex]}`;
}

test("authorization GET is a protected no-script approval form and POST verifies the token", async () => {
  const verified: string[] = [];
  const provider = createProvider({
    verifyUpstreamToken: async (token) => {
      verified.push(token);
    },
  });
  const client = await registerPublicClient(
    provider.clientsStore,
    REDIRECT_URI,
    '<script>alert("client")</script>',
  );
  const params = authorizationParams({ state: "request-state" });
  const get = mockResponse("GET");
  await provider.authorize(client, params, get.response);

  assert.equal(get.state.statusCode, 200);
  assert.match(get.state.type ?? "", /html/u);
  const getContentSecurityPolicy = get.state.headers.get("content-security-policy") ?? "";
  assert.match(getContentSecurityPolicy, /frame-ancestors 'none'/u);
  assert.match(getContentSecurityPolicy, /form-action 'self' https:\/\/chat\.example/u);
  assert.equal(get.state.headers.get("x-frame-options"), "DENY");
  assert.equal(get.state.headers.get("cache-control"), "no-store");
  assert.equal(get.state.headers.get("referrer-policy"), "no-referrer");
  assert.doesNotMatch(get.state.body ?? "", /<script>/u);
  assert.match(get.state.body ?? "", /&lt;script&gt;alert\(&quot;client&quot;\)&lt;\/script&gt;/u);
  assert.match(get.state.body ?? "", /https:\/\/chat\.example/u);
  assert.match(get.state.body ?? "", /dedicated root-scoped read\/write credential/u);
  assert.match(get.state.body ?? "", /name="resource" value="https:\/\/memory\.example\/mcp"/u);
  assert.equal(verified.length, 0);

  const post = mockResponse("POST", { brunn_token: "  dedicated-upstream-token  " });
  await provider.authorize(client, params, post.response);
  assert.deepEqual(verified, ["dedicated-upstream-token"]);
  const redirect = new URL(required(post.state.redirectUrl));
  assert.equal(post.state.redirectStatus, 302);
  assert.equal(redirect.origin, "https://chat.example");
  assert.match(
    post.state.headers.get("content-security-policy") ?? "",
    /form-action 'self' https:\/\/chat\.example/u,
  );
  assert.equal(redirect.searchParams.get("state"), "request-state");
  assert.ok(redirect.searchParams.get("code"));
  assert.doesNotMatch(post.state.redirectUrl ?? "", /dedicated-upstream-token/u);

  const rejected = createProvider({ verifyUpstreamToken: async () => false });
  const rejectedClient = await registerPublicClient(rejected.clientsStore, REDIRECT_URI);
  await assert.rejects(
    rejected.authorize(
      rejectedClient,
      params,
      mockResponse("POST", { brunn_token: "bad-token" }).response,
    ),
    AccessDeniedError,
  );
  await assert.rejects(
    provider.authorize(
      client,
      params,
      mockResponse("POST", { brunn_token: "x".repeat(8_193) }).response,
    ),
    /credential is required/u,
  );
});

test("authorization codes expose their PKCE challenge, exchange once, and die on restart", async () => {
  const provider = createProvider();
  const client = await registerPublicClient(provider.clientsStore, REDIRECT_URI);
  const code = await issueAuthorizationCode(provider, client, "upstream-secret");

  assert.equal(await provider.challengeForAuthorizationCode(client, code), CHALLENGE);
  const tokens = await provider.exchangeAuthorizationCode(
    client,
    code,
    VERIFIER,
    REDIRECT_URI,
    RESOURCE,
  );
  assert.doesNotMatch(tokens.access_token, /upstream-secret/u);
  assert.doesNotMatch(tokens.refresh_token ?? "", /upstream-secret/u);
  const info = await provider.verifyAccessToken(tokens.access_token);
  assert.equal(info.extra?.[UPSTREAM_TOKEN_EXTRA_KEY], "upstream-secret");
  assert.equal(info.resource?.href, RESOURCE.href);

  await assert.rejects(
    provider.exchangeAuthorizationCode(client, code, VERIFIER, REDIRECT_URI, RESOURCE),
    InvalidGrantError,
  );
  await assert.rejects(
    provider.challengeForAuthorizationCode(client, code),
    InvalidGrantError,
  );

  const freshCode = await issueAuthorizationCode(provider, client, "restart-token");
  const restarted = createProvider();
  const restartedClient = await restarted.clientsStore.getClient(client.client_id);
  assert.ok(restartedClient);
  await assert.rejects(
    restarted.challengeForAuthorizationCode(restartedClient, freshCode),
    InvalidGrantError,
  );
});

test("authorization code exchange enforces PKCE, redirect, client, and exact MCP resource", async () => {
  const provider = createProvider();
  const client = await registerPublicClient(provider.clientsStore, REDIRECT_URI);
  const otherClient = await registerPublicClient(
    provider.clientsStore,
    "https://other.example/callback",
  );
  const code = await issueAuthorizationCode(provider, client, "bound-token");

  await assert.rejects(
    provider.exchangeAuthorizationCode(otherClient, code, VERIFIER, REDIRECT_URI, RESOURCE),
    InvalidGrantError,
  );
  await assert.rejects(
    provider.exchangeAuthorizationCode(client, code, "wrong-verifier", REDIRECT_URI, RESOURCE),
    InvalidGrantError,
  );
  await assert.rejects(
    provider.exchangeAuthorizationCode(client, code, VERIFIER, "https://chat.example/wrong", RESOURCE),
    InvalidGrantError,
  );
  await assert.rejects(
    provider.exchangeAuthorizationCode(
      client,
      code,
      VERIFIER,
      REDIRECT_URI,
      new URL("https://other.example/mcp"),
    ),
    InvalidTargetError,
  );
  await assert.rejects(
    provider.exchangeAuthorizationCode(client, code, VERIFIER, REDIRECT_URI),
    InvalidTargetError,
  );

  const tokens = await provider.exchangeAuthorizationCode(
    client,
    code,
    VERIFIER,
    REDIRECT_URI,
    RESOURCE,
  );
  assert.ok(tokens.access_token);
  await assert.rejects(
    provider.authorize(
      client,
      authorizationParams({ resource: new URL("https://other.example/mcp") }),
      mockResponse("GET").response,
    ),
    InvalidTargetError,
  );
});

test("access expiry and refresh rotation enforce resource, client, and scope narrowing", async () => {
  let now = Date.UTC(2026, 6, 31);
  const provider = createProvider({
    now: () => now,
    scopesSupported: ["memory:read", "memory:write"],
    accessTokenTtlSeconds: 60,
    refreshTokenTtlSeconds: 600,
    refreshTokenReplayWindowSeconds: 1,
  });
  const client = await registerPublicClient(provider.clientsStore, REDIRECT_URI);
  const otherClient = await registerPublicClient(
    provider.clientsStore,
    "https://other.example/callback",
  );
  const code = await issueAuthorizationCode(
    provider,
    client,
    "refresh-upstream",
    authorizationParams({ scopes: ["memory:read", "memory:write"] }),
  );
  const original = await provider.exchangeAuthorizationCode(
    client,
    code,
    VERIFIER,
    REDIRECT_URI,
    RESOURCE,
  );
  const originalRefresh = required(original.refresh_token);

  await assert.rejects(
    provider.exchangeRefreshToken(otherClient, originalRefresh, undefined, RESOURCE),
    InvalidGrantError,
  );
  await assert.rejects(
    provider.exchangeRefreshToken(
      client,
      originalRefresh,
      undefined,
      new URL("https://other.example/mcp"),
    ),
    InvalidTargetError,
  );

  now += 30_000;
  const [narrowed, replayed] = await Promise.all([
    provider.exchangeRefreshToken(client, originalRefresh, ["memory:read"], RESOURCE),
    provider.exchangeRefreshToken(client, originalRefresh, ["memory:read"], RESOURCE),
  ]);
  assert.deepEqual((await provider.verifyAccessToken(narrowed.access_token)).scopes, ["memory:read"]);
  assert.deepEqual(replayed, narrowed);
  await assert.rejects(
    provider.exchangeRefreshToken(
      client,
      originalRefresh,
      ["memory:write"],
      RESOURCE,
    ),
    InvalidGrantError,
  );

  now += 2_000;
  await assert.rejects(
    provider.exchangeRefreshToken(client, originalRefresh, ["memory:read"], RESOURCE),
    InvalidGrantError,
  );
  await assert.rejects(
    provider.exchangeRefreshToken(
      client,
      required(narrowed.refresh_token),
      ["memory:read", "memory:write"],
      RESOURCE,
    ),
    InvalidScopeError,
  );

  now += 29_000;
  await assert.rejects(provider.verifyAccessToken(original.access_token), InvalidTokenError);
  assert.equal(
    (await provider.verifyAccessToken(narrowed.access_token)).extra?.[UPSTREAM_TOKEN_EXTRA_KEY],
    "refresh-upstream",
  );
});

test("expired authorization codes and malformed access tokens use SDK OAuth errors", async () => {
  let now = Date.UTC(2026, 6, 31);
  const provider = createProvider({
    now: () => now,
    authorizationCodeTtlSeconds: 30,
  });
  const client = await registerPublicClient(provider.clientsStore, REDIRECT_URI);
  const code = await issueAuthorizationCode(provider, client, "short-lived");
  now += 31_000;
  await assert.rejects(provider.challengeForAuthorizationCode(client, code), InvalidGrantError);
  await assert.rejects(provider.verifyAccessToken("not-a-token"), InvalidTokenError);
});

function createProvider(overrides: Partial<{
  verifyUpstreamToken: (token: string) => Promise<boolean | void>;
  scopesSupported: readonly string[];
  authorizationCodeTtlSeconds: number;
  accessTokenTtlSeconds: number;
  refreshTokenTtlSeconds: number;
  refreshTokenReplayWindowSeconds: number;
  now: () => number;
}> = {}): BrunnOAuthProvider {
  return new BrunnOAuthProvider({
    secret: SECRET,
    resourceUrl: RESOURCE,
    verifyUpstreamToken: overrides.verifyUpstreamToken ?? (async () => true),
    scopesSupported: overrides.scopesSupported ?? ["mcp:tools"],
    authorizationCodeTtlSeconds: overrides.authorizationCodeTtlSeconds ?? 300,
    accessTokenTtlSeconds: overrides.accessTokenTtlSeconds ?? 3_600,
    refreshTokenTtlSeconds: overrides.refreshTokenTtlSeconds ?? 86_400,
    refreshTokenReplayWindowSeconds: overrides.refreshTokenReplayWindowSeconds ?? 60,
    now: overrides.now ?? (() => Date.UTC(2026, 6, 31)),
  });
}

async function registerPublicClient(
  store: StatelessOAuthClientsStore,
  redirectUri: string,
  clientName = "Test connector",
): Promise<OAuthClientInformationFull> {
  return store.registerClient({
    redirect_uris: [redirectUri],
    token_endpoint_auth_method: "none",
    grant_types: ["authorization_code", "refresh_token"],
    response_types: ["code"],
    client_name: clientName,
  });
}

function authorizationParams(
  overrides: Partial<AuthorizationParams> = {},
): AuthorizationParams {
  return {
    redirectUri: REDIRECT_URI,
    codeChallenge: CHALLENGE,
    resource: RESOURCE,
    scopes: ["mcp:tools"],
    ...overrides,
  };
}

async function issueAuthorizationCode(
  provider: BrunnOAuthProvider,
  client: OAuthClientInformationFull,
  upstreamToken: string,
  params = authorizationParams(),
): Promise<string> {
  const response = mockResponse("POST", { brunn_token: upstreamToken });
  await provider.authorize(client, params, response.response);
  const redirect = new URL(required(response.state.redirectUrl));
  return required(redirect.searchParams.get("code") ?? undefined);
}

function mockResponse(
  method: "GET" | "POST",
  body: Record<string, unknown> = {},
): { response: OAuthResponse; state: MockResponseState } {
  const state: MockResponseState = { headers: new Map<string, string>() };
  const responseRecord: Record<string, unknown> = {};
  Object.assign(responseRecord, {
    req: { method, body },
    setHeader(name: string, value: unknown) {
      state.headers.set(name.toLowerCase(), String(value));
      return responseRecord;
    },
    status(code: number) {
      state.statusCode = code;
      return responseRecord;
    },
    type(value: string) {
      state.type = value;
      return responseRecord;
    },
    send(value: unknown) {
      state.body = String(value);
      return responseRecord;
    },
    redirect(status: number, url: string) {
      state.redirectStatus = status;
      state.redirectUrl = url;
      return responseRecord;
    },
  });
  return {
    response: responseRecord as unknown as OAuthResponse,
    state,
  };
}

function required<T>(value: T | undefined): T {
  assert.notEqual(value, undefined);
  return value as T;
}
