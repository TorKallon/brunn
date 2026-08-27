import { createHash } from "node:crypto";
import { readFile, realpath, stat } from "node:fs/promises";
import { basename, relative, resolve } from "node:path";

import {
  configuredAssetRoot,
  parseAssetMetadata,
  storeVerifiedAsset,
} from "./asset-download.js";

export interface ApiResponse {
  status: number;
  body: Record<string, unknown>;
  elapsedMs: number;
}

export type ApiHttpMethod = "GET" | "POST" | "PATCH" | "PUT";

const MAX_STAGE_FILES = 2_000;
const MAX_STAGE_BYTES = 64 * 1024 * 1024;
const DEFAULT_REQUEST_TIMEOUT_MS = 30_000;
const DEFAULT_TRANSFER_TIMEOUT_MS = 15 * 60_000;
const MAX_TIMEOUT_MS = 2_147_483_647;
// workspace.read caps the complete response at four million characters. 32 MiB
// leaves room for four-byte UTF-8 and JSON escaping while preventing a broken
// upstream or proxy from exhausting the adapter process.
const MAX_JSON_RESPONSE_BYTES = 32 * 1024 * 1024;
// Six waits bridge short Railway restarts and rolling deploys while the absolute
// request deadline remains the final bound. Including the initial request, the
// production policy makes at most seven attempts over 17 seconds of backoff.
const DEFAULT_RETRY_BACKOFF_MS = [100, 400, 1_000, 2_500, 5_000, 8_000] as const;
const RETRY_BACKOFF_ENVIRONMENT = "STRAYLIGHT_MCP_RETRY_BACKOFF_MS";
const MAX_RETRY_BACKOFFS = DEFAULT_RETRY_BACKOFF_MS.length;
const TRANSIENT_HTTP_STATUSES = new Set([502, 503, 504]);
const TRANSIENT_NETWORK_CODES = new Set([
  "ECONNREFUSED",
  "ECONNRESET",
  "EHOSTUNREACH",
  "ENETDOWN",
  "ENETRESET",
  "ENETUNREACH",
  "EPIPE",
  "ETIMEDOUT",
  "EAI_AGAIN",
  "UND_ERR_CONNECT_TIMEOUT",
  "UND_ERR_HEADERS_TIMEOUT",
  "UND_ERR_SOCKET",
]);
const MESSAGING_CLIENT_KEY = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/u;
const READ_ONLY_POST_PATHS = new Set([
  "/v1/memory/compute",
  "/v1/memory/open",
  "/v1/memory/query",
  "/v1/memory/read",
  "/v1/memory/verify",
  "/v1/workspace/briefings/dedupe-check",
  "/v1/workspace/open",
  "/v1/workspace/read",
  "/v1/workspace/search",
  "/v1/workspace/secrets/get",
]);
const IDEMPOTENCY_KEY_MUTATION_PATHS = new Set([
  "/v1/memory/capture",
  "/v1/memory/checkpoint",
  "/v1/memory/write",
  "/v1/workspace/briefings/publish",
  "/v1/workspace/capture",
  "/v1/workspace/checkpoint",
  "/v1/workspace/contexts",
  "/v1/workspace/contexts/merge",
  "/v1/workspace/tasks/capture",
  "/v1/workspace/tasks/settings",
  "/v1/workspace/write",
]);

export interface ApiClientTimeouts {
  requestMs?: number;
  transferMs?: number;
  retryBackoffMs?: readonly number[];
}

export class StraylightApiError extends Error {
  constructor(
    readonly status: number,
    readonly body: Record<string, unknown>,
  ) {
    const detail = body.error;
    super(
      typeof detail === "object" && detail !== null && "message" in detail
        ? String(detail.message)
        : `Straylight API returned HTTP ${status}`,
    );
    this.name = "StraylightApiError";
  }
}

export class StraylightApiClient {
  private readonly baseUrl: string;
  private readonly requestTimeoutMs: number;
  private readonly transferTimeoutMs: number;
  private readonly retryBackoffMs: readonly number[];

