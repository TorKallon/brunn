#!/usr/bin/env node

import { getOAuthProtectedResourceMetadataUrl, mcpAuthRouter } from "@modelcontextprotocol/sdk/server/auth/router.js";
import { requireBearerAuth } from "@modelcontextprotocol/sdk/server/auth/middleware/bearerAuth.js";
import type { OAuthServerProvider } from "@modelcontextprotocol/sdk/server/auth/provider.js";
import { createMcpExpressApp } from "@modelcontextprotocol/sdk/server/express.js";
import {
  StreamableHTTPServerTransport,
  type StreamableHTTPServerTransportOptions,
} from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import type { Express, NextFunction, Request, Response } from "express";
import { pathToFileURL } from "node:url";

import { StraylightApiClient } from "./api-client.js";
import { createStraylightMcpServer } from "./index.js";
import {
  StraylightOAuthProvider,
  UPSTREAM_TOKEN_EXTRA_KEY,
} from "./oauth-provider.js";

const REMOTE_SCOPE = "mcp:tools";
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
  fetchImpl?: typeof fetch;
}

export function createRemoteMcpApp(options: RemoteMcpAppOptions): Express {
  const publicUrl = canonicalPublicUrl(options.publicUrl);
  const resourceUrl = new URL("/mcp", publicUrl);
  const resourceMetadataUrl = getOAuthProtectedResourceMetadataUrl(resourceUrl);
  const fetchImpl = options.fetchImpl ?? fetch;
  const app = createMcpExpressApp({ host: "::" });
  app.disable("x-powered-by");
  app.set("trust proxy", 1);
  app.use(securityHeaders);

  app.get("/healthz", (_request, response) => {
    response.status(200).json({ status: "ok", service: "straylight-remote-mcp" });
  });

  app.get("/.well-known/oauth-protected-resource", (_request, response) => {
    response.status(200).json({
      resource: resourceUrl.href,
      authorization_servers: [publicUrl.href],
      scopes_supported: [REMOTE_SCOPE],
      resource_name: "Straylight",
    });
  });

  app.use(mcpAuthRouter({
    provider: options.provider,
    issuerUrl: publicUrl,
    resourceServerUrl: resourceUrl,
    serviceDocumentationUrl: publicUrl,
    scopesSupported: [REMOTE_SCOPE],
    resourceName: "Straylight",
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

    const client = new StraylightApiClient(options.apiUrl, upstreamToken, fetchImpl);
    const server = createStraylightMcpServer(client, {
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
    throw new Error("That Straylight credential is invalid or revoked.");
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
    throw new Error("STRAYLIGHT_MCP_SEALING_KEY must be base64 for exactly 32 bytes");
  }
  return bytes;
}

async function runRemoteServer(): Promise<void> {
  const publicUrl = new URL(requiredEnvironment("STRAYLIGHT_MCP_PUBLIC_URL"));
  const apiUrl = requiredEnvironment("STRAYLIGHT_API_URL").replace(/\/$/, "");
  const secret = decodeSealingKey(requiredEnvironment("STRAYLIGHT_MCP_SEALING_KEY"));
  const resourceUrl = new URL("/mcp", canonicalPublicUrl(publicUrl));
  const provider = new StraylightOAuthProvider({
    secret,
    resourceUrl,
    scopesSupported: [REMOTE_SCOPE],
    refreshTokenTtlSeconds: 365 * 24 * 60 * 60,
    verifyUpstreamToken: (token) => verifyRemoteCredential(apiUrl, token),
  });
  const app = createRemoteMcpApp({ publicUrl, apiUrl, provider });
  const port = parsePort(process.env.PORT ?? "8080");
  app.listen(port, "::", (error?: Error) => {
    if (error) {
      process.stderr.write(`remote MCP listener failed: ${error.message}\n`);
      process.exitCode = 1;
      return;
    }
    process.stderr.write(`Straylight remote MCP listening on port ${port}\n`);
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

function canonicalPublicUrl(value: URL): URL {
  if (value.protocol !== "https:" || value.username || value.password || value.search || value.hash) {
    throw new Error("STRAYLIGHT_MCP_PUBLIC_URL must be a credential-free HTTPS origin");
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
