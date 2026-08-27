import {
  createCipheriv,
  createDecipheriv,
  createHash,
  randomBytes,
  randomUUID,
  timingSafeEqual,
} from "node:crypto";

import type { OAuthRegisteredClientsStore } from "@modelcontextprotocol/sdk/server/auth/clients.js";
import {
  AccessDeniedError,
  InvalidClientMetadataError,
  InvalidGrantError,
  InvalidRequestError,
  InvalidScopeError,
  InvalidTargetError,
  InvalidTokenError,
} from "@modelcontextprotocol/sdk/server/auth/errors.js";
import type {
  AuthorizationParams,
  OAuthServerProvider,
} from "@modelcontextprotocol/sdk/server/auth/provider.js";
import type { AuthInfo } from "@modelcontextprotocol/sdk/server/auth/types.js";
import {
  OAuthClientInformationFullSchema,
  type OAuthClientInformationFull,
  type OAuthTokens,
} from "@modelcontextprotocol/sdk/shared/auth.js";

const CLIENT_TOKEN_KIND = "client";
const AUTHORIZATION_CODE_KIND = "code";
const ACCESS_TOKEN_KIND = "access";
const REFRESH_TOKEN_KIND = "refresh";
const TOKEN_VERSION = 1;
const MAX_SEALED_VALUE_LENGTH = 128 * 1024;
const MAX_UPSTREAM_TOKEN_LENGTH = 8_192;

const DEFAULT_SCOPES = ["mcp:tools"] as const;
const DEFAULT_AUTHORIZATION_CODE_TTL_SECONDS = 5 * 60;
const DEFAULT_ACCESS_TOKEN_TTL_SECONDS = 60 * 60;
const DEFAULT_REFRESH_TOKEN_TTL_SECONDS = 30 * 24 * 60 * 60;

export const UPSTREAM_TOKEN_EXTRA_KEY = "upstreamToken";

type OAuthResponse = Parameters<OAuthServerProvider["authorize"]>[2];
type UpstreamTokenVerifier = (token: string) => Promise<boolean | void>;
type Clock = () => number;

export interface StraylightOAuthProviderOptions {
  secret: string | Uint8Array;
  resourceUrl: string | URL;
  verifyUpstreamToken: UpstreamTokenVerifier;
  scopesSupported?: readonly string[];
  authorizationCodeTtlSeconds?: number;
  accessTokenTtlSeconds?: number;
  refreshTokenTtlSeconds?: number;
  now?: Clock;
}

export interface StatelessOAuthClientsStoreOptions {
  secret: string | Uint8Array;
  now?: Clock;
}

interface ClientEnvelope {
  kind: typeof CLIENT_TOKEN_KIND;
  issuedAt: number;
  metadata: Record<string, unknown>;
}

interface AuthorizationCodeEnvelope {
  kind: typeof AUTHORIZATION_CODE_KIND;
  instanceId: string;
  id: string;
  expiresAt: number;
  upstreamToken: string;
  clientId: string;
  scopes: string[];
  resource: string;
  redirectUri: string;
  codeChallenge: string;
}

interface OAuthTokenEnvelope {
  kind: typeof ACCESS_TOKEN_KIND | typeof REFRESH_TOKEN_KIND;
  id: string;
  expiresAt: number;
  upstreamToken: string;
  clientId: string;
  scopes: string[];
  resource: string;
}

/**
 * A dynamic-client store whose client IDs contain authenticated, encrypted
 * registration metadata. No client registration or client secret is written
 * to disk or retained in a process-global collection.
 */
export class StatelessOAuthClientsStore implements OAuthRegisteredClientsStore {
  private readonly sealer: AesGcmSealer;
  private readonly now: Clock;

  constructor(options: StatelessOAuthClientsStoreOptions) {
    this.sealer = new AesGcmSealer(options.secret);
    this.now = options.now ?? Date.now;
  }

  async registerClient(
    client: Omit<OAuthClientInformationFull, "client_id" | "client_id_issued_at">,
  ): Promise<OAuthClientInformationFull> {
    const issuedAt = this.nowSeconds();
    const parsed = OAuthClientInformationFullSchema.safeParse({
      ...client,
      client_id: "pending",
      client_id_issued_at: issuedAt,
    });
    if (!parsed.success) {
      throw new InvalidClientMetadataError(parsed.error.message);
    }

    validateRegisteredClient(parsed.data);
    const {
      client_id: _discardedClientId,
      client_id_issued_at: _discardedIssuedAt,
      ...metadata
    } = parsed.data;
    const envelope: ClientEnvelope = {
      kind: CLIENT_TOKEN_KIND,
      issuedAt,
      metadata,
    };
    const clientId = this.sealer.seal(CLIENT_TOKEN_KIND, envelope);
    return {
      ...metadata,
      client_id: clientId,
      client_id_issued_at: issuedAt,
    };
  }

