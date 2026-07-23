export interface ApiResponse {
  status: number;
  body: Record<string, unknown>;
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

  constructor(
    baseUrl: string,
    private readonly token: string,
    private readonly fetchImpl: typeof fetch = fetch,
  ) {
    this.baseUrl = baseUrl.replace(/\/$/, "");
  }

  async request(path: string, body?: unknown): Promise<ApiResponse> {
    const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
      method: body === undefined ? "GET" : "POST",
      headers: {
        accept: "application/json",
        authorization: `Bearer ${this.token}`,
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    const parsed = await parseJson(response);
    if (!response.ok) {
      throw new StraylightApiError(response.status, parsed);
    }
    return { status: response.status, body: parsed };
  }

  async stage(
    scope: string,
    stableImportId: string | undefined,
    files: Array<{ path: string; name?: string | undefined; media_type?: string | undefined }>,
  ): Promise<ApiResponse> {
    const form = new FormData();
    form.set("scope", scope);
    if (stableImportId) {
      form.set("stable_import_id", stableImportId);
    }
    const importRoot = await realpath(
      process.env.STRAYLIGHT_MCP_IMPORT_ROOT ?? "/imports",
    );
    for (const file of files) {
      const filePath = await realpath(resolve(importRoot, file.path));
      const insideRoot = relative(importRoot, filePath);
      if (insideRoot.startsWith("..") || insideRoot.includes("/../")) {
        throw new Error("staged paths must remain inside STRAYLIGHT_MCP_IMPORT_ROOT");
      }
      const bytes = await readFile(filePath);
      form.append(
        "file",
        new Blob([bytes], { type: file.media_type ?? "application/octet-stream" }),
        file.name ?? basename(filePath),
      );
    }
    const response = await this.fetchImpl(`${this.baseUrl}/v1/memory/stage`, {
      method: "POST",
      headers: {
        accept: "application/json",
        authorization: `Bearer ${this.token}`,
      },
      body: form,
    });
    const parsed = await parseJson(response);
    if (!response.ok) {
      throw new StraylightApiError(response.status, parsed);
    }
    return { status: response.status, body: parsed };
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
import { readFile, realpath } from "node:fs/promises";
import { basename, relative, resolve } from "node:path";
