import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { installApiMock, renderApp } from "./renderApp";

describe("workspace search and exact read", () => {
  it("renders merged current entries and reads the selected exact reference", async () => {
    let searchPayload: Record<string, unknown> | undefined;
    let readPayload: Record<string, unknown> | undefined;
    installApiMock({
      "POST /api/v1/workspace/search": async (request: Request) => {
        searchPayload = (await request.json()) as Record<string, unknown>;
        return {
          status: "partial",
          gaps: [{ kind: "retrieval_lane_failed", lane: "semantic" }],
          data: {
            workspace_generation: 51,
            results: [
              {
                id: "workspace-search",
                query_status: "partial",
                lane_failures: ["semantic"],
                candidates: [
                  {
                    entry_id: "trip-entry",
                    path: "Trips/Switzerland/Itinerary.md",
                    title: "Switzerland itinerary",
                    version: 7,
                    content_sha256: "trip-hash",
                    heading: "Transfers",
                    excerpt:
                      "The transfer departs Zurich at 09:40 on February 14.",
                    score: 3.91,
                    lanes: ["exact", "lexical"],
                    updated_at: "2026-07-26T18:00:00Z",
                  },
                ],
              },
            ],
          },
        };
      },
      "POST /api/v1/workspace/read": async (request: Request) => {
        readPayload = (await request.json()) as Record<string, unknown>;
        return {
          status: "complete",
          data: {
            workspace_generation: 51,
            items: [
              {
                reference: "entry:trip-entry",
                path: "Trips/Switzerland/Itinerary.md",
                title: "Switzerland itinerary",
                version: 7,
                version_ref: "entry-version:trip-v7",
                content_hash: "sha256:trip-hash",
                media_type: "text/markdown",
                view: "full",
                status: "complete",
                text: "# Switzerland itinerary\n\nThe transfer is confirmed.",
                metadata: { authority: "source" },
                updated_at: "2026-07-26T18:00:00Z",
              },
            ],
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/explore", "search-token");

    const input = await screen.findByLabelText("Search workspace");
    await user.type(input, "Zurich transfer");
    await user.click(screen.getByRole("button", { name: "Search" }));

    expect(
      await screen.findByRole("heading", { name: "Switzerland itinerary" }),
    ).toBeInTheDocument();
    expect(screen.getByText("3.910")).toBeInTheDocument();
    expect(screen.getByText("Partial, 1 gap")).toBeInTheDocument();
    expect(searchPayload).toMatchObject({
      queries: [
        {
          query: "Zurich transfer",
          modes: ["exact", "lexical", "semantic"],
          limit: 20,
          sort: "best_match",
        },
      ],
    });

    await user.click(screen.getByRole("button", { name: "Open entry" }));
    expect(
      await screen.findByLabelText("Switzerland itinerary content"),
    ).toHaveTextContent("Switzerland itinerary The transfer is confirmed.");
    await waitFor(() =>
      expect(readPayload).toMatchObject({
        requests: [{ ref: "entry:trip-entry", version: 7, view: "full" }],
      }),
    );

    await user.click(screen.getByRole("button", { name: "Source" }));
    expect(screen.getByLabelText("Switzerland itinerary content")).toHaveTextContent(
      "# Switzerland itinerary The transfer is confirmed.",
    );
  });

  it("defaults to best match and reruns the search when the sort changes", async () => {
    const searchPayloads: Array<Record<string, unknown>> = [];
    const candidates = [
      {
        reference: "entry:beta",
        path: "Notes/Beta.md",
        title: "Beta",
        version: 1,
        excerpt: "Best relevance match",
        updated_at: "2026-07-24T18:00:00Z",
      },
      {
        reference: "entry:zulu",
        path: "Notes/Zulu.md",
        title: "Zulu",
        version: 1,
        excerpt: "Newest match",
        updated_at: "2026-07-26T18:00:00Z",
      },
      {
        reference: "entry:alpha",
        path: "Notes/Alpha.md",
        title: "Alpha",
        version: 1,
        excerpt: "Middle match",
        updated_at: "2026-07-25T18:00:00Z",
      },
    ];
    installApiMock({
      "POST /api/v1/workspace/search": async (request: Request) => {
        const payload = (await request.json()) as {
          queries: Array<{ sort: string }>;
        };
        searchPayloads.push(payload as unknown as Record<string, unknown>);
        const returnedCandidates =
          payload.queries[0].sort === "last_modified"
            ? [candidates[1], candidates[2], candidates[0]]
            : payload.queries[0].sort === "title"
              ? [candidates[2], candidates[0], candidates[1]]
              : candidates;
        return {
          status: "complete",
          data: {
            workspace_generation: 51,
            results: [
              {
                id: "workspace-search",
                query_status: "complete",
                candidates: returnedCandidates,
              },
            ],
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/explore", "search-token");

    await user.type(await screen.findByLabelText("Search workspace"), "notes");
    expect(screen.getByLabelText("Sort results")).toHaveValue("best_match");
    await user.click(screen.getByRole("button", { name: "Search" }));
    expect(
      (await screen.findAllByRole("heading", { level: 3 })).map(
        (heading) => heading.textContent,
      ),
    ).toEqual(["Beta", "Zulu", "Alpha"]);

    await user.selectOptions(screen.getByLabelText("Sort results"), "last_modified");
    await waitFor(() => expect(searchPayloads).toHaveLength(2));
    expect(screen.getAllByRole("heading", { level: 3 }).map((heading) => heading.textContent))
      .toEqual(["Zulu", "Alpha", "Beta"]);
    expect(searchPayloads.at(-1)).toMatchObject({
      queries: [{ query: "notes", sort: "last_modified" }],
    });

    await user.selectOptions(screen.getByLabelText("Sort results"), "title");
    await waitFor(() => expect(searchPayloads).toHaveLength(3));
    expect(screen.getAllByRole("heading", { level: 3 }).map((heading) => heading.textContent))
      .toEqual(["Alpha", "Beta", "Zulu"]);
    expect(searchPayloads.at(-1)).toMatchObject({
      queries: [{ query: "notes", sort: "title" }],
    });
  });

  it("keeps the sort locked to the in-flight search request", async () => {
    const searchPayloads: Array<Record<string, unknown>> = [];
    let releaseSearch!: () => void;
    const searchGate = new Promise<void>((resolve) => {
      releaseSearch = resolve;
    });
    installApiMock({
      "POST /api/v1/workspace/search": async (request: Request) => {
        searchPayloads.push((await request.json()) as Record<string, unknown>);
        await searchGate;
        return {
          status: "complete",
          data: {
            workspace_generation: 51,
            results: [{ id: "workspace-search", candidates: [] }],
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/explore", "search-token");

    await user.type(await screen.findByLabelText("Search workspace"), "notes");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => expect(searchPayloads).toHaveLength(1));

    expect(screen.getByLabelText("Sort results")).toBeDisabled();
    expect(screen.getByLabelText("Sort results")).toHaveValue("best_match");
    releaseSearch();
    await screen.findByText("No candidates returned");
    expect(searchPayloads).toHaveLength(1);
  });

  it("keeps the read form and displayed entry on the latest concurrent read", async () => {
    const readResolvers = new Map<
      string,
      (response: ReturnType<typeof readEnvelope>) => void
    >();
    const readRequests: Array<{ ref?: string; version?: number; view?: string }> = [];
    installApiMock({
      "POST /api/v1/workspace/search": async () => ({
        status: "complete",
        data: {
          workspace_generation: 51,
          results: [{
            id: "workspace-search",
            candidates: [
              {
                reference: "entry:a",
                path: "Notes/A.md",
                title: "Entry A",
                version: 4,
                excerpt: "First",
                updated_at: "2026-08-02T00:00:00Z",
              },
              {
                reference: "entry:b",
                path: "Notes/B.md",
                title: "Entry B",
                version: 9,
                excerpt: "Second",
                updated_at: "2026-08-01T00:00:00Z",
              },
            ],
          }],
        },
      }),
      "POST /api/v1/workspace/read": async (request: Request) => {
        const payload = (await request.json()) as {
          requests: Array<{ ref?: string; version?: number; view?: string }>;
        };
        const readRequest = payload.requests[0];
        readRequests.push(readRequest);
        const ref = readRequest.ref ?? "";
        return new Promise<ReturnType<typeof readEnvelope>>((resolve) => {
          readResolvers.set(ref, resolve);
        });
      },
    });
    const user = userEvent.setup();
    renderApp("/explore", "search-token");

    await user.type(await screen.findByLabelText("Search workspace"), "entries");
    await user.click(screen.getByRole("button", { name: "Search" }));
    const openButtons = await screen.findAllByRole("button", { name: "Open entry" });
    await user.click(openButtons[0]);
    await user.click(screen.getByRole("tab", { name: /Search/ }));
    await user.click((await screen.findAllByRole("button", { name: "Open entry" }))[1]);

    await waitFor(() => {
      expect(readResolvers.has("entry:a")).toBe(true);
      expect(readResolvers.has("entry:b")).toBe(true);
    });
    expect(readRequests).toEqual([
      { ref: "entry:a", version: 4, view: "full" },
      { ref: "entry:b", version: 9, view: "full" },
    ]);
    readResolvers.get("entry:b")?.(readEnvelope({
      reference: "entry:b",
      path: "Notes/B.md",
      title: "Entry B",
      text: "# Entry B",
    }));
    expect(await screen.findByLabelText("Entry B content")).toBeInTheDocument();

    readResolvers.get("entry:a")?.(readEnvelope({
      reference: "entry:a",
      path: "Notes/A.md",
      title: "Entry A",
      text: "# Entry A",
    }));
    await waitFor(() => {
      expect(screen.getByLabelText("Path or entry reference")).toHaveValue("entry:b");
      expect(screen.getByLabelText("Entry B content")).toBeInTheDocument();
    });
  });

  it("opens relative and wiki-linked entries through exact path candidates", async () => {
    const readRequests: Array<Record<string, unknown>> = [];
    installApiMock({
      "POST /api/v1/workspace/search": async (request: Request) => {
        await request.json();
        return {
          status: "complete",
          data: {
            workspace_generation: 51,
            results: [
              {
                id: "workspace-search",
                candidates: [
                  {
                    reference: "entry:current",
                    path: "sources/Projects/Current/Entry.md",
                    title: "Current entry",
                    version: 3,
                    excerpt: "Entry with links",
                    updated_at: "2026-07-26T18:00:00Z",
                  },
                ],
              },
            ],
          },
        };
      },
      "POST /api/v1/workspace/read": async (request: Request) => {
        const payload = (await request.json()) as {
          requests: Array<{ ref?: string; path?: string; link_target?: string }>;
        };
        readRequests.push(payload.requests[0]);
        const target = payload.requests[0];
        if (target.ref === "entry:current") {
          return readEnvelope({
            reference: "entry:current",
            path: "sources/Projects/Current/Entry.md",
            title: "Current entry",
            text: "# Current entry\n\nOpen [Related](../Related.md).",
          });
        }
        if (target.path === "sources/Projects/Related.md") {
          return readEnvelope({
            reference: "entry:related",
            path: "sources/Projects/Related.md",
            title: "Related",
            text: "# Related\n\nOpen [[Reference]].",
          });
        }
        if (target.link_target === "Reference") {
          return readEnvelope({
            reference: "entry:reference",
            path: "sources/Knowledge/Reference.md",
            title: "Reference",
            text: "# Reference\n\nResolved safely.",
          });
        }
        return {
          status: "degraded",
          gaps: [{ kind: "read_entries_not_found" }],
          data: {
            workspace_generation: 51,
            items: [
              {
                status: "not_found",
                path: target.path,
                reference: target.ref,
                error: { code: "entry_not_found", message: "Not found" },
              },
            ],
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/explore", "search-token");

    await user.type(await screen.findByLabelText("Search workspace"), "current");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.click(await screen.findByRole("button", { name: "Open entry" }));
    await user.click(await screen.findByRole("link", { name: "Related" }));
    expect(await screen.findByLabelText("Related content")).toBeInTheDocument();
    expect(readRequests).toContainEqual({
      path: "sources/Projects/Related.md",
      view: "full",
    });

    await user.click(screen.getByRole("link", { name: "Reference" }));
    expect(await screen.findByLabelText("Reference content")).toHaveTextContent(
      "Resolved safely.",
    );
    expect(readRequests).toContainEqual({
      path: "sources/Projects/Reference.md",
      view: "full",
    });
    expect(readRequests).toContainEqual({ link_target: "Reference", view: "full" });
  });

  it("opens entry-navigation URLs from other workspace readers", async () => {
    let readPayload: Record<string, unknown> | undefined;
    installApiMock({
      "POST /api/v1/workspace/read": async (request: Request) => {
        readPayload = (await request.json()) as Record<string, unknown>;
        return readEnvelope({
          reference: "entry:linked",
          path: "sources/Linked.md",
          title: "Linked",
          text: "# Linked\n\nOpened from another reader.",
        });
      },
    });

    renderApp(
      "/explore?entryPath=sources%2FLinked.md&fallbackQuery=Linked",
      "search-token",
    );

    expect(await screen.findByLabelText("Linked content")).toHaveTextContent(
      "Opened from another reader.",
    );
    expect(readPayload).toMatchObject({
      requests: [{ path: "sources/Linked.md", view: "full" }],
    });
  });

  it("fails closed instead of claiming basename uniqueness from ranked search results", async () => {
    let searchCalls = 0;
    installApiMock({
      "POST /api/v1/workspace/search": async () => {
        searchCalls += 1;
        throw new Error("entry links must not use ranked search as a uniqueness proof");
      },
      "POST /api/v1/workspace/read": async (request: Request) => {
        const payload = (await request.json()) as {
          requests: Array<{ ref?: string; path?: string }>;
        };
        return {
          status: "degraded",
          data: {
            workspace_generation: 51,
            items: [{
              status: "not_found",
              path: payload.requests[0].path,
              reference: payload.requests[0].ref,
              error: { code: "entry_not_found", message: "Not found" },
            }],
          },
        };
      },
    });

    renderApp(
      "/explore?entryPath=sources%2FMissing.md&alternatePaths=Missing.md&linkTarget=Missing&fallbackQuery=Missing",
      "search-token",
    );

    expect(await screen.findByText(/Use the entry's full path/)).toBeInTheDocument();
    expect(searchCalls).toBe(0);
  });
});

function readEnvelope(entry: {
  reference: string;
  path: string;
  title: string;
  text: string;
}) {
  return {
    status: "complete",
    data: {
      workspace_generation: 51,
      items: [
        {
          ...entry,
          version: 1,
          version_ref: `${entry.reference}:v1`,
          content_hash: "sha256:test",
          media_type: "text/markdown",
          view: "full",
          status: "complete",
          metadata: {},
          updated_at: "2026-07-26T18:00:00Z",
        },
      ],
    },
  };
}
