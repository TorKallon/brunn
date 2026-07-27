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
        },
      ],
    });

    await user.click(screen.getByRole("button", { name: "Read exact" }));
    expect(
      await screen.findByLabelText("Switzerland itinerary content"),
    ).toHaveTextContent("# Switzerland itinerary The transfer is confirmed.");
    await waitFor(() =>
      expect(readPayload).toMatchObject({
        requests: [{ ref: "entry:trip-entry", view: "full" }],
      }),
    );
  });
});
