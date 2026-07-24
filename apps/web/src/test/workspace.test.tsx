import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { installApiMock, renderApp } from "./renderApp";

const now = "2026-07-11T18:00:00Z";

describe("workspace continuity", () => {
  it("chains checkpoints without flattening unchanged structured state", async () => {
    const user = userEvent.setup();
    let submitted: unknown;
    installApiMock({
      "GET /api/v1/sessions/session_1": {
        status: "complete",
        corpus_revision: "revision:one",
        data: {
          id: "session_1",
          title: "Continuity test",
          status: "active",
          corpus_revision: "revision:one",
          checkpoint: {
            id: "checkpoint:old",
            title: "Old title",
            objective: "Keep all typed state",
            created_at: now,
            corpus_revision: "revision:one",
            source_refs: ["source:existing"],
            state: {
              title: "Old title",
              objective: "Keep all typed state",
              current_state: [{
                title: "Structured state",
                status: "active",
                provenance: { source_ref: "source:existing" },
              }],
              decisions: [{
                text: "Retain the decision metadata",
                authority: "confirmed",
              }],
              open_questions: ["What changed?"],
              next_actions: [{
                title: "Continue",
                owner: "agent",
                due_at: "2026-07-12T18:00:00Z",
              }],
              artifacts: ["artifact:one"],
              custom_context: {
                reasoning_mode: "evidence_first",
                token_budget: 24000,
              },
            },
          },
          initial_evidence: [{
            id: "result:one",
            source_id: "source:new",
            title: "New evidence",
          }],
          operations: [],
        },
      },
      "POST /api/v1/memory/checkpoint": async (request: Request) => {
        submitted = await request.json();
        return {
          status: "committed",
          data: {
            id: "checkpoint:new",
            title: "Updated title",
            created_at: now,
          },
        };
      },
    });
    renderApp("/sessions/session_1", "write-token");

    const title = await screen.findByRole("textbox", { name: "Title" });
    expect(title).toHaveValue("Old title");
    await user.clear(title);
    await user.type(title, "Updated title");
    await user.click(screen.getByRole("button", { name: "Commit" }));

    await waitFor(() => expect(submitted).toBeDefined());
    const payload = submitted as {
      parent_checkpoint_id: string;
      source_refs: string[];
      state: Record<string, unknown>;
    };
    expect(payload.parent_checkpoint_id).toBe("checkpoint:old");
    expect(payload.source_refs).toEqual(["source:existing", "source:new"]);
    expect(payload.state.current_state).toEqual([{
      title: "Structured state",
      status: "active",
      provenance: { source_ref: "source:existing" },
    }]);
    expect(payload.state.decisions).toEqual([{
      text: "Retain the decision metadata",
      authority: "confirmed",
    }]);
    expect(payload.state.next_actions).toEqual([{
      title: "Continue",
      owner: "agent",
      due_at: "2026-07-12T18:00:00Z",
    }]);
    expect(payload.state.custom_context).toEqual({
      reasoning_mode: "evidence_first",
      token_budget: 24000,
    });
  });

  it("loads every session page using the returned continuation token", async () => {
    const user = userEvent.setup();
    const requestedCursors: Array<string | null> = [];
    installApiMock({
      "GET /api/v1/sessions": (request: Request) => {
        const cursor = new URL(request.url).searchParams.get("cursor");
        requestedCursors.push(cursor);
        return cursor === "signed-next-page"
          ? {
              status: "complete",
              data: {
                items: [{
                  id: "session_second",
                  title: "Second page session",
                  status: "active",
                  corpus_revision: "revision:two",
                  updated_at: now,
                }],
                total: 2,
                continuation_token: null,
              },
            }
          : {
              status: "complete",
              data: {
                items: [{
                  id: "session_first",
                  title: "First page session",
                  status: "active",
                  corpus_revision: "revision:one",
                  updated_at: now,
                }],
                total: 2,
                continuation_token: "signed-next-page",
              },
            };
      },
    });
    renderApp("/work", "read-token");

    expect(await screen.findByText("First page session")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Load more" }));

    expect(await screen.findByText("Second page session")).toBeInTheDocument();
    expect(requestedCursors).toEqual([null, "signed-next-page"]);
  });
});