  constructor(
    baseUrl: string,
    private readonly token: string,
    private readonly fetchImpl: typeof fetch = fetch,
    private readonly requestHeaders: Record<string, string> = {},
    private readonly assetRoot?: string,
    timeouts: ApiClientTimeouts = {},
  ) {
    this.baseUrl = baseUrl.replace(/\/$/, "");
    this.requestTimeoutMs = configuredTimeout(
      timeouts.requestMs,
      "STRAYLIGHT_MCP_REQUEST_TIMEOUT_MS",
      DEFAULT_REQUEST_TIMEOUT_MS,
    );
    this.transferTimeoutMs = configuredTimeout(
      timeouts.transferMs,
      "STRAYLIGHT_MCP_TRANSFER_TIMEOUT_MS",
      DEFAULT_TRANSFER_TIMEOUT_MS,
    );
    this.retryBackoffMs = configuredRetryBackoff(timeouts.retryBackoffMs);
  }

  async request(path: string, body?: unknown): Promise<ApiResponse>;
  async request(method: ApiHttpMethod, path: string, body?: unknown): Promise<ApiResponse>;
  async request(
    methodOrPath: ApiHttpMethod | string,
    pathOrBody?: unknown,
    explicitBody?: unknown,
  ): Promise<ApiResponse> {
    const explicitMethod = isApiHttpMethod(methodOrPath);
    const method: ApiHttpMethod = explicitMethod
      ? methodOrPath
      : pathOrBody === undefined ? "GET" : "POST";
    const path = explicitMethod
      ? typeof pathOrBody === "string"
        ? pathOrBody
        : undefined
      : methodOrPath;
    if (path === undefined || !path.startsWith("/")) {
      throw new TypeError("Straylight API request path must start with /");
    }
    const body = explicitMethod ? explicitBody : pathOrBody;
    if (method === "GET" && body !== undefined) {
      throw new TypeError("Straylight API GET requests cannot include a body");
    }
    const started = performance.now();
    const deadline = started + this.requestTimeoutMs;
    const serializedBody = body === undefined ? undefined : JSON.stringify(body);
    const policy = requestRetryPolicy(method, path, body);
    let attempts = 0;
    let lastRequestId: string | undefined;

    while (true) {
      const remainingMs = deadline - performance.now();
      if (remainingMs <= 0) {
        throw exhaustedTransientError(policy, attempts, lastRequestId, 503);
      }
      attempts += 1;
      let transientStatus = 503;
      try {
        const result = await this.jsonAttempt(
          method,
          path,
          serializedBody,
          Math.max(1, Math.ceil(remainingMs)),
        );
        lastRequestId = result.requestId ?? lastRequestId;
        if (result.response.ok && result.structured) {
          return {
            status: result.response.status,
            body: result.body,
            elapsedMs: performance.now() - started,
          };
        }
        const transientResponse = isTransientResponse(
          result.response.status,
          result.railwayApplicationNotFound,
        );
        if (!transientResponse && result.structured) {
          throw new StraylightApiError(result.response.status, result.body);
        }
        if (!transientResponse && !result.structured && !result.response.ok) {
          throw new StraylightApiError(
            result.response.status,
            invalidUpstreamResponse(result.response.status, result.requestId),
          );
        }
        transientStatus = normalizeTransientStatus(result.response.status);
      } catch (error) {
        if (error instanceof StraylightApiError) {
          throw error;
        }
        if (error instanceof JsonAttemptError) {
          lastRequestId = error.requestId ?? lastRequestId;
          transientStatus = normalizeTransientStatus(error.responseStatus);
        } else {
          if (!isTransientNetworkError(error)) {
            throw error;
          }
          transientStatus = 503;
        }
      }

      if (!policy.retryable || attempts > this.retryBackoffMs.length) {
        throw exhaustedTransientError(policy, attempts, lastRequestId, transientStatus);
      }
      const backoffMs = this.retryBackoffMs[attempts - 1];
      if (backoffMs === undefined || !await waitForRetry(backoffMs, deadline)) {
        throw exhaustedTransientError(policy, attempts, lastRequestId, transientStatus);
      }
    }
  }

