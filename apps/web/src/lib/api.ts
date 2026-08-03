import type {
  ApiEnvelope,
  AuthCompletionData,
  AuthSessionData,
  AssetDownload,
  AssetListData,
  AssetRecord,
  AuditEvent,
  BriefingEditionData,
  BriefingItemActionData,
  BriefingItemActionInput,
  BriefingListData,
  BriefingTopicsSnapshot,
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
  NotificationDetailData,
  NotificationImportance,
  NotificationListData,
  NotificationReceiptData,
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
  WorkspaceBinary,
  WorkspaceBinaryListData,
  WorkspaceBinaryReceipt,
  WorkspaceChangesData,
  WorkspaceCheckpointReceipt,
  WorkspaceDashboardData,
  WorkspaceDreamReceipt,
  WorkspaceJobsData,
  WorkspaceManifestData,
  WorkspaceOpenData,
  WorkspaceReadData,
  WorkspaceSearchData,
  WorkspaceUsageData,
  WorkspaceUsageSort,
  WorkspaceWriteReceipt,
} from "./types";

const API_ROOT = "/api/v1";
const MAX_BROWSER_DOWNLOAD_BYTES = 64 * 1024 * 1024;
export const SESSION_INVALIDATED_EVENT = "straylight:session-invalidated";
const PUBLIC_AUTH_PATHS = new Set([
  "/auth/login",
  "/auth/forgot-password",
  "/auth/reset-password",
]);

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
  authSession(): Promise<ApiEnvelope<AuthSessionData>>;
  login(email: string, password: string): Promise<ApiEnvelope<AuthSessionData>>;
  logout(): Promise<ApiEnvelope<AuthCompletionData>>;
  forgotPassword(identifier: string): Promise<ApiEnvelope<AuthCompletionData>>;
  resetPassword(token: string, password: string): Promise<ApiEnvelope<AuthCompletionData>>;
  me(): Promise<ApiEnvelope<MeData>>;
  status(): Promise<ApiEnvelope<ServiceStatus>>;
  workspaceOpen(payload: JsonObject): Promise<ApiEnvelope<WorkspaceOpenData>>;
  workspaceSearch(payload: JsonObject): Promise<ApiEnvelope<WorkspaceSearchData>>;
  workspaceRead(payload: JsonObject): Promise<ApiEnvelope<WorkspaceReadData>>;
  workspaceWrite(payload: JsonObject): Promise<ApiEnvelope<WorkspaceWriteReceipt>>;
  workspaceCapture(payload: JsonObject): Promise<ApiEnvelope<WorkspaceWriteReceipt>>;
  workspaceChanges(
    sinceGeneration?: number,
    limit?: number,
  ): Promise<ApiEnvelope<WorkspaceChangesData>>;
  workspaceCheckpoint(
    payload: JsonObject,
  ): Promise<ApiEnvelope<WorkspaceCheckpointReceipt>>;
  workspaceBinaries(
    offset?: number,
    limit?: number,
  ): Promise<ApiEnvelope<WorkspaceBinaryListData>>;
  workspaceBinary(entryRef: string): Promise<ApiEnvelope<WorkspaceBinary>>;
  uploadWorkspaceBinary(
    payload: FormData,
  ): Promise<ApiEnvelope<WorkspaceBinaryReceipt>>;
  downloadWorkspaceBinary(
    entryRef: string,
    expectedHash: string,
    expectedSize: number,
  ): Promise<AssetDownload>;
  workspaceManifest(
    offset?: number,
    limit?: number,
  ): Promise<ApiEnvelope<WorkspaceManifestData>>;
  workspaceUsage(
    sort?: WorkspaceUsageSort,
    offset?: number,
    limit?: number,
  ): Promise<ApiEnvelope<WorkspaceUsageData>>;
  workspaceDashboard(
    timezone?: string,
  ): Promise<ApiEnvelope<WorkspaceDashboardData>>;
  workspaceJobs(
    status?: string,
    offset?: number,
    limit?: number,
  ): Promise<ApiEnvelope<WorkspaceJobsData>>;
  workspaceDream(
    payload: JsonObject,
  ): Promise<ApiEnvelope<WorkspaceDreamReceipt>>;
  briefingsList(
    limit?: number,
    afterPath?: string,
  ): Promise<ApiEnvelope<BriefingListData>>;
  briefingGet(
    date: string,
    edition: string,
    version?: number,
  ): Promise<ApiEnvelope<BriefingEditionData>>;
  briefingTopics(): Promise<ApiEnvelope<BriefingTopicsSnapshot>>;
  briefingItemAction(
    input: BriefingItemActionInput,
  ): Promise<ApiEnvelope<BriefingItemActionData>>;
  notificationsList(
    limit?: number,
    cursor?: string,
    unread?: boolean,
    importance?: NotificationImportance,
  ): Promise<ApiEnvelope<NotificationListData>>;
  notificationGet(
    notificationRef: string,
  ): Promise<ApiEnvelope<NotificationDetailData>>;
  notificationReceipt(
    notificationRef: string,
    kind: "opened" | "acknowledged",
    deliveryRef?: string,
  ): Promise<ApiEnvelope<NotificationReceiptData>>;
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