  async getClient(clientId: string): Promise<OAuthClientInformationFull | undefined> {
    try {
      const value = this.sealer.open(CLIENT_TOKEN_KIND, clientId);
      const envelope = parseClientEnvelope(value);
      if (!envelope) {
        return undefined;
      }
      const parsed = OAuthClientInformationFullSchema.safeParse({
        ...envelope.metadata,
        client_id: clientId,
        client_id_issued_at: envelope.issuedAt,
      });
      if (!parsed.success) {
        return undefined;
      }
      validateRegisteredClient(parsed.data);
      return parsed.data;
    } catch {
      return undefined;
    }
  }

  private nowSeconds(): number {
    return Math.floor(this.now() / 1_000);
  }
}

/**
 * OAuth 2.1 provider for the hosted Straylight MCP endpoint.
 *
 * Client registrations and all issued credentials are AES-256-GCM sealed.
 * Authorization codes additionally carry a per-process instance identifier,
 * and their identifiers are consumed in memory before token issuance. This
 * makes a code single-use and deliberately invalid after a server restart.
 */
export class StraylightOAuthProvider implements OAuthServerProvider {
  readonly clientsStore: StatelessOAuthClientsStore;
  readonly skipLocalPkceValidation = false;

  private readonly sealer: AesGcmSealer;
  private readonly resourceUrl: URL;
  private readonly verifyUpstreamToken: UpstreamTokenVerifier;
  private readonly scopesSupported: readonly string[];
  private readonly authorizationCodeTtlSeconds: number;
  private readonly accessTokenTtlSeconds: number;
  private readonly refreshTokenTtlSeconds: number;
  private readonly now: Clock;
  private readonly instanceId = randomUUID();
  private readonly consumedAuthorizationCodes = new Map<string, number>();
  private readonly consumedRefreshTokens = new Map<string, number>();

  constructor(options: StraylightOAuthProviderOptions) {
    this.sealer = new AesGcmSealer(options.secret);
    this.resourceUrl = validateResourceUrl(options.resourceUrl);
    this.verifyUpstreamToken = options.verifyUpstreamToken;
    this.scopesSupported = validateSupportedScopes(options.scopesSupported ?? DEFAULT_SCOPES);
    this.authorizationCodeTtlSeconds = validateTtl(
      "authorizationCodeTtlSeconds",
      options.authorizationCodeTtlSeconds ?? DEFAULT_AUTHORIZATION_CODE_TTL_SECONDS,
    );
    this.accessTokenTtlSeconds = validateTtl(
      "accessTokenTtlSeconds",
      options.accessTokenTtlSeconds ?? DEFAULT_ACCESS_TOKEN_TTL_SECONDS,
    );
    this.refreshTokenTtlSeconds = validateTtl(
      "refreshTokenTtlSeconds",
      options.refreshTokenTtlSeconds ?? DEFAULT_REFRESH_TOKEN_TTL_SECONDS,
    );
    this.now = options.now ?? Date.now;
    this.clientsStore = new StatelessOAuthClientsStore({
      secret: options.secret,
      now: this.now,
    });
  }

