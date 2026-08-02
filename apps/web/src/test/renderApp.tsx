import { QueryClient } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import { StrictMode } from "react";
import { vi } from "vitest";
import { StraylightApp } from "../App";
import { createTestRouter } from "../router";

interface MockResponse {
  status?: number;
  body: unknown;
}

type MockRoute =
  | unknown
  | MockResponse
  | Response
  | ((
      request: Request,
    ) =>
      | unknown
      | MockResponse
      | Response
      | Promise<unknown | MockResponse | Response>);

const now = "2026-07-11T18:00:00Z";

export const defaultMe = {
  status: "complete",
  corpus_revision: "rev_001",
  freshness: {
    source_updated_at: now,
    semantic_index_updated_at: now,
  },
  data: {
    user: {
      id: "user_1",
      display_name: "Aether",
      username: "aether",
      email: "aether@example.com",
    },
    active_scope: { id: "scope_1", name: "Primary", access: "read_write" },
    scopes: [{ id: "scope_1", name: "Primary", access: "read_write" }],
    corpus_revision: "rev_001",
    capabilities: [
      "open",
      "query",
      "read",
      "status",
      "save",
      "checkpoint",
      "stage",
      "dream",
      "credential:manage",
    ],
    read_only: false,
    freshness: {
      source_updated_at: now,
      semantic_index_updated_at: now,
    },
  },
};

const defaultStatus = {
  status: "complete",
  data: {
    status: "healthy",
    version: "test",
    corpus_revision: "rev_001",
    dependencies: {},
    indexes: {},
  },
};

function isMockResponse(value: unknown): value is MockResponse {
  return Boolean(value && typeof value === "object" && "body" in value);
}

export function installApiMock(routes: Record<string, MockRoute> = {}) {
  const allRoutes: Record<string, MockRoute> = {
    "GET /api/v1/auth/session": {
      status: "complete",
      data: {
        user: {
          id: "user_1",
          display_name: "Aether",
          username: "aether",
          email: "aether@example.com",
        },
        expires_at: "2099-07-12T18:00:00Z",
      },
    },
    "GET /api/v1/me": defaultMe,
    "GET /api/v1/status": defaultStatus,
    "GET /api/v1/sessions": { status: "complete", data: { items: [] } },
    "GET /api/v1/workspace/manifest": {
      status: "complete",
      corpus_revision: "generation:12",
      data: {
        workspace_generation: 12,
        entries: [],
        offset: 0,
        limit: 1000,
        truncated: false,
      },
    },
    "GET /api/v1/workspace/changes": {
      status: "complete",
      corpus_revision: "generation:12",
      data: {
        since_generation: 0,
        workspace_generation: 12,
        changes: [],
        truncated: false,
      },
    },
    "GET /api/v1/workspace/binaries": {
      status: "complete",
      data: { binaries: [] },
    },
    "GET /api/v1/workspace/usage": {
      status: "complete",
      data: {
        sort: "most_used",
        entries: [],
        offset: 0,
        limit: 100,
      },
    },
    "GET /api/v1/workspace/jobs": {
      status: "complete",
      data: {
        jobs: [],
        offset: 0,
        limit: 100,
        truncated: false,
      },
    },
    "POST /api/v1/workspace/open": {
      status: "complete",
      session_id: "session_test",
      corpus_revision: "generation:12",
      data: {
        workspace_generation: 12,
        evidence: [],
        changes_since_checkpoint: [],
        retrieval_sufficiency: {
          status: "no_evidence",
          complete_source_count: 0,
          selected_source_count: 0,
        },
      },
    },
    ...routes,
  };

  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request
      ? input
      : new Request(new URL(String(input), window.location.origin), init);
    const path = new URL(request.url).pathname;
    const key = `${request.method.toUpperCase()} ${path}`;
    const route = allRoutes[key];
    if (route === undefined) throw new Error(`Unhandled API request: ${key}`);
    const result = typeof route === "function" ? await route(request) : route;
    if (result instanceof Response) return result;
    const response: MockResponse = isMockResponse(result) ? result : { body: result };
    return new Response(JSON.stringify(response.body), {
      status: response.status ?? 200,
      headers: { "Content-Type": "application/json" },
    });
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

export function renderApp(
  path: string,
  _legacyTokenHint?: string,
  options: { strict?: boolean } = {},
) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: 0, refetchOnWindowFocus: false },
      mutations: { retry: false },
    },
  });
  const router = createTestRouter(path);
  const app = <StraylightApp router={router} queryClient={queryClient} />;
  return {
    ...render(options.strict ? <StrictMode>{app}</StrictMode> : app),
    queryClient,
    router,
  };
}
