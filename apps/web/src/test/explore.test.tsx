import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { installApiMock, renderApp } from "./renderApp";

describe("explore search", () => {
  it("renders fused search results with selection evidence", async () => {
    installApiMock({
      "POST /api/v1/memory/query": {
        status: "complete",
        coverage: { searched: [{ lane: "lexical", complete: true }], unsearched: [], absence_safe: true },
        data: {
          results: [
            {
              id: "result_1",
              object_id: "object_alpine",
              source_id: "source_itinerary",
              title: "Alpine transfer reservation",
              excerpt: "The transfer departs Zurich at 09:40 on February 14.",
              kind: "core.arrangement",
              score: 0.934,
              why_selected: ["exact date match", "current source"],
              lane_scores: { lexical: 0.91, semantic: 0.87 },
            },
            {
              reference: "chunk:result_2",
              source_ref: "trip:itinerary",
              path: "Trips/Switzerland/Itinerary.md",
              heading: "(preamble)",
              content: "A native source chunk without legacy UI aliases.",
              score: 0.812,
              why_selected: ["lexical_recall"],
              lane_scores: { lexical: 0.81 },
            },
          ],
        },
      },
    });
    const user = userEvent.setup();
    renderApp("/explore", "search-token");

    const input = await screen.findByLabelText("Search authorized context");
    await user.type(input, "Zurich transfer");
    await user.click(screen.getByRole("button", { name: "Search" }));

    expect(await screen.findByRole("heading", { name: "Alpine transfer reservation" })).toBeInTheDocument();
    expect(screen.getByText("exact date match · current source")).toBeInTheDocument();
    expect(screen.getByText("0.934")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Object object_alp/i })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Trips/Switzerland/Itinerary.md" })).toBeInTheDocument();
    expect(screen.getByText("A native source chunk without legacy UI aliases.")).toBeInTheDocument();
  });
});
