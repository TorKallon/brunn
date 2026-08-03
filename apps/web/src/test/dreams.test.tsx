import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { installApiMock, renderApp } from "./renderApp";

describe("visible background maintenance", () => {
  it("lists proposal Markdown, jobs, and queues a focused dream pass", async () => {
    let dreamPayload: Record<string, unknown> | undefined;
    const fetchMock = installApiMock({
      "GET /api/v1/workspace/manifest": {
        status: "complete",
        data: {
          workspace_generation: 88,
          offset: 0,
          limit: 1000,
          truncated: false,
          entries: [
            {
              entry_ref: "entry:proposal-1",
              path: ".straylight/proposals/people-index.md",
              title: "People index refresh",
              kind: "markdown",
              media_type: "text/markdown",
              version: 1,
              content_hash: "sha256:proposal",
              size_bytes: 300,
              metadata: {
                kind: "dream_proposal",
                status: "needs_review",
              },
              created_at: "2026-07-26T18:00:00Z",
            },
            {
              entry_ref: "entry:ordinary",
              path: "Projects/Status.md",
              title: "Status",
              kind: "markdown",
              media_type: "text/markdown",
              version: 2,
              content_hash: "sha256:status",
              size_bytes: 100,
              metadata: { kind: "project" },
              created_at: "2026-07-26T17:00:00Z",
            },
          ],
        },
      },
      "GET /api/v1/workspace/changes": {
        status: "complete",
        data: {
          since_generation: 0,
          workspace_generation: 88,
          truncated: false,
          changes: [
            {
              generation: 88,
              operation: "create",
              path: ".straylight/proposals/people-index.md",
              version: 1,
              content_hash: "sha256:proposal",
              recorded_at: "2026-07-26T18:00:00Z",
            },
          ],
        },
      },
      "GET /api/v1/workspace/jobs": {
        status: "complete",
        data: {
          jobs: [
            {
              job_ref: "job:dream-1",
              kind: "dream_workspace",
              status: "queued",
              payload: { from_generation: 80, to_generation: 88 },
              watermark: 80,
              attempts: 0,
              available_at: "2026-07-26T18:00:00Z",
              created_at: "2026-07-26T18:00:00Z",
            },
          ],
          offset: 0,
          limit: 100,
          truncated: false,
        },
      },
      "POST /api/v1/workspace/dreams": async (request: Request) => {
        dreamPayload = (await request.json()) as Record<string, unknown>;
        return {
          status: "committed",
          data: {
            job_ref: "job:dream-2",
            status: "queued",
            workspace_generation: 88,
            watermark: 80,
            no_op: false,
          },
        };
      },
      "POST /api/v1/workspace/read": {
        status: "complete",
        data: {
          workspace_generation: 88,
          items: [
            {
              reference: "entry:proposal-1",
              path: ".straylight/proposals/people-index.md",
              title: "People index refresh",
              version: 1,
              version_ref: "entry-version:proposal-v1",
              content_hash: "sha256:proposal",
              media_type: "text/markdown",
              view: "full",
              status: "complete",
              text:
                "# Proposal\n\nRefresh the people index from three changed source notes.",
              metadata: { kind: "dream_proposal", status: "needs_review" },
              updated_at: "2026-07-26T18:00:00Z",
            },
          ],
        },
      },
    });
    const user = userEvent.setup();
    renderApp("/dreams", "read-token");

    expect(
      await screen.findByRole("row", {
        name: "Read proposal People index refresh",
      }),
    ).toBeInTheDocument();
    expect(screen.queryByText("Projects/Status.md")).not.toBeInTheDocument();
    expect(screen.getByText("1 recent jobs loaded")).toBeInTheDocument();
    expect(screen.getByText("Dream Workspace")).toBeInTheDocument();
    expect(screen.getByText("No workspace endpoint available")).toBeInTheDocument();
    expect(screen.getByText("No proposal selected")).toBeInTheDocument();

    await user.type(screen.getByRole("textbox", { name: "Focus" }), "People notes");
    await user.click(screen.getByRole("button", { name: "Run now" }));
    expect(await screen.findByText("job:dream-2")).toBeInTheDocument();
    expect(dreamPayload).toEqual({ focus: "People notes" });

    await user.click(
      screen.getByRole("row", {
        name: "Read proposal People index refresh",
      }),
    );
    expect(
      await screen.findByLabelText("People index refresh content"),
    ).toHaveTextContent(
      "Proposal Refresh the people index from three changed source notes.",
    );
    expect(screen.queryByRole("button", { name: "Approve" })).not.toBeInTheDocument();
    expect(
      fetchMock.mock.calls.some(([input]) =>
        new URL(String(input), window.location.origin).pathname.startsWith(
          "/api/v1/dreams",
        ),
      ),
    ).toBe(false);
  });
});
