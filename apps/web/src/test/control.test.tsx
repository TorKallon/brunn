import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

const credential = {
  id: "credential_1",
  name: "Codex read only",
  access: "read_only",
  scope_ids: ["scope_1"],
  created_at: "2026-07-26T18:00:00Z",
  last_used_at: null,
  revoked_at: null,
};

describe("workspace activity and access", () => {
  it("shows recent changed paths from the monotonic workspace feed", async () => {
    installApiMock({
      "GET /api/v1/workspace/manifest": {
        status: "complete",
        data: {
          workspace_generation: 500,
          entries: [],
          offset: 0,
          limit: 1000,
          truncated: false,
        },
      },
      "GET /api/v1/workspace/changes": (request: Request) => {
        expect(
          new URL(request.url).searchParams.get("since_generation"),
        ).toBe("300");
        return {
          status: "complete",
          data: {
            since_generation: 0,
            workspace_generation: 500,
            truncated: false,
            changes: [
              {
                generation: 500,
                operation: "update",
                path: "Projects/Brunn/Status.md",
                version: 12,
                content_hash: "sha256:status",
                recorded_at: "2026-07-26T18:00:00Z",
              },
            ],
          },
        };
      },
    });
    renderApp("/control", "owner-token");

    expect(
      await screen.findByText("Projects/Brunn/Status.md"),
    ).toBeInTheDocument();
    expect(screen.getByText("g500")).toBeInTheDocument();
    expect(screen.getByText("v12")).toBeInTheDocument();

    const user = userEvent.setup();
    await user.click(screen.getByRole("tab", { name: "Checkpoints" }));
    expect(screen.getByText("No checkpoint selected")).toBeInTheDocument();
  });

  it("switches among the four workspace usage rankings", async () => {
    const requestedSorts: string[] = [];
    installApiMock({
      "GET /api/v1/workspace/usage": (request: Request) => {
        const sort =
          new URL(request.url).searchParams.get("sort") ?? "most_used";
        requestedSorts.push(sort);
        return {
          status: "complete",
          data: {
            sort,
            offset: 0,
            limit: 100,
            entries: [
              {
                entry_ref: `entry:${sort}`,
                path: `Usage/${sort}.md`,
                title: sort,
                kind: "markdown",
                read_count: 4,
                search_count: 3,
                total_uses: 7,
                first_used_at: "2026-07-20T18:00:00Z",
                last_used_at: "2026-07-26T18:00:00Z",
              },
            ],
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/control", "owner-token");

    await user.click(await screen.findByRole("tab", { name: "Usage" }));
    expect(await screen.findByText("Usage/most_used.md")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Least used" }));
    expect(await screen.findByText("Usage/least_used.md")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Most recent" }));
    expect(
      await screen.findByText("Usage/most_recently_used.md"),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Least recent" }));
    expect(
      await screen.findByText("Usage/least_recently_used.md"),
    ).toBeInTheDocument();
    expect(requestedSorts).toEqual([
      "most_used",
      "least_used",
      "most_recently_used",
      "least_recently_used",
    ]);
  });

  it("keeps credential management behind owner authority", async () => {
    installApiMock({
      "GET /api/v1/credentials": {
        status: "complete",
        data: { items: [credential] },
      },
    });
    const user = userEvent.setup();
    const owner = renderApp("/control", "owner-token");

    await user.click(await screen.findByRole("tab", { name: "Access" }));
    expect(
      await screen.findByRole("button", { name: "New credential" }),
    ).toBeEnabled();
    expect(screen.getByText("Codex read only")).toBeInTheDocument();
    owner.unmount();

    const writerMe = structuredClone(defaultMe);
    writerMe.data.capabilities = writerMe.data.capabilities.filter(
      (capability) => capability !== "credential:manage",
    );
    installApiMock({
      "GET /api/v1/me": writerMe,
      "GET /api/v1/credentials": {
        status: "complete",
        data: { items: [credential] },
      },
    });
    renderApp("/control", "writer-token");

    await user.click(await screen.findByRole("tab", { name: "Access" }));
    expect(
      await screen.findByRole("button", { name: "New credential" }),
    ).toBeDisabled();
  });

  it("defaults new credentials to read/write", async () => {
    let createdBody: unknown;
    installApiMock({
      "GET /api/v1/credentials": {
        status: "complete",
        data: { items: [] },
      },
      "POST /api/v1/credentials": async (request: Request) => {
        createdBody = await request.json();
        return {
          status: "complete",
          data: {
            id: "credential_created",
            name: "Codex on Erebus RW",
            access: "read_write",
            scope_ids: ["scope_1"],
            token: "sl_created_once",
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/control", "owner-token");

    await user.click(await screen.findByRole("tab", { name: "Access" }));
    await user.click(screen.getByRole("button", { name: "New credential" }));
    expect(screen.getByRole("combobox", { name: "Access" })).toHaveValue(
      "read_write",
    );
    await user.type(screen.getByRole("textbox", { name: "Name" }), "Codex on Erebus RW");
    await user.click(screen.getByRole("button", { name: "Create" }));

    expect(await screen.findByText("Credential created")).toBeInTheDocument();
    expect(createdBody).toEqual({
      name: "Codex on Erebus RW",
      access: "read_write",
      scope_ids: ["scope_1"],
    });
  });
});