  async authorize(
    client: OAuthClientInformationFull,
    params: AuthorizationParams,
    res: OAuthResponse,
  ): Promise<void> {
    if (!redirectUriMatches(params.redirectUri, client.redirect_uris)) {
      throw new InvalidRequestError("Unregistered redirect_uri");
    }
    this.requireExactResource(params.resource);
    const scopes = this.grantedScopes(params.scopes);
    const request = res.req as unknown as {
      method?: unknown;
      body?: unknown;
    };
    setAuthorizationResponseHeaders(res, params.redirectUri);

    if (request.method !== "POST") {
      res.status(200).type("html").send(renderApprovalForm(client, params, scopes));
      return;
    }

    const upstreamToken = extractSubmittedToken(request.body);
    if (!upstreamToken) {
      throw new InvalidRequestError("A Straylight credential is required");
    }
    try {
      const accepted = await this.verifyUpstreamToken(upstreamToken);
      if (accepted === false) {
        throw new Error("credential rejected");
      }
    } catch {
      throw new AccessDeniedError("The Straylight credential could not be verified");
    }

    const codeEnvelope: AuthorizationCodeEnvelope = {
      kind: AUTHORIZATION_CODE_KIND,
      instanceId: this.instanceId,
      id: randomUUID(),
      expiresAt: this.nowSeconds() + this.authorizationCodeTtlSeconds,
      upstreamToken,
      clientId: client.client_id,
      scopes,
      resource: this.resourceUrl.href,
      redirectUri: params.redirectUri,
      codeChallenge: params.codeChallenge,
    };
    const code = this.sealer.seal(AUTHORIZATION_CODE_KIND, codeEnvelope);
    const redirect = new URL(params.redirectUri);
    redirect.searchParams.set("code", code);
    if (params.state !== undefined) {
      redirect.searchParams.set("state", params.state);
    }
    res.redirect(302, redirect.href);
  }

  async challengeForAuthorizationCode(
    client: OAuthClientInformationFull,
    authorizationCode: string,
  ): Promise<string> {
    const envelope = this.readAuthorizationCode(authorizationCode);
    this.validateAuthorizationCode(envelope, client);
    return envelope.codeChallenge;
  }

  async exchangeAuthorizationCode(
    client: OAuthClientInformationFull,
    authorizationCode: string,
    codeVerifier?: string,
    redirectUri?: string,
    resource?: URL,
  ): Promise<OAuthTokens> {
    const envelope = this.readAuthorizationCode(authorizationCode);
    this.validateAuthorizationCode(envelope, client);
    if (redirectUri !== envelope.redirectUri) {
      throw new InvalidGrantError("Authorization code redirect_uri does not match");
    }
    this.requireExactResource(resource);
    if (envelope.resource !== resource.href) {
      throw new InvalidTargetError("Authorization code resource does not match");
    }
    if (codeVerifier !== undefined && !pkceVerifierMatches(codeVerifier, envelope.codeChallenge)) {
      throw new InvalidGrantError("code_verifier does not match the challenge");
    }

    this.consumedAuthorizationCodes.set(envelope.id, envelope.expiresAt);
    return this.issueTokenPair(
      envelope.upstreamToken,
      envelope.clientId,
      envelope.scopes,
    );
  }

  async exchangeRefreshToken(
    client: OAuthClientInformationFull,
    refreshToken: string,
    scopes?: string[],
    resource?: URL,
  ): Promise<OAuthTokens> {
    const envelope = this.readOAuthToken(REFRESH_TOKEN_KIND, refreshToken, InvalidGrantError);
    if (envelope.clientId !== client.client_id) {
      throw new InvalidGrantError("Refresh token was not issued to this client");
    }
    this.requireExactResource(resource);
    if (envelope.resource !== resource.href) {
      throw new InvalidTargetError("Refresh token resource does not match");
    }
    this.pruneConsumed(this.consumedRefreshTokens);
    if (this.consumedRefreshTokens.has(envelope.id)) {
      throw new InvalidGrantError("Refresh token has already been used");
    }

    const narrowedScopes = scopes === undefined
      ? envelope.scopes
      : validateNarrowedScopes(scopes, envelope.scopes);
    this.consumedRefreshTokens.set(envelope.id, envelope.expiresAt);
    return this.issueTokenPair(
      envelope.upstreamToken,
      envelope.clientId,
      narrowedScopes,
    );
  }

  async verifyAccessToken(token: string): Promise<AuthInfo> {
    const envelope = this.readOAuthToken(ACCESS_TOKEN_KIND, token, InvalidTokenError);
    if (envelope.resource !== this.resourceUrl.href) {
      throw new InvalidTokenError("Access token has an invalid resource");
    }
    return {
      token,
      clientId: envelope.clientId,
      scopes: envelope.scopes,
      expiresAt: envelope.expiresAt,
      resource: new URL(envelope.resource),
      extra: {
        [UPSTREAM_TOKEN_EXTRA_KEY]: envelope.upstreamToken,
      },
    };
  }

