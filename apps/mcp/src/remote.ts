#!/usr/bin/env node

import { getOAuthProtectedResourceMetadataUrl, mcpAuthRouter } from "@modelcontextprotocol/sdk/server/auth/router.js";
import { requireBearerAuth } from "@modelcontextprotocol/sdk/server/auth/middleware/bearerAuth.js";
import type { OAuthServerProvider } from "@modelcontextprotocol/sdk/server/auth/provider.js";
import {
  StreamableHTTPServerTransport,
  type StreamableHTTPServerTransportOptions,
} from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import express, { type Express, type NextFunction, type Request, type Response } from "express";
import { createHash, randomUUID } from "node:crypto";
import { pathToFileURL } from "node:url";

import { BrunnApiClient } from "./api-client.js";
import { createBrunnMcpServer } from "./index.js";
import {
  BrunnOAuthProvider,
  UPSTREAM_TOKEN_EXTRA_KEY,
} from "./oauth-provider.js";

const REMOTE_SCOPE = "mcp:tools";
const MCP_ALLOWED_METHODS = "GET, POST, DELETE, OPTIONS";
const MCP_EXPOSED_HEADERS = "WWW-Authenticate, MCP-Session-Id, MCP-Protocol-Version, X-Request-ID";
// Tool schemas permit a 4 MiB checkpoint/write field. Leave bounded room for
// the JSON-RPC envelope while matching the public proxy and API body limits.
const REMOTE_JSON_BODY_LIMIT_BYTES = 5 * 1024 * 1024;
const REQUIRED_CAPABILITIES = [
  "open",
  "query",
  "read",
  "save",
  "checkpoint",
  "status",
] as const;

export interface RemoteMcpAppOptions {
  publicUrl: URL;
  apiUrl: string;
  provider: OAuthServerProvider;
  allowedOrigins: readonly string[];
  fetchImpl?: typeof fetch;
  auditLog?: (record: RemoteMcpAuditRecord) => void;
}

export interface RemoteMcpAuditRecord {
  at: string;
  event: "mcp_request";
  request_id: string;
  rpc_method: string;
  tool_name: string | null;
  idempotency_key_sha256: string | null;
  http_status: number;
  elapsed_ms: number;
  request_bytes: number | null;
  outcome: "finished" | "closed";
  origin_present: boolean;
}

export function createRemoteMcpApp(options: RemoteMcpAppOptions): Express {
  const publicUrl = canonicalPublicUrl(options.publicUrl);
  const resourceUrl = new URL("/mcp", publicUrl);
  const resourceMetadataUrl = getOAuthProtectedResourceMetadataUrl(resourceUrl);
  const fetchImpl = options.fetchImpl ?? fetch;
  const auditLog = options.auditLog ?? writeRemoteMcpAuditLog;
  const allowedOrigins = new Set(normalizeAllowedOrigins(options.allowedOrigins));
  const app = express();
  app.use("/mcp", mcpCors(allowedOrigins));
  app.use("/mcp", auditRemoteMcpRequest(auditLog));
  app.use(express.json({ limit: REMOTE_JSON_BODY_LIMIT_BYTES, strict: true }));
  app.disable("x-powered-by");
  app.set("trust proxy", 1);
  app.use(securityHeaders);

  app.get("/healthz", (_request, response) => {
    response.status(200).json({ status: "ok", service: "brunn-remote-mcp" });
  });

  app.options("/.well-known/oauth-protected-resource", publicMetadataCors);
  app.get("/.well-known/oauth-protected-resource", publicMetadataCors, (_request, response) => {
    response.status(200).json({
      resource: resourceUrl.href,
      authorization_servers: [publicUrl.href],
      scopes_supported: [REMOTE_SCOPE],
      resource_name: "Brunn",
    });
  });

  app.use(mcpAuthRouter({
    provider: options.provider,
    issuerUrl: publicUrl,
    resourceServerUrl: resourceUrl,
    serviceDocumentationUrl: publicUrl,
    scopesSupported: [REMOTE_SCOPE],
    resourceName: "Brunn",
    clientRegistrationOptions: {
      clientIdGeneration: false,
      clientSecretExpirySeconds: 365 * 24 * 60 * 60,
    },
  }));

  const bearerAuth = requireBearerAuth({
    verifier: options.provider,
    requiredScopes: [REMOTE_SCOPE],
    resourceMetadataUrl,
  });

  app.post("/mcp", bearerAuth, async (request, response) => {
    const upstreamToken = request.auth?.extra?.[UPSTREAM_TOKEN_EXTRA_KEY];
    if (typeof upstreamToken !== "string" || upstreamToken.length === 0) {
      response.status(401).json({ error: "invalid_token" });
      return;
    }

    const client = new BrunnApiClient(options.apiUrl, upstreamToken, fetchImpl);
    const server = createBrunnMcpServer(client, {
      surface: "remote",
      includeStructuredContent: true,
    });
    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined,
      enableJsonResponse: true,
    } as unknown as StreamableHTTPServerTransportOptions);
    let closed = false;
    const close = async (): Promise<void> => {
      if (closed) {
        return;
      }
      closed = true;
      await server.close().catch(() => undefined);
    };
    response.once("close", () => void close());

    try {
      // The SDK's Node transport declaration is not exact-optional compatible
      // with its base Transport declaration, though it implements that type at runtime.
      await server.connect(transport as never);
      await transport.handleRequest(request, response, request.body);
    } catch {
      if (!response.headersSent) {
        response.status(500).json({
          jsonrpc: "2.0",
          error: { code: -32603, message: "Internal server error" },
          id: null,
        });
      }
    } finally {
      if (response.writableEnded) {
        await close();
      }
    }
  });

  const methodNotAllowed = (_request: Request, response: Response): void => {
    response.status(405).json({
      jsonrpc: "2.0",
      error: { code: -32000, message: "Method not allowed" },
      id: null,
    });
  };
  app.get("/mcp", bearerAuth, methodNotAllowed);
  app.delete("/mcp", bearerAuth, methodNotAllowed);

  app.use((error: unknown, _request: Request, response: Response, _next: NextFunction) => {
    if (response.headersSent) {
      return;
    }
    const status = isEntityTooLarge(error) ? 413 : 400;
    response.status(status).json({ error: status === 413 ? "request_too_large" : "invalid_request" });
  });
  return app;
}

