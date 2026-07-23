import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { installApiMock, renderApp } from "./renderApp";

describe("dream audit", () => {
  it("exposes candidate lineage, gates, and the durable audit trail", async () => {
    installApiMock({
      "GET /api/v1/dreams": {
        status: "complete",
        data: { items: [{ id: "dream_1", region: "Travel coordination", status: "awaiting_review", base_revision: "rev_1", candidate_revision: "rev_2", updated_at: "2026-07-11T18:00:00Z" }] },
      },
      "GET /api/v1/dreams/dream_1": {
        status: "complete",
        data: {
          id: "dream_1",
          region: "Travel coordination",
          status: "awaiting_review",
          base_revision: "rev_1",
          candidate_revision: "rev_2",
          diff: [{ id: "change_1", kind: "derived_view", target: "Swiss itinerary", disposition: "add", after: { summary: "Current itinerary" } }],
          gates: [{ id: "gate_1", name: "Protected fact preservation", status: "passed", summary: "All protected facts remain retrievable." }],
          lineage: [{ source_id: "source_1", source_title: "Confirmed itinerary", locator: "lines 4-9", hash: "sha256:abc" }],
          evaluation: { baseline_score: 0.94, candidate_score: 0.96, regressions: [] },
          audit: [{ id: "audit_1", action: "candidate.created", actor: "dream-worker", status: "complete", created_at: "2026-07-11T17:58:00Z", details: { revision: "rev_2" } }],
        },
      },
    });
    const user = userEvent.setup();
    renderApp("/dreams", "dream-token");

    await screen.findByRole("tab", { name: /Candidate diff/ });
    await user.click(screen.getByRole("tab", { name: /Gates/ }));
    expect(screen.getByText("Protected fact preservation")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: /Audit/ }));
    expect(screen.getByRole("heading", { name: "Dream audit trail" })).toBeInTheDocument();
    expect(screen.getByText("candidate.created")).toBeInTheDocument();
    expect(screen.getByText("dream-worker")).toBeInTheDocument();
    expect(screen.getByLabelText("candidate.created details")).toHaveTextContent("rev_2");
  });
});