  private issueTokenPair(
    upstreamToken: string,
    clientId: string,
    scopes: readonly string[],
  ): OAuthTokens {
    const now = this.nowSeconds();
    const accessEnvelope: OAuthTokenEnvelope = {
      kind: ACCESS_TOKEN_KIND,
      id: randomUUID(),
      expiresAt: now + this.accessTokenTtlSeconds,
      upstreamToken,
      clientId,
      scopes: [...scopes],
      resource: this.resourceUrl.href,
    };
    const refreshEnvelope: OAuthTokenEnvelope = {
      kind: REFRESH_TOKEN_KIND,
      id: randomUUID(),
      expiresAt: now + this.refreshTokenTtlSeconds,
      upstreamToken,
      clientId,
      scopes: [...scopes],
      resource: this.resourceUrl.href,
    };
    return {
      access_token: this.sealer.seal(ACCESS_TOKEN_KIND, accessEnvelope),
      token_type: "Bearer",
      expires_in: this.accessTokenTtlSeconds,
      scope: scopes.join(" "),
      refresh_token: this.sealer.seal(REFRESH_TOKEN_KIND, refreshEnvelope),
    };
  }

  private readAuthorizationCode(authorizationCode: string): AuthorizationCodeEnvelope {
    try {
      const envelope = parseAuthorizationCodeEnvelope(
        this.sealer.open(AUTHORIZATION_CODE_KIND, authorizationCode),
      );
      if (!envelope) {
        throw new Error("malformed authorization code");
      }
      return envelope;
    } catch {
      throw new InvalidGrantError("Invalid authorization code");
    }
  }

  private validateAuthorizationCode(
    envelope: AuthorizationCodeEnvelope,
    client: OAuthClientInformationFull,
  ): void {
    this.pruneConsumed(this.consumedAuthorizationCodes);
    if (envelope.instanceId !== this.instanceId) {
      throw new InvalidGrantError("Authorization code is no longer valid");
    }
    if (envelope.expiresAt <= this.nowSeconds()) {
      throw new InvalidGrantError("Authorization code has expired");
    }
    if (envelope.clientId !== client.client_id) {
      throw new InvalidGrantError("Authorization code was not issued to this client");
    }
    if (this.consumedAuthorizationCodes.has(envelope.id)) {
      throw new InvalidGrantError("Authorization code has already been used");
    }
    if (envelope.resource !== this.resourceUrl.href) {
      throw new InvalidGrantError("Authorization code has an invalid resource");
    }
  }

  private readOAuthToken<TError extends InvalidGrantError | InvalidTokenError>(
    kind: typeof ACCESS_TOKEN_KIND | typeof REFRESH_TOKEN_KIND,
    token: string,
    ErrorType: new (message: string) => TError,
  ): OAuthTokenEnvelope {
    try {
      const envelope = parseOAuthTokenEnvelope(this.sealer.open(kind, token), kind);
      if (!envelope || envelope.expiresAt <= this.nowSeconds()) {
        throw new Error("invalid or expired token");
      }
      return envelope;
    } catch {
      throw new ErrorType(`Invalid or expired ${kind} token`);
    }
  }

  private grantedScopes(requested: string[] | undefined): string[] {
    const scopes = requested === undefined || requested.length === 0
      ? [...this.scopesSupported]
      : uniqueScopes(requested);
    const supported = new Set(this.scopesSupported);
    if (scopes.some((scope) => !supported.has(scope))) {
      throw new InvalidScopeError("One or more requested scopes are not supported");
    }
    return scopes;
  }

  private requireExactResource(resource: URL | undefined): asserts resource is URL {
    if (!resource || resource.href !== this.resourceUrl.href) {
      throw new InvalidTargetError(`resource must be ${this.resourceUrl.href}`);
    }
  }

  private pruneConsumed(entries: Map<string, number>): void {
    const now = this.nowSeconds();
    for (const [id, expiresAt] of entries) {
      if (expiresAt <= now) {
        entries.delete(id);
      }
    }
  }

  private nowSeconds(): number {
    return Math.floor(this.now() / 1_000);
  }
}

class AesGcmSealer {
  private readonly key: Buffer;

  constructor(secret: string | Uint8Array) {
    const bytes = typeof secret === "string" ? Buffer.from(secret, "utf8") : Buffer.from(secret);
    if (bytes.byteLength < 32) {
      throw new Error("OAuth secret must contain at least 32 bytes");
    }
    this.key = createHash("sha256")
      .update("straylight-mcp-oauth-key-v1\0", "utf8")
      .update(bytes)
      .digest();
  }