export async function verifyRemoteCredential(
  apiUrl: string,
  token: string,
  fetchImpl: typeof fetch = fetch,
): Promise<void> {
  const response = await fetchImpl(`${apiUrl.replace(/\/$/, "")}/v1/me`, {
    method: "GET",
    headers: {
      accept: "application/json",
      authorization: `Bearer ${token}`,
    },
    redirect: "error",
    signal: AbortSignal.timeout(15_000),
  });
  if (!response.ok) {
    await response.body?.cancel().catch(() => undefined);
    throw new Error("That Brunn credential is invalid or revoked.");
  }
  const value: unknown = await response.json();
  const body = unwrapData(value);
  const capabilities = stringArray(body.capabilities);
  const scopes = Array.isArray(body.scopes) ? body.scopes : [];
  const hasRoot = scopes.some((scope) => {
    if (typeof scope !== "object" || scope === null) {
      return false;
    }
    const record = scope as Record<string, unknown>;
    return record.scope_ref === "scope:root" || record.id === "scope:root";
  });
  const missing = REQUIRED_CAPABILITIES.filter((capability) => !capabilities.includes(capability));
  const isOwner = capabilities.includes("admin") || capabilities.includes("credential:manage");
  if (body.read_only !== false || !hasRoot || missing.length > 0 || isOwner) {
    throw new Error(
      "Use a dedicated root-scoped read/write credential without owner or credential-management access.",
    );
  }
}

export function decodeSealingKey(value: string): Uint8Array {
  const trimmed = value.trim();
  const encoded = trimmed.startsWith("base64:") ? trimmed.slice(7) : trimmed;
  const bytes = Buffer.from(encoded, "base64");
  if (bytes.byteLength !== 32 || bytes.toString("base64").replace(/=+$/, "") !== encoded.replace(/=+$/, "")) {
    throw new Error("BRUNN_MCP_SEALING_KEY must be base64 for exactly 32 bytes");
  }
  return bytes;
}

