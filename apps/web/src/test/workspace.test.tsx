import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { installApiMock, renderApp } from "./renderApp";

const now = "2026-07-26T18:00:00Z";

describe("simplified workspace continuity", () => {
  it("opens bounded evidence, exact-reads it, and writes a compact checkpoint", async () => {
    const user = userEvent.setup();
    let openPayload: Record<string, unknown> | undefined;
    let checkpointPayload: Record<string, unknown> | undefined;
    installApiMock({
      "POST /api/v1/workspace/open": async (request: Request) => {
        openPayload = (await request.json()) as Record<string, unknown>;
        return {
          status: "complete",
          session_id: "session:open-1",
          corpus_revision: "generation:42",
          data: {
            workspace_generation: 42,
            checkpoint: {
              checkpoint_id: "checkpoint:parent",
              path: ".brunn/checkpoints/parent.md",
              workspace_generation: 39,
              text: "# Checkpoint\n\nContinue the trip plan.",
              source_entries: ["entry:trip"],
            },
            changes_since_checkpoint: [
              {
                generation: 41,
                operation: "update",
                path: "Trips/Europe 2026.md",
                version: 4,
                content_hash: "sha256:change",
                recorded_at: now,
              },
            ],
            retrieval_sufficiency: {
              status: "bounded_evidence",
              complete_source_count: 1,
              selected_source_count: 1,
            },
            evidence: [
              {
                reference: "entry:trip",
                path: "Trips/Europe 2026.md",
                title: "Europe 2026",
                version: 4,
                content_hash: "sha256:trip",
                representation: "complete_source",
                text: "# Europe 2026\n\nRail reservations remain open.",
                why_selected: ["exact", "lexical"],
                score: 10,
              },
            ],
          },
        };
      },
      "POST /api/v1/workspace/read": {
        status: "complete",
        data: {
          workspace_generation: 42,
          items: [
            {
              reference: "entry:trip",
              path: "Trips/Europe 2026.md",
              title: "Europe 2026",
              version: 4,
              version_ref: "entry-version:trip-v4",
              content_hash: "sha256:trip",
              media_type: "text/markdown",
              view: "full",
              status: "complete",
              text: "# Europe 2026\n\nRail reservations remain open.",
              metadata: { authority: "owner" },
              updated_at: now,
            },
          ],
        },
      },
      "POST /api/v1/workspace/checkpoint": async (request: Request) => {
        checkpointPayload = (await request.json()) as Record<string, unknown>;
        return {
          status: "committed",
          session_id: "session:open-1",
          data: {
            checkpoint_id: "checkpoint:next",
            checkpoint_ref: "checkpoint:next",
            path: ".brunn/checkpoints/next.md",
            workspace_generation: 43,
            source_entries: ["entry:trip"],
            write: {
              entry_ref: "entry:checkpoint",
              version_ref: "entry-version:checkpoint-v1",
              path: ".brunn/checkpoints/next.md",
              version: 1,
              content_hash: "sha256:checkpoint",
              workspace_generation: 43,
              no_op: false,
            },
          },
        };
      },
    });
    renderApp("/work", "write-token");

    await user.type(
      await screen.findByRole("textbox", { name: "Goal" }),
      "Continue Europe planning",
    );
    await user.type(
      screen.getByRole("textbox", { name: "Resume checkpoint" }),
      "checkpoint:parent",
    );
    await user.click(screen.getByRole("button", { name: "Open" }));

    expect(await screen.findByText("Europe 2026")).toBeInTheDocument();
    expect(screen.getAllByText("Trips/Europe 2026.md")).toHaveLength(2);
    expect(
      screen.getByRole("heading", { name: "Changes since checkpoint" }),
    ).toBeInTheDocument();
    expect(openPayload).toMatchObject({
      task: "Continue Europe planning",
      resume_checkpoint_ref: "checkpoint:parent",
      token_budget: 24000,
    });

    await user.click(screen.getByRole("button", { name: "Read exact" }));
    expect(
      await screen.findByLabelText("Europe 2026 content"),
    ).toBeInTheDocument();

    await user.type(
      screen.getByRole("textbox", { name: "Current state" }),
      "Rail reservations remain open",
    );
    await user.type(
      screen.getByRole("textbox", { name: "Decisions" }),
      "Use the confirmed flight itinerary",
    );
    await user.type(
      screen.getByRole("textbox", { name: "Next actions" }),
      "Book seat reservations",
    );
    await user.click(screen.getByRole("button", { name: "Save checkpoint" }));

    await waitFor(() => expect(checkpointPayload).toBeDefined());
    expect(checkpointPayload).toMatchObject({
      session_id: "session:open-1",
      parent_checkpoint_id: "checkpoint:parent",
      source_refs: ["entry:trip"],
      state: {
        objective: "Continue Europe planning",
        current_state: ["Rail reservations remain open"],
        decisions: ["Use the confirmed flight itinerary"],
        next_actions: ["Book seat reservations"],
      },
    });
    expect(await screen.findByText("checkpoint:next")).toBeInTheDocument();
  });

  it("redirects retired durable-session URLs to the stateless workspace", async () => {
    installApiMock();
    renderApp("/sessions/legacy-session", "read-token");

    expect(
      await screen.findByRole("heading", { name: "Workspace" }),
    ).toBeInTheDocument();
    expect(screen.getByText("No context packet open")).toBeInTheDocument();
  });
});