  private async jsonAttempt(
    method: ApiHttpMethod,
    path: string,
    serializedBody: string | undefined,
    timeoutMs: number,
  ): Promise<{
    response: Response;
    body: Record<string, unknown>;
    structured: boolean;
    railwayApplicationNotFound: boolean;
    requestId: string | undefined;
  }> {
    const controller = new AbortController();
    let rejectDeadline: ((reason: unknown) => void) | undefined;
    const deadline = new Promise<never>((_resolve, reject) => {
      rejectDeadline = reject;
    });
    const timer = setTimeout(() => {
      const error = new DOMException("Straylight request deadline exceeded", "TimeoutError");
      controller.abort(error);
      rejectDeadline?.(error);
    }, timeoutMs);
    try {
      const response = await Promise.race([
        this.fetchImpl(`${this.baseUrl}${path}`, {
          method,
          headers: {
            accept: "application/json",
            authorization: `Bearer ${this.token}`,
            ...this.requestHeaders,
            ...(serializedBody === undefined ? {} : { "content-type": "application/json" }),
          },
          ...(serializedBody === undefined ? {} : { body: serializedBody }),
          signal: controller.signal,
        }),
        deadline,
      ]);
      const headerRequestId = responseRequestId(response.headers);
      let rawText: string;
      try {
        rawText = await readBoundedResponseText(response, deadline);
      } catch (error) {
        throw new JsonAttemptError(headerRequestId, response.status, error);
      }
      const parsed = parseJsonText(rawText);
      return {
        response,
        body: parsed.body,
        structured: parsed.structured,
        railwayApplicationNotFound: !parsed.structured
          && rawText.trim().toLowerCase() === "application not found",
        requestId: bodyRequestId(parsed.body) ?? headerRequestId,
      };
    } catch (error) {
      if (!controller.signal.aborted) {
        controller.abort(error);
      }
      throw error;
    } finally {
      clearTimeout(timer);
    }
  }

  async assetMetadata(
    assetRef: string,
    sessionId: string,
    requestedVersion?: number,
  ): Promise<ApiResponse> {
    void sessionId;
    return this.request(
      `/v1/workspace/binaries/${encodeURIComponent(assetRef)}`
      + binaryVersionQuery(requestedVersion),
    );
  }

  async listAssets(
    sessionId: string,
    offset = 0,
    limit = 100,
  ): Promise<ApiResponse> {
    void sessionId;
    const query = new URLSearchParams({
      offset: String(offset),
      limit: String(limit),
    });
    return this.request(`/v1/workspace/binaries?${query.toString()}`);
  }

  async workspaceChanges(
    sinceGeneration = 0,
    limit = 200,
  ): Promise<ApiResponse> {
    const query = new URLSearchParams({
      since_generation: String(sinceGeneration),
      limit: String(limit),
    });
    return this.request(`/v1/workspace/changes?${query.toString()}`);
  }

  async fetchAsset(
    assetRef: string,
    sessionId: string,
    requestedVersion?: number,
  ): Promise<ApiResponse> {
    const started = performance.now();
    let metadataResponse: ApiResponse;
    try {
      metadataResponse = await this.assetMetadata(assetRef, sessionId, requestedVersion);
    } catch (error) {
      if (error instanceof StraylightApiError) {
        if (isResilienceFailure(error.body)) {
          throw error;
        }
        throw new StraylightApiError(
          error.status,
          assetFailure("metadata", error.status),
        );
      }
      throw error;
    }
    const metadata = parseAssetMetadata(metadataResponse.body, assetRef);
    if (
      requestedVersion !== undefined
      && requestedVersion !== metadata.version
    ) {
      throw new Error(
        `asset metadata returned version ${metadata.version} for requested version ${requestedVersion}`,
      );
    }
    const response = await this.fetchImpl(
      `${this.baseUrl}/v1/workspace/binaries/${encodeURIComponent(assetRef)}/content`
      + binaryVersionQuery(requestedVersion),
      {
        method: "GET",
        headers: {
          accept: "*/*",
          "accept-encoding": "identity",
          authorization: `Bearer ${this.token}`,
          ...this.requestHeaders,
        },
        redirect: "error",
        signal: AbortSignal.timeout(this.transferTimeoutMs),
      },
    );
    if (!response.ok) {
      await response.body?.cancel().catch(() => undefined);
      throw new StraylightApiError(
        response.status,
        assetFailure("download", response.status),
      );
    }
    const body = await storeVerifiedAsset(
      response,
      metadata,
      this.assetRoot ?? configuredAssetRoot(),
    );
    return {
      status: response.status,
      body,
      elapsedMs: performance.now() - started,
    };
  }

