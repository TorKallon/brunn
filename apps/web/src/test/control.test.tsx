import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

function controlRoutes(me = defaultMe) {
  return {
    "GET /api/v1/me": me,
    "GET /api/v1/credentials": { status: "complete", data: { items: [] } },
    "GET /api/v1/scopes": { status: "complete", data: { items: [] } },
    "GET /api/v1/policies": { status: "complete", data: { items: [] } },
    "GET /api/v1/audit": { status: "complete", data: { items: [] } },
    "GET /api/v1/usage": {
      status: "complete",
      data: {
        summary: {
          active_source_count: 3,
          used_source_count: 2,
          never_used_source_count: 1,
          source_uses: 8,
          record_exposures: 12,
          reference_occurrences: 16,
          tracked_response_count: 5,
          tracked_since: "2026-07-10T10:00:00Z",
          last_used_at: "2026-07-11T18:00:00Z",
        },
        most_used: [
          {
            id: "source:1",
            source_ref: "Projects/Straylight/Specification.md",
            title: "Straylight specification",
            kind: "file",
            scope_id: "scope:primary",
            captured_at: "2026-07-09T10:00:00Z",
            access_count: 6,
            record_exposure_count: 9,
            reference_occurrence_count: 11,
            session_count: 4,
            first_used_at: "2026-07-10T10:00:00Z",
            last_used_at: "2026-07-11T18:00:00Z",
            never_used: false,
            operation_counts: { open: 3, query: 2, read: 1, compute: 0, verify: 0 },
          },
        ],
        least_used: [
          {
            id: "source:2",
            source_ref: "Archive/unused.md",
            title: "Unused archive",
            kind: "file",
            scope_id: "scope:primary",
            captured_at: "2026-07-08T10:00:00Z",
            access_count: 0,
            record_exposure_count: 0,
            reference_occurrence_count: 0,
            session_count: 0,
            first_used_at: null,
            last_used_at: null,
            never_used: true,
            operation_counts: { open: 0, query: 0, read: 0, compute: 0, verify: 0 },
          },
        ],
        least_recently_used: [],
        by_operation: [
          {
            operation: "open",
            source_uses: 5,
            record_exposures: 8,
            reference_occurrences: 10,
            response_count: 3,
          },
        ],
      },
    },
  };
}

describe("credential management authority", () => {
  it("enables credential controls for an owner credential", async () => {
    installApiMock(controlRoutes());
    renderApp("/control", "owner-token");

    expect(await screen.findByRole("button", { name: "New credential" })).toBeEnabled();
  });

  it("does not present an ordinary writer as a credential manager", async () => {
    const me = structuredClone(defaultMe);
    me.data.capabilities = me.data.capabilities.filter(
      (capability) => capability !== "credential:manage",
    );
    installApiMock(controlRoutes(me));
    renderApp("/control", "writer-token");

    const button = await screen.findByRole("button", { name: "New credential" });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("title", "Requires an owner credential");
  });

  it("shows most-used, least-used, and never-used source telemetry", async () => {
    const user = userEvent.setup();
    installApiMock(controlRoutes());
    renderApp("/control", "owner-token");

    await user.click(await screen.findByRole("tab", { name: /Usage/ }));
    expect(screen.getByText("Straylight specification")).toBeInTheDocument();
    expect(screen.getByText("Active sources").parentElement).toHaveTextContent("3");

    await user.click(screen.getByRole("button", { name: "Least used" }));
    expect(screen.getByText("Unused archive")).toBeInTheDocument();
    expect(screen.getByText("Never")).toBeInTheDocument();
  });
});
