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

  constructor(
    baseUrl: string,
    private readonly token: string,
    private readonly fetchImpl: typeof fetch = fetch,
    private readonly requestHeaders: Record<string, string> = {},
    private readonly assetRoot?: string,
  ) {
    this.baseUrl = baseUrl.replace(/\/$/, "");
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

  async assetMetadata(assetRef: string, sessionId: string): Promise<ApiResponse> {
    return this.request(
      `/v1/assets/${encodeURIComponent(assetRef)}?${sessionQuery(sessionId)}`,
    );
  }

  async listAssets(
    sessionId: string,
    offset = 0,
    limit = 100,
  ): Promise<ApiResponse> {
    const query = new URLSearchParams({
      session_id: sessionId,
      offset: String(offset),
      limit: String(limit),
    });
    return this.request(`/v1/assets?${query.toString()}`);
  }

  async fetchAsset(
    assetRef: string,
    sessionId: string,
    requestedVersion?: number,
  ): Promise<ApiResponse> {
    const started = performance.now();
    let metadataResponse: ApiResponse;
    try {
      metadataResponse = await this.assetMetadata(assetRef, sessionId);
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
        `asset version ${requestedVersion} is not active in the supplied session`,
      );
    }
    const response = await this.fetchImpl(
      `${this.baseUrl}/v1/assets/${encodeURIComponent(assetRef)}/versions/`
      + `${metadata.version}/content?${sessionQuery(sessionId)}`,
      {
        method: "GET",
        headers: {
          accept: "*/*",
          "accept-encoding": "identity",
          authorization: `Bearer ${this.token}`,
          ...this.requestHeaders,
        },
        redirect: "error",
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
    const form = new FormData();
    form.set("scope", scope);
    if (stableImportId) {
      form.set("stable_import_id", stableImportId);
    }
    form.set("describe_binaries", describeBinaries ? "true" : "false");
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
    for (const { file, filePath } of resolvedFiles) {
      const bytes = await readFile(filePath);
      form.append("path", file.name ?? file.path.replaceAll("\\", "/"));
      form.append(
        "file",
        new Blob([bytes], { type: file.media_type ?? "application/octet-stream" }),
        basename(filePath),
      );
    }
    const response = await this.fetchImpl(`${this.baseUrl}/v1/memory/stage`, {
      method: "POST",
      headers: {
        accept: "application/json",
        authorization: `Bearer ${this.token}`,
        ...this.requestHeaders,
      },
      body: form,
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

function sessionQuery(sessionId: string): string {
  return new URLSearchParams({ session_id: sessionId }).toString();
}
