import { ApiError, SESSION_INVALIDATED_EVENT } from "../lib/api";
import type { ApiEnvelope, JsonObject, JsonValue } from "../lib/types";
import type {
  CreateAgentInput,
  CreateConversationData,
  CredentialBindingData,
  MessagingAgentListData,
  MessagingAgentMutationData,
  MessagingSyncData,
  ReadConversationData,
  SendMessageData,
  SendMessageInput,
  UpdateAgentInput,
} from "./types";

const API_ROOT = "/api/v1/workspace/messaging";
const PUBLIC_AUTH_PATHS = new Set([
  "/auth/login",
  "/auth/forgot-password",
  "/auth/reset-password",
]);

export interface SyncInput {
  cursor?: number;
  conversationId?: string;
  afterSeq?: number;
  limit?: number;
  wait?: number;
}

export interface MessagingApi {
  sync(input?: SyncInput): Promise<ApiEnvelope<MessagingSyncData>>;
  listAgents(): Promise<ApiEnvelope<MessagingAgentListData>>;
  createConversation(
    participants: string[],
    subject?: string,
  ): Promise<ApiEnvelope<CreateConversationData>>;
  sendMessage(
    conversationId: string,
    input: SendMessageInput,
  ): Promise<ApiEnvelope<SendMessageData>>;
  markRead(
    conversationId: string,
    lastReadSeq: number,
  ): Promise<ApiEnvelope<ReadConversationData>>;
  createAgent(input: CreateAgentInput): Promise<ApiEnvelope<MessagingAgentMutationData>>;
  updateAgent(
    agentId: string,
    input: UpdateAgentInput,
  ): Promise<ApiEnvelope<MessagingAgentMutationData>>;
  bindCredential(
    agentId: string,
    credentialId: string | null,
  ): Promise<ApiEnvelope<CredentialBindingData>>;
}

function isEnvelope<T>(value: unknown): value is ApiEnvelope<T> {
  return Boolean(
    value && typeof value === "object" && "status" in value && "data" in value,
  );
}

async function parseBody(response: Response): Promise<unknown> {
  if (response.status === 204) return null;
  const contentType = response.headers.get("content-type") ?? "";
  return contentType.includes("application/json")
    ? response.json()
    : { message: await response.text() };
}

function readCsrfToken(): string | null {
  for (const raw of document.cookie.split(";")) {
    const cookie = raw.trim();
    for (const name of ["__Host-straylight_csrf", "straylight_csrf"]) {
      const prefix = `${name}=`;
      if (!cookie.startsWith(prefix)) continue;
      const value = cookie.slice(prefix.length);
      try {
        return decodeURIComponent(value);
      } catch {
        return value;
      }
    }
  }
  return null;
}

export function createMessagingApiClient(): MessagingApi {
  async function request<T>(
    path: string,
    method = "GET",
    payload?: JsonObject,
  ): Promise<ApiEnvelope<T>> {
    const headers = new Headers({ Accept: "application/json" });
    if (payload) headers.set("Content-Type", "application/json");
    if (!["GET", "HEAD", "OPTIONS"].includes(method)) {
      const csrfToken = readCsrfToken();
      if (csrfToken) headers.set("X-CSRF-Token", csrfToken);
    }

    let response: Response;
    try {
      response = await fetch(`${API_ROOT}${path}`, {
        method,
        credentials: "same-origin",
        headers,
        body: payload ? JSON.stringify(payload) : undefined,
      });
    } catch {
      throw new ApiError(
        0,
        "network_error",
        "The messaging service could not be reached.",
      );
    }

    if (
      response.status === 401 &&
      !PUBLIC_AUTH_PATHS.has(path) &&
      typeof window !== "undefined"
    ) {
      window.dispatchEvent(new Event(SESSION_INVALIDATED_EVENT));
    }
    const body = await parseBody(response);
    if (!response.ok) {
      const errorBody = body as {
        code?: string;
        message?: string;
        details?: JsonValue;
        error?: { code?: string; message?: string; details?: JsonValue };
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
    return isEnvelope<T>(body)
      ? body
      : { status: "complete", data: body as T };
  }

  return {
    sync: (input = {}) => {
      const query = new URLSearchParams({
        cursor: String(input.conversationId ? 0 : (input.cursor ?? 0)),
        wait: String(input.wait ?? 0),
        limit: String(input.limit ?? 200),
      });
      if (input.conversationId) {
        query.set("conversation_id", input.conversationId);
        query.set("after_seq", String(input.afterSeq ?? 0));
      }
      return request<MessagingSyncData>(`/sync?${query.toString()}`);
    },
    listAgents: () => request<MessagingAgentListData>("/agents"),
    createConversation: (participants, subject) =>
      request<CreateConversationData>("/conversations", "POST", {
        participants,
        ...(subject ? { subject } : {}),
      }),
    sendMessage: (conversationId, input) =>
      request<SendMessageData>(
        `/conversations/${encodeURIComponent(conversationId)}/messages`,
        "POST",
        input as unknown as JsonObject,
      ),
    markRead: (conversationId, lastReadSeq) =>
      request<ReadConversationData>(
        `/conversations/${encodeURIComponent(conversationId)}/read`,
        "POST",
        { last_read_seq: lastReadSeq },
      ),
    createAgent: (input) =>
      request<MessagingAgentMutationData>(
        "/agents",
        "POST",
        input as unknown as JsonObject,
      ),
    updateAgent: (agentId, input) =>
      request<MessagingAgentMutationData>(
        `/agents/${encodeURIComponent(agentId)}`,
        "PATCH",
        input as unknown as JsonObject,
      ),
    bindCredential: (agentId, credentialId) =>
      request<CredentialBindingData>(
        `/agents/${encodeURIComponent(agentId)}/credential`,
        "PUT",
        { credential_id: credentialId },
      ),
  };
}