  seal(kind: string, value: unknown): string {
    const prefix = sealedPrefix(kind);
    const iv = randomBytes(12);
    const cipher = createCipheriv("aes-256-gcm", this.key, iv);
    cipher.setAAD(Buffer.from(prefix, "utf8"));
    const ciphertext = Buffer.concat([
      cipher.update(JSON.stringify(value), "utf8"),
      cipher.final(),
    ]);
    const packed = Buffer.concat([iv, cipher.getAuthTag(), ciphertext]);
    return `${prefix}.${packed.toString("base64url")}`;
  }

  open(kind: string, sealed: string): unknown {
    const prefix = sealedPrefix(kind);
    if (
      sealed.length > MAX_SEALED_VALUE_LENGTH ||
      !sealed.startsWith(`${prefix}.`) ||
      !/^[A-Za-z0-9_-]+$/.test(sealed.slice(prefix.length + 1))
    ) {
      throw new Error("Invalid sealed value");
    }
    const encoded = sealed.slice(prefix.length + 1);
    const packed = Buffer.from(encoded, "base64url");
    // Node's base64url decoder accepts noncanonical trailing-bit aliases. Two
    // different strings can therefore decode to the same authenticated bytes,
    // making a textual client/token mutation appear valid. Sealed identities
    // are canonical protocol values, so reject aliases before authenticating.
    if (packed.toString("base64url") !== encoded) {
      throw new Error("Invalid sealed value");
    }
    if (packed.byteLength <= 28) {
      throw new Error("Invalid sealed value");
    }
    const iv = packed.subarray(0, 12);
    const tag = packed.subarray(12, 28);
    const ciphertext = packed.subarray(28);
    const decipher = createDecipheriv("aes-256-gcm", this.key, iv);
    decipher.setAAD(Buffer.from(prefix, "utf8"));
    decipher.setAuthTag(tag);
    const plaintext = Buffer.concat([
      decipher.update(ciphertext),
      decipher.final(),
    ]).toString("utf8");
    return JSON.parse(plaintext) as unknown;
  }
}

function sealedPrefix(kind: string): string {
  return `straylight_${kind}_v${TOKEN_VERSION}`;
}

function parseClientEnvelope(value: unknown): ClientEnvelope | undefined {
  if (!isRecord(value) || value.kind !== CLIENT_TOKEN_KIND) {
    return undefined;
  }
  if (!isInteger(value.issuedAt) || !isRecord(value.metadata)) {
    return undefined;
  }
  return {
    kind: CLIENT_TOKEN_KIND,
    issuedAt: value.issuedAt,
    metadata: value.metadata,
  };
}

function parseAuthorizationCodeEnvelope(value: unknown): AuthorizationCodeEnvelope | undefined {
  if (!isRecord(value) || value.kind !== AUTHORIZATION_CODE_KIND) {
    return undefined;
  }
  const scopes = parseScopes(value.scopes);
  if (
    !isString(value.instanceId) ||
    !isString(value.id) ||
    !isInteger(value.expiresAt) ||
    !isString(value.upstreamToken) ||
    !isString(value.clientId) ||
    !scopes ||
    !isString(value.resource) ||
    !isString(value.redirectUri) ||
    !isString(value.codeChallenge)
  ) {
    return undefined;
  }
  return {
    kind: AUTHORIZATION_CODE_KIND,
    instanceId: value.instanceId,
    id: value.id,
    expiresAt: value.expiresAt,
    upstreamToken: value.upstreamToken,
    clientId: value.clientId,
    scopes,
    resource: value.resource,
    redirectUri: value.redirectUri,
    codeChallenge: value.codeChallenge,
  };
}

function parseOAuthTokenEnvelope(
  value: unknown,
  kind: typeof ACCESS_TOKEN_KIND | typeof REFRESH_TOKEN_KIND,
): OAuthTokenEnvelope | undefined {
  if (!isRecord(value) || value.kind !== kind) {
    return undefined;
  }
  const scopes = parseScopes(value.scopes);
  if (
    !isString(value.id) ||
    !isInteger(value.expiresAt) ||
    !isString(value.upstreamToken) ||
    !isString(value.clientId) ||
    !scopes ||
    !isString(value.resource)
  ) {
    return undefined;
  }
  return {
    kind,
    id: value.id,
    expiresAt: value.expiresAt,
    upstreamToken: value.upstreamToken,
    clientId: value.clientId,
    scopes,
    resource: value.resource,
  };
}

