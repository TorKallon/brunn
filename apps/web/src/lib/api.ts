import type {
  ApiEnvelope,
  AssetDownload,
  AssetListData,
  AssetRecord,
  AuditEvent,
  CheckpointSummary,
  CommitReceipt,
  CaptureReceipt,
  CredentialSummary,
  DataUsage,
  DreamDetail,
  DreamSummary,
  JsonObject,
  JsonValue,
  ListData,
  MeData,
  ObjectRecord,
  PolicySummary,
  QueryResultData,
  ScopeSummary,
  ServiceStatus,
  SessionDetail,
  SessionSummary,
  SourceRecord,
  StageReceipt,
  VerificationResult,
} from "./types";

const API_ROOT = "/api/v1";
const MAX_BROWSER_DOWNLOAD_BYTES = 64 * 1024 * 1024;

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly details?: JsonValue;

  constructor(status: number, code: string, message: string, details?: JsonValue) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

function isEnvelope<T>(value: unknown): value is ApiEnvelope<T> {
  return Boolean(
    value &&
      typeof value === "object" &&
      "status" in value &&
      "data" in value,
  );
}

function completeEnvelope<T>(data: T): ApiEnvelope<T> {
  return { status: "complete", data };
}

async function parseBody(response: Response): Promise<unknown> {
  if (response.status === 204) return null;
  const contentType = response.headers.get("content-type") ?? "";
  if (!contentType.includes("application/json")) {
    return { message: await response.text() };
  }
  return response.json();
}

export interface StraylightApi {
  me(): Promise<ApiEnvelope<MeData>>;
  status(): Promise<ApiEnvelope<ServiceStatus>>;
  sessions(cursor?: string): Promise<ApiEnvelope<ListData<SessionSummary> | SessionSummary[]>>;
  session(id: string): Promise<ApiEnvelope<SessionDetail>>;
  refreshSession(id: string): Promise<ApiEnvelope<SessionDetail>>;
  open(payload: JsonObject): Promise<ApiEnvelope<SessionDetail>>;
  query(payload: JsonObject): Promise<ApiEnvelope<QueryResultData>>;
  read(payload: JsonObject): Promise<ApiEnvelope<JsonValue>>;
  compute(payload: JsonObject): Promise<ApiEnvelope<JsonValue>>;
  verify(payload: JsonObject): Promise<ApiEnvelope<{ results: VerificationResult[] }>>;
  checkpoint(payload: JsonObject): Promise<ApiEnvelope<CheckpointSummary>>;
  capture(payload: JsonObject): Promise<ApiEnvelope<CaptureReceipt>>;
  save(payload: JsonObject): Promise<ApiEnvelope<CommitReceipt>>;
  stage(payload: FormData | JsonObject): Promise<ApiEnvelope<StageReceipt>>;
  object(id: string): Promise<ApiEnvelope<ObjectRecord>>;
  source(id: string): Promise<ApiEnvelope<SourceRecord>>;
  downloadSource(id: string): Promise<Blob>;
  assets(
    sessionId: string,
    offset?: number,
    limit?: number,
  ): Promise<ApiEnvelope<AssetListData>>;
  asset(id: string, sessionId: string): Promise<ApiEnvelope<AssetRecord>>;
  downloadAsset(
    id: string,
    version: number,
    sessionId: string,
    expectedHash: string,
    expectedSize: number,
  ): Promise<AssetDownload>;
  dreams(): Promise<ApiEnvelope<ListData<DreamSummary> | DreamSummary[]>>;
  dream(id: string): Promise<ApiEnvelope<DreamDetail>>;
  reviewDream(
    id: string,
    payload: JsonObject,
  ): Promise<ApiEnvelope<DreamDetail>>;
  rollbackDream(id: string, payload: JsonObject): Promise<ApiEnvelope<DreamDetail>>;
  credentials(): Promise<
    ApiEnvelope<ListData<CredentialSummary> | CredentialSummary[]>
  >;
  createCredential(payload: JsonObject): Promise<ApiEnvelope<CredentialSummary>>;
  revokeCredential(id: string): Promise<ApiEnvelope<CredentialSummary>>;
  scopes(): Promise<ApiEnvelope<ListData<ScopeSummary> | ScopeSummary[]>>;
  policies(): Promise<ApiEnvelope<ListData<PolicySummary> | PolicySummary[]>>;
  audit(cursor?: string): Promise<ApiEnvelope<ListData<AuditEvent> | AuditEvent[]>>;
  usage(): Promise<ApiEnvelope<DataUsage>>;
}