async function runRemoteServer(): Promise<void> {
  const publicUrl = new URL(requiredEnvironment("BRUNN_MCP_PUBLIC_URL"));
  const apiUrl = requiredEnvironment("BRUNN_API_URL").replace(/\/$/, "");
  const secret = decodeSealingKey(requiredEnvironment("BRUNN_MCP_SEALING_KEY"));
  const allowedOrigins = parseAllowedOrigins(
    requiredEnvironment("BRUNN_MCP_ALLOWED_ORIGINS"),
  );
  const resourceUrl = new URL("/mcp", canonicalPublicUrl(publicUrl));
  const provider = new BrunnOAuthProvider({
    secret,
    resourceUrl,
    scopesSupported: [REMOTE_SCOPE],
    refreshTokenTtlSeconds: 365 * 24 * 60 * 60,
    verifyUpstreamToken: (token) => verifyRemoteCredential(apiUrl, token),
  });
  const app = createRemoteMcpApp({ publicUrl, apiUrl, provider, allowedOrigins });
  const port = parsePort(process.env.PORT ?? "8080");
  app.listen(port, "::", (error?: Error) => {
    if (error) {
      process.stderr.write(`remote MCP listener failed: ${error.message}\n`);
      process.exitCode = 1;
      return;
    }
    process.stderr.write(`Brunn remote MCP listening on port ${port}\n`);
  });
}

function securityHeaders(_request: Request, response: Response, next: NextFunction): void {
  response.setHeader("Cache-Control", "no-store");
  response.setHeader("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'");
  response.setHeader("Permissions-Policy", "camera=(), microphone=(), geolocation=()");
  response.setHeader("Referrer-Policy", "no-referrer");
  response.setHeader("X-Content-Type-Options", "nosniff");
  response.setHeader("X-Frame-Options", "DENY");
  next();
}

function mcpCors(allowedOrigins: ReadonlySet<string>) {
  return (request: Request, response: Response, next: NextFunction): void => {
    response.vary("Origin");
    const origin = request.get("Origin");
    if (origin === undefined) {
      next();
      return;
    }
    if (!allowedOrigins.has(origin)) {
      response.status(403).json({ error: "origin_not_allowed" });
      return;
    }

    response.setHeader("Access-Control-Allow-Origin", origin);
    response.setHeader("Access-Control-Allow-Methods", MCP_ALLOWED_METHODS);
    response.setHeader("Access-Control-Expose-Headers", MCP_EXPOSED_HEADERS);

    if (request.method !== "OPTIONS") {
      next();
      return;
    }

    const requestedHeaders = request.get("Access-Control-Request-Headers");
    if (requestedHeaders !== undefined) {
      response.vary("Access-Control-Request-Headers");
      response.setHeader("Access-Control-Allow-Headers", requestedHeaders);
    }
    response.setHeader("Access-Control-Max-Age", "600");
    response.status(204).end();
  };
}

function publicMetadataCors(request: Request, response: Response, next: NextFunction): void {
  response.setHeader("Access-Control-Allow-Origin", "*");
  response.setHeader("Access-Control-Allow-Methods", "GET, OPTIONS");
  const requestedHeaders = request.get("Access-Control-Request-Headers");
  if (requestedHeaders !== undefined) {
    response.vary("Access-Control-Request-Headers");
    response.setHeader("Access-Control-Allow-Headers", requestedHeaders);
  }
  if (request.method === "OPTIONS") {
    response.setHeader("Access-Control-Max-Age", "600");
    response.status(204).end();
    return;
  }
  next();
}

function auditRemoteMcpRequest(
  auditLog: (record: RemoteMcpAuditRecord) => void,
) {
  return (request: Request, response: Response, next: NextFunction): void => {
    if (request.method !== "POST") {
      next();
      return;
    }
    const started = performance.now();
    const requestId = safeRequestId(request.get("x-request-id")) ?? randomUUID();
    response.setHeader("X-Request-ID", requestId);
    let recorded = false;
    const record = (outcome: RemoteMcpAuditRecord["outcome"]): void => {
      if (recorded) {
        return;
      }
      recorded = true;
      const identity = remoteMcpIdentity(request.body);
      try {
        auditLog({
          at: new Date().toISOString(),
          event: "mcp_request",
          request_id: requestId,
          rpc_method: identity.rpcMethod,
          tool_name: identity.toolName,
          idempotency_key_sha256: identity.idempotencyKeySha256,
          http_status: response.statusCode,
          elapsed_ms: Math.round((performance.now() - started) * 1_000) / 1_000,
          request_bytes: requestByteLength(request),
          outcome,
          origin_present: request.get("origin") !== undefined,
        });
      } catch {
        // Operational logging is observational and must never alter MCP behavior.
      }
    };
    response.once("finish", () => record("finished"));
    response.once("close", () => {
      if (!response.writableEnded) {
        record("closed");
      }
    });
    next();
  };
}