export function createApiClient(): StraylightApi {
  async function request<T>(
    path: string,
    init: RequestInit = {},
  ): Promise<ApiEnvelope<T>> {
    const headers = new Headers(init.headers);
    headers.set("Accept", "application/json");
    if (init.body && !(init.body instanceof FormData)) {
      headers.set("Content-Type", "application/json");
    }
    if (isUnsafeMethod(init.method)) {
      const csrfToken = readCsrfToken();
      if (csrfToken) headers.set("X-CSRF-Token", csrfToken);
    }

    let response: Response;
    try {
      response = await fetch(`${API_ROOT}${path}`, {
        ...init,
        credentials: "same-origin",
        headers,
      });
    } catch (error) {
      throw new ApiError(
        0,
        "network_error",
        error instanceof Error ? error.message : "The service could not be reached.",
      );
    }

    notifyInvalidSession(response, path);
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
    let response: Response;
    try {
      response = await fetch(`${API_ROOT}${path}`, {
        credentials: "same-origin",
        headers,
      });
    } catch (error) {
      throw new ApiError(0, "network_error", error instanceof Error ? error.message : "The service could not be reached.");
    }
    notifyInvalidSession(response, path);
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
        "This asset is too large for a browser download. Use the Straylight CLI or MCP asset fetch instead.",
      );
    }
    if (expected && expected.sizeBytes > MAX_BROWSER_DOWNLOAD_BYTES) {
      await response.body?.cancel();
      throw new ApiError(
        413,
        "browser_download_too_large",
        "This asset is too large for a browser download. Use the Straylight CLI or MCP asset fetch instead.",
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
    authSession: () => get<AuthSessionData>("/auth/session"),
    login: (email, password) =>
      post<AuthSessionData>("/auth/login", { email, password }),
    logout: () => post<AuthCompletionData>("/auth/logout", {}),
    forgotPassword: (identifier) =>
      post<AuthCompletionData>("/auth/forgot-password", { identifier }),
    resetPassword: (token, password) =>
      post<AuthCompletionData>("/auth/reset-password", { token, password }),
    me: () => get<MeData>("/me"),
    status: () => get<ServiceStatus>("/status"),
    workspaceOpen: (payload) =>
      post<WorkspaceOpenData>("/workspace/open", payload),
    workspaceSearch: (payload) =>
      post<WorkspaceSearchData>("/workspace/search", payload),
    workspaceRead: (payload) =>
      post<WorkspaceReadData>("/workspace/read", payload),
    workspaceWrite: (payload) =>
      post<WorkspaceWriteReceipt>("/workspace/write", payload),
    workspaceCapture: (payload) =>
      post<WorkspaceWriteReceipt>("/workspace/capture", payload),
    workspaceChanges: (sinceGeneration = 0, limit = 200) => {
      const query = new URLSearchParams({
        since_generation: String(sinceGeneration),
        limit: String(limit),
      });
      return get<WorkspaceChangesData>(`/workspace/changes?${query.toString()}`);
    },
    workspaceCheckpoint: (payload) =>
      post<WorkspaceCheckpointReceipt>("/workspace/checkpoint", payload),
    workspaceBinaries: (offset = 0, limit = 100) => {
      const query = new URLSearchParams({
        offset: String(offset),
        limit: String(limit),
      });
      return get<WorkspaceBinaryListData>(
        `/workspace/binaries?${query.toString()}`,
      );
    },
    workspaceBinary: (entryRef) =>
      get<WorkspaceBinary>(
        `/workspace/binaries/${encodeURIComponent(entryRef)}`,
      ),
    uploadWorkspaceBinary: (payload) =>
      post<WorkspaceBinaryReceipt>("/workspace/binaries", payload),
    downloadWorkspaceBinary: (entryRef, expectedHash, expectedSize) =>
      download(
        `/workspace/binaries/${encodeURIComponent(entryRef)}/content`,
        { contentHash: expectedHash, sizeBytes: expectedSize },
      ),
    workspaceManifest: (offset = 0, limit = 1_000) => {
      const query = new URLSearchParams({
        offset: String(offset),
        limit: String(limit),
      });
      return get<WorkspaceManifestData>(
        `/workspace/manifest?${query.toString()}`,
      );
    },
    workspaceUsage: (sort = "most_used", offset = 0, limit = 100) => {
      const query = new URLSearchParams({
        sort,
        offset: String(offset),
        limit: String(limit),
      });
      return get<WorkspaceUsageData>(
        `/workspace/usage?${query.toString()}`,
      );
    },
    workspaceDashboard: (timezone) => {
      const query = new URLSearchParams();
      if (timezone) query.set("timezone", timezone);
      const suffix = query.size ? `?${query.toString()}` : "";
      return get<WorkspaceDashboardData>(`/workspace/dashboard${suffix}`);
    },
    workspaceJobs: (status, offset = 0, limit = 100) => {
      const query = new URLSearchParams({
        offset: String(offset),
        limit: String(limit),
      });
      if (status) query.set("status", status);
      return get<WorkspaceJobsData>(`/workspace/jobs?${query.toString()}`);
    },
    workspaceDream: (payload) =>
      post<WorkspaceDreamReceipt>("/workspace/dreams", payload),
    briefingsList: (limit = 14, afterPath) => {
      const query = new URLSearchParams({ limit: String(limit) });
      if (afterPath) query.set("after_path", afterPath);
      return get<BriefingListData>(`/workspace/briefings?${query.toString()}`);
    },
    briefingGet: (date, edition, version) => {
      const path =
        `/workspace/briefings/${encodeURIComponent(date)}` +
        `/${encodeURIComponent(edition)}`;
      if (version === undefined) return get<BriefingEditionData>(path);
      const query = new URLSearchParams({ version: String(version) });
      return get<BriefingEditionData>(`${path}?${query.toString()}`);
    },
    briefingTopics: () =>
      get<BriefingTopicsSnapshot>("/workspace/briefings/topics"),
    briefingItemAction: (input) => {
      const payload: JsonObject = { action: input.action };
      if (input.edition_ref !== undefined) payload.edition_ref = input.edition_ref;
      if (input.item_id !== undefined) payload.item_id = input.item_id;
      if (input.topic_slug !== undefined) payload.topic_slug = input.topic_slug;
      if (input.verdict !== undefined) payload.verdict = input.verdict;
      if (input.note !== undefined) payload.note = input.note;
      return post<BriefingItemActionData>(
        "/workspace/briefings/items/action",
        payload,
      );
    },
    notificationsList: (limit = 50, cursor, unread, importance) => {
      const query = new URLSearchParams({ limit: String(limit) });
      if (cursor) query.set("cursor", cursor);
      if (unread !== undefined) query.set("unread", String(unread));
      if (importance) query.set("importance", importance);
      return get<NotificationListData>(
        `/workspace/notifications?${query.toString()}`,
      );
    },
    notificationGet: (notificationRef) =>
      get<NotificationDetailData>(
        `/workspace/notifications/${encodeURIComponent(notificationRef)}`,
      ),
    notificationReceipt: (notificationRef, kind, deliveryRef) => {
      const payload: JsonObject = { kind };
      if (deliveryRef) payload.delivery_ref = deliveryRef;
      return post<NotificationReceiptData>(
        `/workspace/notifications/${encodeURIComponent(notificationRef)}/receipts`,
        payload,
      );
    },
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

function isUnsafeMethod(method?: string): boolean {
  const normalized = (method ?? "GET").toUpperCase();
  return !["GET", "HEAD", "OPTIONS"].includes(normalized);
}

function notifyInvalidSession(response: Response, path: string): void {
  if (
    response.status === 401
    && !PUBLIC_AUTH_PATHS.has(path)
    && typeof window !== "undefined"
  ) {
    window.dispatchEvent(new Event(SESSION_INVALIDATED_EVENT));
  }
}

function readCsrfToken(): string | null {
  const cookies = document.cookie.split(";");
  for (const name of ["__Host-straylight_csrf", "straylight_csrf"]) {
    const prefix = `${name}=`;
    const cookie = cookies.map((value) => value.trim()).find((value) => value.startsWith(prefix));
    if (!cookie) continue;
    const value = cookie.slice(prefix.length);
    try {
      return decodeURIComponent(value);
    } catch {
      return value;
    }
  }
  return null;
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
          "This asset is too large for a browser download. Use the Straylight CLI or MCP asset fetch instead.",
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
