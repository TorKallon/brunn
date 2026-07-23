import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { installApiMock, renderApp } from "./renderApp";

describe("capture receipt", () => {
  it("shows the durable commit receipt after a successful save", async () => {
    installApiMock({
      "POST /api/v1/memory/save": async (request: Request) => {
        const payload = await request.json() as {
          base_corpus_revision: string;
          idempotency_key: string;
          intent: string;
          source_refs: Array<{ ref: string }>;
          items: Array<{ action: string; kind: string; payload: { title: string; source_kind: string } }>;
        };
        expect(payload.base_corpus_revision).toBe("rev_001");
        expect(payload.intent).toBe("capture:user_input");
        expect(payload.source_refs).toEqual([{ ref: expect.stringMatching(/^capture:/) }]);
        expect(payload.items).toHaveLength(1);
        expect(payload.items[0]).toMatchObject({
          action: "create",
          kind: "source",
          payload: { title: "Trip confirmation", source_kind: "user_input" },
        });
        expect(payload.idempotency_key).toBeTruthy();
        return {
          status: "complete",
          corpus_revision: "rev_002",
          data: {
            id: "receipt_42",
            operation_id: "operation_42",
            status: "committed",
            base_corpus_revision: "rev_001",
            resulting_corpus_revision: "rev_002",
            created: [{ kind: "object", id: "object_trip" }],
            revised: [],
            deduplicated: [],
            indexing: { exact: "ready", lexical: "ready", semantic: "processing" },
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/capture", "write-token");

    await user.click(await screen.findByRole("tab", { name: "Canonical save" }));
    await user.type(await screen.findByLabelText("Source title"), "Trip confirmation");
    await user.click(screen.getByRole("button", { name: "Save change" }));

    expect(await screen.findByRole("heading", { name: "Commit receipt" })).toBeInTheDocument();
    expect(screen.getByText("receipt_42")).toBeInTheDocument();
    expect(screen.getByText("rev_002")).toBeInTheDocument();
    expect(screen.getByText("Processing")).toBeInTheDocument();
  });

  it("sends ordinary source content through automatic capture and renders a safe draft", async () => {
    installApiMock({
      "POST /api/v1/memory/capture": async (request: Request) => {
        const payload = await request.json() as {
          content: string;
          source: { title: string; origin: string };
          scope: string;
          mode: string;
        };
        expect(payload.content).toBe("The hotel moved check-in to 4 PM.");
        expect(payload.source).toMatchObject({ title: "Hotel message", origin: "external" });
        expect(payload.scope).toBe("scope_1");
        expect(payload.mode).toBe("auto");
        return {
          status: "needs_review",
          corpus_revision: "rev_001",
          data: {
            status: "needs_review",
            summary: "The arrangement target is ambiguous.",
            issues: [{ code: "consequential_ambiguity", item: 0 }],
            unresolved: ["Which reservation changed?"],
            draft: { save_request: { intent: "capture:user_input", items: [] } },
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/capture", "write-token");

    await user.type(await screen.findByLabelText("Source title"), "Hotel message");
    await user.selectOptions(screen.getByLabelText("Origin"), "external");
    await user.type(screen.getByLabelText("Content"), "The hotel moved check-in to 4 PM.");
    await user.click(screen.getByRole("button", { name: "Capture" }));

    expect(await screen.findByRole("heading", { name: "Capture receipt" })).toBeInTheDocument();
    expect(screen.getByText("The arrangement target is ambiguous.")).toBeInTheDocument();
    expect(screen.getByLabelText("Capture issues")).toHaveTextContent("consequential_ambiguity");
  });
});