export function createApiClient(getToken: () => string | null): StraylightApi {
  async function request<T>(
    path: string,
    init: RequestInit = {},
  ): Promise<ApiEnvelope<T>> {
    const token = getToken();
    const headers = new Headers(init.headers);
    headers.set("Accept", "application/json");
    if (token) headers.set("Authorization", `Bearer ${token}`);
    if (init.body && !(init.body instanceof FormData)) {
      headers.set("Content-Type", "application/json");
    }

    let response: Response;
    try {
      response = await fetch(`${API_ROOT}${path}`, { ...init, headers });
    } catch (error) {
      throw new ApiError(
        0,
        "network_error",
        error instanceof Error ? error.message : "The service could not be reached.",
      );
    }

    const body = await parseBody(response);
    if (!response.ok) {
      const errorBody = body as {
        code?: string;
        message?: string;
        error?: { code?: string; message?: string; details?: JsonValue };
        details?: JsonValue;
      };
      throw new ApiError(
        response.status,
        errorBody.error?.code ?? errorBody.code ?? `http_${response.status}`,
        errorBody.error?.message ??
          errorBody.message ??
          response.statusText ??
          "The request failed.",
        errorBody.error?.details ?? errorBody.details,
      );
    }

    return isEnvelope<T>(body) ? body : completeEnvelope(body as T);
  }

  const get = <T>(path: string) => request<T>(path);
  const post = <T>(path: string, payload?: JsonObject | FormData) =>
    request<T>(path, {
      method: "POST",
      body:
        payload instanceof FormData
          ? payload
          : payload
            ? JSON.stringify(payload)
            : undefined,
    });
  const del = <T>(path: string) => request<T>(path, { method: "DELETE" });

  async function download(
    path: string,
    expected?: { contentHash: string; sizeBytes: number },
  ): Promise<AssetDownload> {
    const headers = new Headers({ Accept: "application/octet-stream" });
    const token = getToken();
    if (token) headers.set("Authorization", `Bearer ${token}`);
    let response: Response;
    try {
      response = await fetch(`${API_ROOT}${path}`, { headers });
    } catch (error) {
      throw new ApiError(0, "network_error", error instanceof Error ? error.message : "The service could not be reached.");
    }
    if (!response.ok) {
      const body = await parseBody(response) as {
        code?: string;
        message?: string;
        error?: { code?: string; message?: string; details?: JsonValue };
        details?: JsonValue;
      };
      throw new ApiError(
        response.status,
        body.error?.code ?? body.code ?? `http_${response.status}`,
        body.error?.message ?? body.message ?? "The download failed.",
        body.error?.details ?? body.details,
      );
    }
    const declaredSize = parseDownloadSize(response.headers.get("content-length"));
    if (
      declaredSize !== undefined
      && declaredSize > MAX_BROWSER_DOWNLOAD_BYTES
    ) {
      await response.body?.cancel();
      throw new ApiError(
        413,
        "browser_download_too_large",
        "This asset is too large for a browser download. Use the CarryState CLI or MCP asset fetch instead.",
      );
    }
    if (expected && expected.sizeBytes > MAX_BROWSER_DOWNLOAD_BYTES) {
      await response.body?.cancel();
      throw new ApiError(
        413,
        "browser_download_too_large",
        "This asset is too large for a browser download. Use the CarryState CLI or MCP asset fetch instead.",
      );
    }
    const bytes = await readBoundedDownload(response);
    const responseHash = normalizeSha256(
      response.headers.get("x-carrystate-sha256"),
    );
    if (expected) {
      const expectedHash = normalizeSha256(expected.contentHash);
      if (declaredSize !== undefined && declaredSize !== expected.sizeBytes) {
        throw new ApiError(
          409,
          "asset_size_mismatch",
          "The asset download size does not match its session-pinned metadata.",
        );
      }
      if (bytes.byteLength !== expected.sizeBytes) {
        throw new ApiError(
          409,
          "asset_size_mismatch",
          "The downloaded asset bytes do not match the expected size.",
        );
      }
      if (!responseHash || responseHash !== expectedHash) {
        throw new ApiError(
          409,
          "asset_hash_mismatch",
          "The asset download hash header does not match its session-pinned metadata.",
        );
      }
      const actualHash = hexDigest(
        await globalThis.crypto.subtle.digest("SHA-256", bytes),
      );
      if (actualHash !== expectedHash) {
        throw new ApiError(
          409,
          "asset_hash_mismatch",
          "The downloaded asset failed SHA-256 verification.",
        );
      }
    }
    return {
      blob: new Blob([bytes]),
      filename: filenameFromDisposition(response.headers.get("content-disposition")),
      contentHash: responseHash ? `sha256:${responseHash}` : undefined,
    };
  }

  return {
    me: () => get<MeData>("/me"),
    status: () => get<ServiceStatus>("/status"),
    sessions: (cursor) =>
      get<ListData<SessionSummary> | SessionSummary[]>(
        cursor ? `/sessions?cursor=${encodeURIComponent(cursor)}` : "/sessions",
      ),
    session: (id) => get<SessionDetail>(`/sessions/${encodeURIComponent(id)}`),
    refreshSession: (id) =>
      post<SessionDetail>(`/sessions/${encodeURIComponent(id)}/refresh`, {}),
    open: (payload) => post<SessionDetail>("/memory/open", payload),
    query: (payload) => post<QueryResultData>("/memory/query", payload),
    read: (payload) => post<JsonValue>("/memory/read", payload),
    compute: (payload) => post<JsonValue>("/memory/compute", payload),
    verify: (payload) =>
      post<{ results: VerificationResult[] }>("/memory/verify", payload),
    checkpoint: (payload) =>
      post<CheckpointSummary>("/memory/checkpoint", payload),
    capture: (payload) => post<CaptureReceipt>("/memory/capture", payload),
    save: (payload) => post<CommitReceipt>("/memory/save", payload),
    stage: (payload) => post<StageReceipt>("/memory/stage", payload),
    object: (id) => get<ObjectRecord>(`/objects/${encodeURIComponent(id)}`),
    source: (id) => get<SourceRecord>(`/sources/${encodeURIComponent(id)}`),
    downloadSource: async (id) =>
      (await download(`/sources/${encodeURIComponent(id)}/content`)).blob,
    assets: (sessionId, offset = 0, limit = 100) => {
      const query = new URLSearchParams({
        session_id: sessionId,
        offset: String(offset),
        limit: String(limit),
      });
      return get<AssetListData>(`/assets?${query.toString()}`);
    },
    asset: (id, sessionId) => {
      const query = new URLSearchParams({ session_id: sessionId });
      return get<AssetRecord>(
        `/assets/${encodeURIComponent(id)}?${query.toString()}`,
      );
    },
    downloadAsset: (id, version, sessionId, expectedHash, expectedSize) => {
      const query = new URLSearchParams({ session_id: sessionId });
      return download(
        `/assets/${encodeURIComponent(id)}/versions/${version}/content?${query.toString()}`,
        { contentHash: expectedHash, sizeBytes: expectedSize },
      );
    },
    dreams: () => get<ListData<DreamSummary> | DreamSummary[]>("/dreams"),
    dream: (id) => get<DreamDetail>(`/dreams/${encodeURIComponent(id)}`),
    reviewDream: (id, payload) =>
      post<DreamDetail>(`/dreams/${encodeURIComponent(id)}/review`, payload),
    rollbackDream: (id, payload) =>
      post<DreamDetail>(`/dreams/${encodeURIComponent(id)}/rollback`, payload),
    credentials: () =>
      get<ListData<CredentialSummary> | CredentialSummary[]>("/credentials"),
    createCredential: (payload) => post<CredentialSummary>("/credentials", payload),
    revokeCredential: (id) =>
      del<CredentialSummary>(`/credentials/${encodeURIComponent(id)}`),
    scopes: () => get<ListData<ScopeSummary> | ScopeSummary[]>("/scopes"),
    policies: () => get<ListData<PolicySummary> | PolicySummary[]>("/policies"),
    audit: (cursor) =>
      get<ListData<AuditEvent> | AuditEvent[]>(
        cursor ? `/audit?cursor=${encodeURIComponent(cursor)}` : "/audit",
      ),
    usage: () => get<DataUsage>("/usage"),
  };
}