function remoteMcpIdentity(body: unknown): {
  rpcMethod: string;
  toolName: string | null;
  idempotencyKeySha256: string | null;
} {
  if (typeof body !== "object" || body === null) {
    return { rpcMethod: "unknown", toolName: null, idempotencyKeySha256: null };
  }
  const request = body as Record<string, unknown>;
  const rpcMethod = safeProtocolName(request.method) ?? "other";
  if (rpcMethod !== "tools/call" || typeof request.params !== "object" || request.params === null) {
    return { rpcMethod, toolName: null, idempotencyKeySha256: null };
  }
  const params = request.params as Record<string, unknown>;
  const toolName = safeProtocolName(params.name);
  const args = typeof params.arguments === "object" && params.arguments !== null
    ? params.arguments as Record<string, unknown>
    : {};
  const idempotencyKey = args.idempotency_key;
  const idempotencyKeySha256 = typeof idempotencyKey === "string"
    && idempotencyKey.length > 0
    && Buffer.byteLength(idempotencyKey, "utf8") <= 256
    && !/[\u0000-\u001f\u007f-\u009f]/u.test(idempotencyKey)
    ? `sha256:${createHash("sha256").update(idempotencyKey, "utf8").digest("hex")}`
    : null;
  return { rpcMethod, toolName, idempotencyKeySha256 };
}

function requestByteLength(request: Request): number | null {
  const declared = request.get("content-length");
  if (declared !== undefined && /^\d{1,16}$/u.test(declared)) {
    const parsed = Number(declared);
    if (Number.isSafeInteger(parsed)) {
      return parsed;
    }
  }
  try {
    return request.body === undefined
      ? null
      : Buffer.byteLength(JSON.stringify(request.body), "utf8");
  } catch {
    return null;
  }
}

function safeProtocolName(value: unknown): string | null {
  return typeof value === "string" && /^[a-z0-9][a-z0-9._/-]{0,127}$/iu.test(value)
    ? value
    : null;
}

function safeRequestId(value: unknown): string | undefined {
  return typeof value === "string" && /^[a-z0-9][a-z0-9._:-]{0,127}$/iu.test(value)
    ? value
    : undefined;
}

function writeRemoteMcpAuditLog(record: RemoteMcpAuditRecord): void {
  try {
    process.stderr.write(`${JSON.stringify(record)}\n`);
  } catch {
    // Log delivery is fail-open.
  }
}

export function parseAllowedOrigins(value: string): string[] {
  return normalizeAllowedOrigins(value.split(","));
}

function normalizeAllowedOrigins(values: readonly string[]): string[] {
  const origins = new Set<string>();
  for (const value of values) {
    const candidate = value.trim();
    if (candidate.length === 0 || candidate === "*") {
      throw new Error("BRUNN_MCP_ALLOWED_ORIGINS requires explicit HTTPS origins");
    }
    let parsed: URL;
    try {
      parsed = new URL(candidate);
    } catch {
      throw new Error("BRUNN_MCP_ALLOWED_ORIGINS contains an invalid URL");
    }
    if (
      parsed.protocol !== "https:"
      || parsed.username.length > 0
      || parsed.password.length > 0
      || parsed.pathname !== "/"
      || parsed.search.length > 0
      || parsed.hash.length > 0
    ) {
      throw new Error("BRUNN_MCP_ALLOWED_ORIGINS requires exact credential-free HTTPS origins");
    }
    origins.add(parsed.origin);
  }
  if (origins.size === 0) {
    throw new Error("BRUNN_MCP_ALLOWED_ORIGINS requires at least one origin");
  }
  return [...origins];
}

function canonicalPublicUrl(value: URL): URL {
  if (value.protocol !== "https:" || value.username || value.password || value.search || value.hash) {
    throw new Error("BRUNN_MCP_PUBLIC_URL must be a credential-free HTTPS origin");
  }
  const canonical = new URL(value.origin);
  canonical.pathname = "/";
  return canonical;
}

function unwrapData(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null) {
    return {};
  }
  const record = value as Record<string, unknown>;
  if (typeof record.data === "object" && record.data !== null) {
    return record.data as Record<string, unknown>;
  }
  return record;
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

function isEntityTooLarge(error: unknown): boolean {
  return typeof error === "object" && error !== null && "type" in error
    && (error as { type?: unknown }).type === "entity.too.large";
}

function parsePort(value: string): number {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < 1 || parsed > 65_535) {
    throw new Error("PORT must be an integer from 1 through 65535");
  }
  return parsed;
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}

if (
  process.argv[1] !== undefined
  && import.meta.url === pathToFileURL(process.argv[1]).href
) {
  await runRemoteServer();
}
