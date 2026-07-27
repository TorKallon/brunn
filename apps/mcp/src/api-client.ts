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

const MAX_STAGE_FILES = 2_000;
const MAX_STAGE_BYTES = 64 * 1024 * 1024;
const DEFAULT_REQUEST_TIMEOUT_MS = 30_000;
const DEFAULT_TRANSFER_TIMEOUT_MS = 15 * 60_000;
const MAX_TIMEOUT_MS = 2_147_483_647;

export interface ApiClientTimeouts {
  requestMs?: number;
  transferMs?: number;
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
  }

  async request(path: string, body?: unknown): Promise<ApiResponse> {
    const started = performance.now();
    const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
      method: body === undefined ? "GET" : "POST",
      headers: {
        accept: "application/json",
        authorization: `Bearer ${this.token}`,
        ...this.requestHeaders,
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(this.requestTimeoutMs),
    });
    const parsed = await parseJson(response);
    if (!response.ok) {
      throw new StraylightApiError(response.status, parsed);
    }
    return {
      status: response.status,
      body: parsed,
      elapsedMs: performance.now() - started,
    };
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
      if (!response.ok) {
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
  const text = await response.text();
  if (!text) {
    return {};
  }
  try {
    const value: unknown = JSON.parse(text);
    return typeof value === "object" && value !== null
      ? value as Record<string, unknown>
      : { data: value };
  } catch {
    return { error: { code: "invalid_upstream_response", message: text.slice(0, 2_000) } };
  }
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