function parseDownloadSize(value: string | null): number | undefined {
  if (value === null) return undefined;
  if (!/^(0|[1-9][0-9]*)$/.test(value)) {
    throw new ApiError(409, "invalid_content_length", "The download returned an invalid content length.");
  }
  const size = Number(value);
  if (!Number.isSafeInteger(size)) {
    throw new ApiError(409, "invalid_content_length", "The download content length is too large.");
  }
  return size;
}

async function readBoundedDownload(response: Response): Promise<ArrayBuffer> {
  if (!response.body) return new ArrayBuffer(0);
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const item = await reader.read();
      if (item.done) break;
      size += item.value.byteLength;
      if (size > MAX_BROWSER_DOWNLOAD_BYTES) {
        await reader.cancel();
        throw new ApiError(
          413,
          "browser_download_too_large",
          "This asset is too large for a browser download. Use the CarryState CLI or MCP asset fetch instead.",
        );
      }
      chunks.push(item.value);
    }
  } finally {
    reader.releaseLock();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes.buffer;
}

function normalizeSha256(value: string | null): string | undefined {
  if (!value) return undefined;
  const digest = value.startsWith("sha256:") ? value.slice(7) : value;
  return /^[0-9a-f]{64}$/i.test(digest) ? digest.toLowerCase() : undefined;
}

function hexDigest(value: ArrayBuffer): string {
  return Array.from(new Uint8Array(value), (byte) =>
    byte.toString(16).padStart(2, "0")
  ).join("");
}

function filenameFromDisposition(value: string | null): string | undefined {
  if (!value) return undefined;
  const encoded = value.match(/filename\*=UTF-8''([^;]+)/i)?.[1];
  if (encoded) {
    try {
      return decodeURIComponent(encoded);
    } catch {
      return undefined;
    }
  }
  return value.match(/filename="([^"]+)"/i)?.[1] ?? value.match(/filename=([^;]+)/i)?.[1]?.trim();
}