function validateRegisteredClient(client: OAuthClientInformationFull): void {
  if (client.redirect_uris.length === 0) {
    throw new InvalidClientMetadataError("At least one redirect URI is required");
  }
  for (const redirectUri of client.redirect_uris) {
    if (!isAllowedRedirectUri(redirectUri)) {
      throw new InvalidClientMetadataError(
        "redirect_uris must use HTTPS or loopback HTTP and must not contain credentials or fragments",
      );
    }
  }

  const authenticationMethod = client.token_endpoint_auth_method ?? "client_secret_post";
  if (authenticationMethod === "none") {
    if (client.client_secret !== undefined) {
      throw new InvalidClientMetadataError("Public clients must not have a client secret");
    }
    return;
  }
  if (authenticationMethod !== "client_secret_post") {
    throw new InvalidClientMetadataError("Only none and client_secret_post are supported");
  }
  if (!client.client_secret) {
    throw new InvalidClientMetadataError("Confidential clients require a generated client secret");
  }
}

function validateResourceUrl(value: string | URL): URL {
  const resource = new URL(value.toString());
  if (
    resource.protocol !== "https:" ||
    resource.pathname !== "/mcp" ||
    resource.username !== "" ||
    resource.password !== "" ||
    resource.search !== "" ||
    resource.hash !== ""
  ) {
    throw new Error("OAuth resourceUrl must be an HTTPS URL with the exact /mcp path");
  }
  return resource;
}

function validateSupportedScopes(scopes: readonly string[]): readonly string[] {
  const unique = uniqueScopes(scopes);
  if (unique.length === 0) {
    throw new Error("At least one OAuth scope must be supported");
  }
  return Object.freeze(unique);
}

function validateTtl(name: string, value: number): number {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`${name} must be a positive integer`);
  }
  return value;
}

function validateNarrowedScopes(requested: readonly string[], granted: readonly string[]): string[] {
  const narrowed = uniqueScopes(requested);
  if (narrowed.length === 0) {
    throw new InvalidScopeError("Refresh scope must not be empty");
  }
  const allowed = new Set(granted);
  if (narrowed.some((scope) => !allowed.has(scope))) {
    throw new InvalidScopeError("Refresh scopes may only narrow the original grant");
  }
  return narrowed;
}

function uniqueScopes(scopes: readonly string[]): string[] {
  const unique: string[] = [];
  const seen = new Set<string>();
  for (const scope of scopes) {
    if (!scope || /[\s\x00-\x1f\x7f]/u.test(scope)) {
      throw new InvalidScopeError("OAuth scopes must be non-empty tokens without whitespace");
    }
    if (!seen.has(scope)) {
      seen.add(scope);
      unique.push(scope);
    }
  }
  return unique;
}

function parseScopes(value: unknown): string[] | undefined {
  if (!Array.isArray(value) || value.some((scope) => typeof scope !== "string")) {
    return undefined;
  }
  try {
    return uniqueScopes(value as string[]);
  } catch {
    return undefined;
  }
}

function isAllowedRedirectUri(value: string): boolean {
  try {
    const uri = new URL(value);
    if (uri.username !== "" || uri.password !== "" || uri.hash !== "") {
      return false;
    }
    if (uri.protocol === "https:") {
      return true;
    }
    return uri.protocol === "http:" && isLoopbackHostname(uri.hostname);
  } catch {
    return false;
  }
}

function redirectUriMatches(requested: string, registered: readonly string[]): boolean {
  if (registered.includes(requested)) {
    return true;
  }
  try {
    const requestedUrl = new URL(requested);
    return registered.some((registeredValue) => {
      const registeredUrl = new URL(registeredValue);
      return requestedUrl.protocol === "http:" &&
        registeredUrl.protocol === "http:" &&
        isLoopbackHostname(requestedUrl.hostname) &&
        requestedUrl.hostname === registeredUrl.hostname &&
        requestedUrl.pathname === registeredUrl.pathname &&
        requestedUrl.search === registeredUrl.search &&
        requestedUrl.hash === registeredUrl.hash;
    });
  } catch {
    return false;
  }
}

function isLoopbackHostname(hostname: string): boolean {
  const normalized = hostname.toLowerCase();
  if (normalized === "localhost" || normalized === "[::1]" || normalized === "::1") {
    return true;
  }
  const octets = normalized.split(".");
  return octets.length === 4 &&
    octets[0] === "127" &&
    octets.every((octet) => /^\d{1,3}$/u.test(octet) && Number(octet) <= 255);
}

