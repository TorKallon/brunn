import { describe, expect, it, vi } from "vitest";
import { ApiError, createApiClient } from "../lib/api";
import type { BriefingListData } from "../lib/types";
import { workspaceEntryKind } from "../lib/workspace";

function installFetch(body: unknown, status = 200) {
  const fetchMock = vi.fn(
    async (_input: RequestInfo | URL, _init?: RequestInit) =>
      new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
      }),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function requestOf(fetchMock: ReturnType<typeof installFetch>): Request {
  const [input, init] = fetchMock.mock.calls[0];
  return input instanceof Request
    ? input
    : new Request(new URL(String(input), "https://brunn.test"), init);
}

const api = () => createApiClient();

describe("briefing api client", () => {
  it("lists editions with the default limit", async () => {
    const fetchMock = installFetch({
      status: "complete",
      data: {
        editions: [],
        limit: 14,
        truncated: false,
        next: null,
        workspace_generation: 7,
      },
    });
    const envelope = await api().briefingsList();
    const request = requestOf(fetchMock);
    const url = new URL(request.url);
    expect(request.method).toBe("GET");
    expect(url.pathname).toBe("/api/v1/workspace/briefings");
    expect(url.searchParams.get("limit")).toBe("14");
    expect(url.searchParams.has("after_path")).toBe(false);
    expect(request.credentials).toBe("same-origin");
    expect(request.headers.get("Authorization")).toBeNull();
    expect(envelope.status).toBe("complete");
    expect(envelope.data.workspace_generation).toBe(7);
  });

  it("encodes the keyset cursor when paging editions", async () => {
    const fetchMock = installFetch({
      status: "complete",
      data: {
        editions: [],
        limit: 5,
        truncated: false,
        next: null,
        workspace_generation: 7,
      },
    });
    await api().briefingsList(5, "Briefings/2026/Morning briefing - 2026-08-01.md");
    const url = new URL(requestOf(fetchMock).url);
    expect(url.searchParams.get("limit")).toBe("5");
    expect(url.searchParams.get("after_path")).toBe(
      "Briefings/2026/Morning briefing - 2026-08-01.md",
    );
  });

  it("fetches an edition by date and slug, optionally pinned to a version", async () => {
    const edition = {
      status: "complete",
      data: {
        path: "Briefings/2026/Morning briefing - 2026-08-01.md",
        entry_ref: "entry:11111111-1111-4111-8111-111111111111",
        version: 2,
        current_version: 3,
        date: "2026-08-01",
        edition: "morning",
        briefing: null,
        markdown: "# Morning briefing - 2026-08-01",
        created_at: "2026-08-01T06:30:00Z",
        versions: [
          { version: 1, created_at: "2026-08-01T06:30:00Z" },
          { version: 2, created_at: "2026-08-01T10:00:00Z" },
        ],
        workspace_generation: 9,
      },
    };
    let fetchMock = installFetch(edition);
    await api().briefingGet("2026-08-01", "morning");
    let url = new URL(requestOf(fetchMock).url);
    expect(url.pathname).toBe("/api/v1/workspace/briefings/2026-08-01/morning");
    expect(url.search).toBe("");

    fetchMock = installFetch(edition);
    const envelope = await api().briefingGet("2026-08-01", "morning", 2);
    url = new URL(requestOf(fetchMock).url);
    expect(url.searchParams.get("version")).toBe("2");
    expect(envelope.data.versions).toHaveLength(2);
  });

  it("fetches the topics snapshot", async () => {
    const fetchMock = installFetch({
      status: "complete",
      data: {
        topics: [],
        pending_requests: [],
        feedback_path: "Briefings/Feedback/2026-08.md",
        feedback_tail: [],
        workspace_generation: 9,
      },
    });
    const envelope = await api().briefingTopics();
    const request = requestOf(fetchMock);
    expect(request.method).toBe("GET");
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/workspace/briefings/topics",
    );
    expect(envelope.data.feedback_path).toBe("Briefings/Feedback/2026-08.md");
  });

  it("posts item actions as json without undefined fields", async () => {
    document.cookie = "brunn_csrf=briefing-csrf; Path=/";
    const fetchMock = installFetch({
      status: "committed",
      data: {
        action: "feedback",
        path: "Briefings/Feedback/2026-08.md",
        entry_ref: "entry:22222222-2222-4222-8222-222222222222",
        version_ref: "entry-version:33333333-3333-4333-8333-333333333333",
        version: 4,
        content_hash: "sha256:feedback",
      },
    });
    const envelope = await api().briefingItemAction({
      action: "feedback",
      edition_ref: "entry:11111111-1111-4111-8111-111111111111",
      item_id: "openai-o5",
      verdict: "useful",
    });
    const request = requestOf(fetchMock);
    expect(request.method).toBe("POST");
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/workspace/briefings/items/action",
    );
    expect(request.headers.get("Content-Type")).toBe("application/json");
    expect(request.headers.get("X-CSRF-Token")).toBe("briefing-csrf");
    expect(await request.json()).toEqual({
      action: "feedback",
      edition_ref: "entry:11111111-1111-4111-8111-111111111111",
      item_id: "openai-o5",
      verdict: "useful",
    });
    expect(envelope.status).toBe("committed");
    expect(envelope.data.version).toBe(4);
  });

  it("wraps bare payloads in a complete envelope", async () => {
    installFetch({
      editions: [],
      limit: 14,
      truncated: false,
      next: null,
      workspace_generation: 1,
    } satisfies BriefingListData);
    const envelope = await api().briefingsList();
    expect(envelope.status).toBe("complete");
    expect(envelope.data.limit).toBe(14);
  });

  it("surfaces briefing error codes as ApiError", async () => {
    installFetch(
      {
        error: {
          code: "request_exists",
          message: "a pending expansion request already exists for this item",
        },
      },
      409,
    );
    await expect(
      api().briefingItemAction({
        action: "expand",
        edition_ref: "entry:11111111-1111-4111-8111-111111111111",
        item_id: "openai-o5",
      }),
    ).rejects.toMatchObject({
      name: "ApiError",
      status: 409,
      code: "request_exists",
    } satisfies Partial<ApiError>);
  });
});

describe("workspaceEntryKind briefing classification", () => {
  it("prefers explicit briefing metadata kinds", () => {
    expect(
      workspaceEntryKind({
        path: "Briefings/2026/Morning briefing - 2026-08-01.md",
        metadata: { kind: "briefing_edition" },
      }),
    ).toBe("briefing_edition");
    expect(
      workspaceEntryKind({
        path: "Briefings/Topics/frontier-labs.md",
        metadata: { kind: "briefing_topic" },
      }),
    ).toBe("briefing_topic");
    expect(
      workspaceEntryKind({
        path: "Briefings/Requests/2026-08-01 - openai-o5.md",
        metadata: { kind: "briefing_request" },
      }),
    ).toBe("briefing_request");
  });

  it("falls back to briefing path prefixes when metadata is silent", () => {
    expect(
      workspaceEntryKind({
        path: "Briefings/Topics/frontier-labs.md",
        metadata: {},
      }),
    ).toBe("briefing_topic");
    expect(
      workspaceEntryKind({
        path: "Briefings/Requests/2026-08-01 - openai-o5.md",
        metadata: {},
      }),
    ).toBe("briefing_request");
    expect(
      workspaceEntryKind({
        path: "Briefings/2026/Morning briefing - 2026-08-01.md",
        metadata: {},
      }),
    ).toBe("briefing_edition");
    expect(
      workspaceEntryKind({ path: "Trips/Europe 2026.md", metadata: {} }),
    ).toBe("entry");
  });
});