  async stage(
    scope: string,
    stableImportId: string | undefined,
    files: Array<{ path: string; name?: string | undefined; media_type?: string | undefined }>,
    describeBinaries = true,
  ): Promise<ApiResponse> {
    const started = performance.now();
    const importRoot = await realpath(
      process.env.STRAYLIGHT_MCP_IMPORT_ROOT ?? "/imports",
    );
    if (files.length > MAX_STAGE_FILES) {
      throw new Error(`staging is limited to ${MAX_STAGE_FILES} files per request`);
    }
    const resolvedFiles = [];
    let totalBytes = 0;
    for (const file of files) {
      const filePath = await realpath(resolve(importRoot, file.path));
      const insideRoot = relative(importRoot, filePath);
      if (insideRoot.startsWith("..") || insideRoot.includes("/../")) {
        throw new Error("staged paths must remain inside STRAYLIGHT_MCP_IMPORT_ROOT");
      }
      const metadata = await stat(filePath);
      if (!metadata.isFile()) {
        throw new Error(`staged path is not a regular file: ${file.path}`);
      }
      if (metadata.size > MAX_STAGE_BYTES) {
        throw new Error(`staged files are limited to ${MAX_STAGE_BYTES} bytes each`);
      }
      totalBytes += metadata.size;
      if (totalBytes > MAX_STAGE_BYTES) {
        throw new Error(`staged requests are limited to ${MAX_STAGE_BYTES} bytes`);
      }
      resolvedFiles.push({ file, filePath });
    }
    const uploads: Record<string, unknown>[] = [];
    let status = 200;
    for (const { file, filePath } of resolvedFiles) {
      const form = new FormData();
      const bytes = await readFile(filePath);
      const contentHash = createHash("sha256").update(bytes).digest("hex");
      const logicalPath = file.name ?? file.path.replaceAll("\\", "/");
      form.set("path", logicalPath);
      form.set("media_type", file.media_type ?? "application/octet-stream");
      form.set("expected_content_hash", `sha256:${contentHash}`);
      if (describeBinaries) {
        form.set(
          "limitations",
          "The immutable binary bytes are authoritative; content-specific description may still be pending.",
        );
      }
      form.set(
        "file",
        new Blob([bytes], { type: file.media_type ?? "application/octet-stream" }),
        basename(filePath),
      );
      const idempotencyKey = stableImportId === undefined
        ? undefined
        : stageIdempotencyKey(stableImportId, scope, logicalPath);
      const response = await this.fetchImpl(`${this.baseUrl}/v1/workspace/binaries`, {
        method: "POST",
        headers: {
          accept: "application/json",
          authorization: `Bearer ${this.token}`,
          ...this.requestHeaders,
          ...(idempotencyKey === undefined ? {} : { "idempotency-key": idempotencyKey }),
        },
        body: form,
        signal: AbortSignal.timeout(this.transferTimeoutMs),
      });
      const parsed = await parseJson(response);
      if (!response.ok || isInvalidUpstreamResponse(parsed)) {
        throw new StraylightApiError(response.status, parsed);
      }
      status = response.status;
      uploads.push(parsed);
    }
    return {
      status,
      body: { status: "complete", data: { uploads } },
      elapsedMs: performance.now() - started,
    };
  }
}

