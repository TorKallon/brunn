import { QueryClient } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import { vi } from "vitest";
import { StraylightApp } from "../App";
import { AUTH_SESSION_KEY } from "../lib/auth";
import { createTestRouter } from "../router";

interface MockResponse {
  status?: number;
  body: unknown;
}

type MockRoute = unknown | MockResponse | ((request: Request) => unknown | MockResponse | Promise<unknown | MockResponse>);

const now = "2026-07-11T18:00:00Z";

export const defaultMe = {
  status: "complete",
  corpus_revision: "rev_001",
  freshness: {
    source_updated_at: now,
    semantic_index_updated_at: now,
  },
  data: {
    user: { id: "user_1", display_name: "Aether", email: "aether@example.com" },
    active_scope: { id: "scope_1", name: "Primary", access: "read_write" },
    scopes: [{ id: "scope_1", name: "Primary", access: "read_write" }],
    corpus_revision: "rev_001",
    capabilities: ["open", "query", "read", "compute", "verify", "status", "save", "credential:manage"],
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
    "GET /api/v1/me": defaultMe,
    "GET /api/v1/status": defaultStatus,
    "GET /api/v1/sessions": { status: "complete", data: { items: [] } },
    "POST /api/v1/memory/open": {
      status: "complete",
      session_id: "session_test",
      corpus_revision: "rev_001",
      data: { session_id: "session_test", corpus_revision: "rev_001" },
    },
    ...routes,
  };

  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
    const path = new URL(request.url).pathname;
    const key = `${request.method.toUpperCase()} ${path}`;
    const route = allRoutes[key];
    if (route === undefined) throw new Error(`Unhandled API request: ${key}`);
    const result = typeof route === "function" ? await route(request) : route;
    const response: MockResponse = isMockResponse(result) ? result : { body: result };
    return new Response(JSON.stringify(response.body), {
      status: response.status ?? 200,
      headers: { "Content-Type": "application/json" },
    });
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

export function renderApp(path: string, token?: string) {
  if (token) window.sessionStorage.setItem(AUTH_SESSION_KEY, token);
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: 0, refetchOnWindowFocus: false },
      mutations: { retry: false },
    },
  });
  const router = createTestRouter(path);
  return {
    ...render(<StraylightApp router={router} queryClient={queryClient} />),
    queryClient,
    router,
  };
}
