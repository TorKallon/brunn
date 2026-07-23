import type {
  ApiEnvelope,
  AuditEvent,
  CheckpointSummary,
  CommitReceipt,
  CaptureReceipt,
  CredentialSummary,
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
  sessions(): Promise<ApiEnvelope<ListData<SessionSummary> | SessionSummary[]>>;
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
  audit(): Promise<ApiEnvelope<ListData<AuditEvent> | AuditEvent[]>>;
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

  async function download(path: string): Promise<Blob> {
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
      const body = await parseBody(response) as { code?: string; message?: string };
      throw new ApiError(response.status, body.code ?? `http_${response.status}`, body.message ?? "The download failed.");
    }
    return response.blob();
  }

  return {
    me: () => get<MeData>("/me"),
    status: () => get<ServiceStatus>("/status"),
    sessions: () => get<ListData<SessionSummary> | SessionSummary[]>("/sessions"),
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
    downloadSource: (id) => download(`/sources/${encodeURIComponent(id)}/content`),
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
    audit: () => get<ListData<AuditEvent> | AuditEvent[]>("/audit"),
  };
}
