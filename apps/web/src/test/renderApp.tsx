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
    credential_id: "credential:current",
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

export const defaultDashboard = {
  status: "complete",
  data: {
    generated_at: now,
    timezone: "UTC",
    workspace_generation: 12,
    activity_tracking_started_at: "2026-07-05T00:00:00Z",
    tracking: {
      status: "enabled",
      dropped_events: 0,
      flush_failures: 0,
    },
    storage: {
      text: { count: 128, size_bytes: 2_621_440 },
      binary: {
        count: 14,
        size_bytes: 18_874_368,
        semantics: "physical_object_versions",
        status: "fresh",
        observed_at: now,
      },
    },
    today: {
      read_operations: 37,
      read_bytes: 483_328,
      write_operations: 6,
      write_bytes: 32_768,
    },
    activity: [
      { date: "2026-07-05", read_operations: 12, read_bytes: 110_000, write_operations: 2, write_bytes: 8_000 },
      { date: "2026-07-06", read_operations: 19, read_bytes: 180_000, write_operations: 1, write_bytes: 4_000 },
      { date: "2026-07-07", read_operations: 8, read_bytes: 72_000, write_operations: 4, write_bytes: 22_000 },
      { date: "2026-07-08", read_operations: 24, read_bytes: 260_000, write_operations: 3, write_bytes: 16_000 },
      { date: "2026-07-09", read_operations: 30, read_bytes: 390_000, write_operations: 7, write_bytes: 44_000 },
      { date: "2026-07-10", read_operations: 21, read_bytes: 240_000, write_operations: 2, write_bytes: 9_000 },
      { date: "2026-07-11", read_operations: 37, read_bytes: 483_328, write_operations: 6, write_bytes: 32_768 },
    ],
    access: [
      {
        id: "credential:current",
        name: "Straylight Web",
        kind: "web_ui",
        manageable: false,
        access: "owner",
        status: "active",
        scope_ids: ["scope:root"],
        created_at: "2026-07-01T00:00:00Z",
        last_used_at: now,
        last_operation: "read",
        read_operations_today: 12,
        write_operations_today: 2,
      },
      {
        id: "credential:ios",
        name: "iPhone",
        kind: "api_credential",
        manageable: true,
        access: "read_only",
        status: "active",
        scope_ids: ["scope:root"],
        created_at: "2026-07-02T00:00:00Z",
        last_used_at: "2026-07-11T17:30:00Z",
        last_operation: "read",
        read_operations_today: 5,
        write_operations_today: 0,
      },
    ],
    coverage: { days: 7, activity: "tracked_operations_only" },
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
    "GET /api/v1/workspace/dashboard": defaultDashboard,
    "GET /api/v1/workspace/briefings": {
      status: "complete",
      data: {
        editions: [],
        limit: 7,
        truncated: false,
        next: null,
        workspace_generation: 12,
      },
    },
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