function extractSubmittedToken(body: unknown): string | undefined {
  if (!isRecord(body) || typeof body.straylight_token !== "string") {
    return undefined;
  }
  const token = body.straylight_token.trim();
  return token === "" || token.length > MAX_UPSTREAM_TOKEN_LENGTH ? undefined : token;
}

function renderApprovalForm(
  client: OAuthClientInformationFull,
  params: AuthorizationParams,
  scopes: readonly string[],
): string {
  const hiddenFields: Array<[string, string]> = [
    ["client_id", client.client_id],
    ["redirect_uri", params.redirectUri],
    ["response_type", "code"],
    ["code_challenge", params.codeChallenge],
    ["code_challenge_method", "S256"],
    ["scope", scopes.join(" ")],
    ["resource", params.resource?.href ?? ""],
  ];
  if (params.state !== undefined) {
    hiddenFields.push(["state", params.state]);
  }
  const inputs = hiddenFields
    .map(([name, value]) => `<input type="hidden" name="${escapeHtml(name)}" value="${escapeHtml(value)}">`)
    .join("\n");
  const clientName = client.client_name ?? "this application";
  const redirectOrigin = new URL(params.redirectUri).origin;
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Connect Straylight</title>
  <style>
    :root { color-scheme: light dark; font-family: system-ui, sans-serif; }
    body { margin: 0; padding: 2rem 1rem; background: Canvas; color: CanvasText; }
    main { max-width: 34rem; margin: 0 auto; }
    label { display: block; margin: 1.25rem 0 .4rem; font-weight: 650; }
    input[type=password] { box-sizing: border-box; width: 100%; padding: .75rem; font: inherit; }
    button { margin-top: 1rem; padding: .7rem 1rem; font: inherit; font-weight: 650; }
    .scope { overflow-wrap: anywhere; opacity: .8; }
  </style>
</head>
<body>
  <main>
    <h1>Connect Straylight</h1>
    <p><strong>${escapeHtml(clientName)}</strong> is requesting access to your hosted Straylight workspace.</p>
    <p class="scope">Requested access: ${escapeHtml(scopes.join(", "))}</p>
    <p>After approval, Straylight will return control to <strong>${escapeHtml(redirectOrigin)}</strong>.</p>
    <p>Use a dedicated root-scoped read/write credential without owner or credential-management access.</p>
    <form method="post" action="" autocomplete="off">
${inputs}
      <label for="straylight-token">Straylight credential</label>
      <input id="straylight-token" name="straylight_token" type="password" required autofocus autocomplete="off" spellcheck="false">
      <button type="submit">Approve connection</button>
    </form>
  </main>
</body>
</html>`;
}

function setAuthorizationResponseHeaders(res: OAuthResponse, redirectUri: string): void {
  const redirectOrigin = new URL(redirectUri).origin;
  res.setHeader("Cache-Control", "no-store");
  res.setHeader("Pragma", "no-cache");
  res.setHeader(
    "Content-Security-Policy",
    `default-src 'none'; style-src 'unsafe-inline'; form-action 'self' ${redirectOrigin}; base-uri 'none'; frame-ancestors 'none'`,
  );
  res.setHeader("X-Frame-Options", "DENY");
  res.setHeader("Referrer-Policy", "no-referrer");
  res.setHeader("X-Content-Type-Options", "nosniff");
  res.setHeader("Permissions-Policy", "camera=(), microphone=(), geolocation=()");
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/gu, (character) => {
    switch (character) {
      case "&": return "&amp;";
      case "<": return "&lt;";
      case ">": return "&gt;";
      case "\"": return "&quot;";
      case "'": return "&#39;";
      default: return character;
    }
  });
}

function pkceVerifierMatches(verifier: string, challenge: string): boolean {
  const calculated = createHash("sha256").update(verifier, "ascii").digest("base64url");
  const calculatedBytes = Buffer.from(calculated, "utf8");
  const challengeBytes = Buffer.from(challenge, "utf8");
  return calculatedBytes.byteLength === challengeBytes.byteLength &&
    timingSafeEqual(calculatedBytes, challengeBytes);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isString(value: unknown): value is string {
  return typeof value === "string" && value !== "";
}

function isInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}