async function parseJson(response: Response): Promise<Record<string, unknown>> {
  let text: string;
  try {
    text = await readBoundedResponseText(response);
  } catch {
    return invalidUpstreamResponse(response.status, responseRequestId(response.headers));
  }
  const parsed = parseJsonText(text);
  return parsed.structured
    ? parsed.body
    : invalidUpstreamResponse(response.status, responseRequestId(response.headers));
}

function parseJsonText(text: string): ParsedJsonText {
  if (!text) {
    return { body: {}, structured: false };
  }
  try {
    const value: unknown = JSON.parse(text);
    return {
      body: typeof value === "object" && value !== null
      ? value as Record<string, unknown>
      : { data: value },
      structured: true,
    };
  } catch {
    return { body: {}, structured: false };
  }
}

async function readBoundedResponseText(
  response: Response,
  deadline?: Promise<never>,
): Promise<string> {
  const contentLength = response.headers.get("content-length");
  if (/^\d+$/.test(contentLength ?? "")) {
    const declaredBytes = BigInt(contentLength ?? "0");
    if (declaredBytes > BigInt(MAX_JSON_RESPONSE_BYTES)) {
      void response.body?.cancel().catch(() => undefined);
      throw new ResponseTooLargeError();
    }
  }
  if (response.body === null) {
    return "";
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  const textParts: string[] = [];
  let totalBytes = 0;
  try {
    while (true) {
      const read = reader.read();
      const result = deadline === undefined
        ? await read
        : await Promise.race([read, deadline]);
      if (result.done) {
        break;
      }
      if (result.value.byteLength > MAX_JSON_RESPONSE_BYTES - totalBytes) {
        throw new ResponseTooLargeError();
      }
      totalBytes += result.value.byteLength;
      textParts.push(decoder.decode(result.value, { stream: true }));
    }
    textParts.push(decoder.decode());
    return textParts.join("");
  } catch (error) {
    void reader.cancel(error).catch(() => undefined);
    throw error;
  } finally {
    try {
      reader.releaseLock();
    } catch {
      // A synthetic or non-conforming stream may retain a pending read while
      // cancellation propagates. The request AbortController is also aborted
      // by jsonAttempt, so never let lock cleanup replace the real failure.
    }
  }
}

function invalidUpstreamResponse(
  status: number,
  requestId: string | undefined,
): Record<string, unknown> {
  return {
    ...(requestId === undefined ? {} : { request_id: requestId }),
    error: {
      code: "invalid_upstream_response",
      message: `Straylight upstream returned an invalid JSON response (HTTP ${status})`,
    },
  };
}

interface RequestRetryPolicy {
  mutation: boolean;
  retryable: boolean;
}

interface ParsedJsonText {
  body: Record<string, unknown>;
  structured: boolean;
}

class JsonAttemptError extends Error {
  constructor(
    readonly requestId: string | undefined,
    readonly responseStatus: number,
    cause: unknown,
  ) {
    super("Straylight upstream response could not be read safely", { cause });
    this.name = "JsonAttemptError";
  }
}

class ResponseTooLargeError extends Error {
  constructor() {
    super(`Straylight upstream response exceeded ${MAX_JSON_RESPONSE_BYTES} bytes`);
    this.name = "ResponseTooLargeError";
  }
}

function requestRetryPolicy(
  method: ApiHttpMethod,
  path: string,
  body: unknown,
): RequestRetryPolicy {
  const requestPath = normalizedPath(path);
  if (method === "GET" || (method === "POST" && READ_ONLY_POST_PATHS.has(requestPath))) {
    return { mutation: false, retryable: true };
  }
  const record = typeof body === "object" && body !== null
    ? body as Record<string, unknown>
    : {};
  const hasIdempotencyKey = supportsIdempotencyKey(requestPath)
    && validIdempotencyIdentity(record.idempotency_key);
  const notificationIdentity = requestPath === "/v1/workspace/notifications/publish"
    && validIdempotencyIdentity(record.event_key);
  const messagingIdentity = method === "POST"
    && /^\/v1\/workspace\/messaging\/conversations\/[0-9a-f-]{36}\/messages$/iu.test(
      requestPath,
    )
    && typeof record.client_key === "string"
    && MESSAGING_CLIENT_KEY.test(record.client_key);
  return {
    mutation: true,
    retryable: hasIdempotencyKey || notificationIdentity || messagingIdentity,
  };
}

function isApiHttpMethod(value: string): value is ApiHttpMethod {
  return value === "GET" || value === "POST" || value === "PATCH" || value === "PUT";
}

function supportsIdempotencyKey(requestPath: string): boolean {
  if (IDEMPOTENCY_KEY_MUTATION_PATHS.has(requestPath)) {
    return true;
  }
  return /^\/v1\/workspace\/tasks\/[0-9a-f-]{36}$/u.test(requestPath)
    || /^\/v1\/workspace\/contexts\/(?:available\/)?[a-z0-9]+(?:[._-][a-z0-9]+)*$/u
      .test(requestPath)
    || /^\/v1\/workspace\/projects\/[a-z0-9]+(?:-[a-z0-9]+)*(?:\/interest)?$/u
      .test(requestPath);
}

function normalizedPath(path: string): string {
  const withoutQuery = path.split("?", 1)[0] ?? path;
  return withoutQuery.length > 1 ? withoutQuery.replace(/\/+$/, "") : withoutQuery;
}

function validIdempotencyIdentity(value: unknown): boolean {
  return typeof value === "string"
    && value.length > 0
    && Buffer.byteLength(value, "utf8") <= 256
    && !/[\u0000-\u001f\u007f-\u009f]/u.test(value);
}

function isTransientResponse(status: number, railwayApplicationNotFound: boolean): boolean {
  return TRANSIENT_HTTP_STATUSES.has(status)
    || (status === 404 && railwayApplicationNotFound);
}

function normalizeTransientStatus(status: number): number {
  return TRANSIENT_HTTP_STATUSES.has(status) ? status : 503;
}

function isTransientNetworkError(error: unknown): boolean {
  if (error instanceof DOMException) {
    return error.name === "AbortError" || error.name === "TimeoutError";
  }
  if (error instanceof TypeError) {
    return true;
  }
  let candidate: unknown = error;
  for (let depth = 0; depth < 4 && typeof candidate === "object" && candidate !== null; depth += 1) {
    const record = candidate as { code?: unknown; cause?: unknown };
    if (typeof record.code === "string" && TRANSIENT_NETWORK_CODES.has(record.code)) {
      return true;
    }
    candidate = record.cause;
  }
  return false;
}

async function waitForRetry(delayMs: number, deadline: number): Promise<boolean> {
  const remainingMs = deadline - performance.now();
  if (remainingMs <= delayMs) {
    return false;
  }
  await new Promise<void>((resolve) => setTimeout(resolve, delayMs));
  return performance.now() < deadline;
}

function exhaustedTransientError(
  policy: RequestRetryPolicy,
  attempts: number,
  requestId: string | undefined,
  status: number,
): StraylightApiError {
  const error = policy.mutation
    ? policy.retryable
      ? {
          code: "ambiguous_outcome",
          message:
            "Straylight could not confirm this idempotent mutation after bounded transient retries. "
            + "It may already have committed. Replay the identical request with the identical "
            + "idempotency key or event identity to recover the durable receipt; do not mint a new key.",
          outcome: "unknown",
          retryable: true,
          attempts,
        }
      : {
          code: "ambiguous_outcome",
          message:
            "Straylight could not confirm this mutation. It may already have committed, and the "
            + "request had no safe idempotency identity, so it was not retried automatically. "
            + "Confirm durable state before attempting another mutation.",
          outcome: "unknown",
          retryable: false,
          attempts,
        }
    : {
        code: "upstream_unavailable",
        message:
          "Straylight is temporarily unavailable after bounded attempts. Retry the same read request.",
        retryable: true,
        attempts,
      };
  return new StraylightApiError(status, {
    ...(requestId === undefined ? {} : { request_id: requestId }),
    error,
  });
}

function responseRequestId(headers: Headers): string | undefined {
  for (const name of ["x-request-id", "x-railway-request-id", "railway-request-id"]) {
    const value = safeRequestId(headers.get(name));
    if (value !== undefined) {
      return value;
    }
  }
  return undefined;
}

function bodyRequestId(body: Record<string, unknown>): string | undefined {
  return safeRequestId(body.request_id);
}

function safeRequestId(value: unknown): string | undefined {
  return typeof value === "string"
    && value.length > 0
    && value.length <= 512
    && !/[\u0000-\u001f\u007f-\u009f]/u.test(value)
    ? value
    : undefined;
}

function isResilienceFailure(body: Record<string, unknown>): boolean {
  const detail = body.error;
  if (typeof detail !== "object" || detail === null || !("code" in detail)) {
    return false;
  }
  return detail.code === "upstream_unavailable" || detail.code === "ambiguous_outcome";
}

function isInvalidUpstreamResponse(body: Record<string, unknown>): boolean {
  const detail = body.error;
  return typeof detail === "object"
    && detail !== null
    && "code" in detail
    && detail.code === "invalid_upstream_response";
}

function assetFailure(
  stage: "metadata" | "download",
  status: number,
): Record<string, unknown> {
  return {
    error: {
      code: `asset_${stage}_failed`,
      message: `CarryState asset ${stage} request returned HTTP ${status}`,
    },
  };
}

function binaryVersionQuery(version: number | undefined): string {
  return version === undefined
    ? ""
    : `?${new URLSearchParams({ version: String(version) }).toString()}`;
}

function stageIdempotencyKey(
  stableImportId: string,
  scope: string,
  logicalPath: string,
): string {
  const digest = createHash("sha256")
    .update(stableImportId)
    .update("\0")
    .update(scope)
    .update("\0")
    .update(logicalPath)
    .digest("hex");
  return `stage:${digest}`;
}

function configuredTimeout(
  explicit: number | undefined,
  environmentName: string,
  fallback: number,
): number {
  const environmentValue = process.env[environmentName];
  const value = explicit
    ?? (environmentValue === undefined ? fallback : Number(environmentValue));
  if (!Number.isSafeInteger(value) || value <= 0 || value > MAX_TIMEOUT_MS) {
    throw new Error(
      `${environmentName} must be a positive integer no greater than ${MAX_TIMEOUT_MS}`,
    );
  }
  return value;
}

function configuredRetryBackoff(explicit: readonly number[] | undefined): readonly number[] {
  const environmentValue = process.env[RETRY_BACKOFF_ENVIRONMENT];
  const schedule = explicit
    ?? (environmentValue === undefined
      ? DEFAULT_RETRY_BACKOFF_MS
      : parseRetryBackoffEnvironment(environmentValue));
  if (schedule.length > MAX_RETRY_BACKOFFS) {
    throw new Error(
      `${RETRY_BACKOFF_ENVIRONMENT} must contain no more than ${MAX_RETRY_BACKOFFS} delays`,
    );
  }
  for (const delayMs of schedule) {
    if (!Number.isSafeInteger(delayMs) || delayMs < 0 || delayMs > MAX_TIMEOUT_MS) {
      throw new Error(
        `${RETRY_BACKOFF_ENVIRONMENT} delays must be non-negative integers no greater than `
        + MAX_TIMEOUT_MS,
      );
    }
  }
  return Object.freeze([...schedule]);
}

function parseRetryBackoffEnvironment(value: string): readonly number[] {
  const values = value.split(",");
  if (values.length === 0 || values.some((candidate) => !/^\d+$/.test(candidate))) {
    throw new Error(
      `${RETRY_BACKOFF_ENVIRONMENT} must be a comma-separated list of non-negative integers`,
    );
  }
  return values.map((candidate) => Number(candidate));
}
