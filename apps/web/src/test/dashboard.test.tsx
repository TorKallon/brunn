import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { defaultDashboard, installApiMock, renderApp } from "./renderApp";

const todayBriefing = {
  status: "complete",
  data: {
    editions: [
      {
        date: "2026-08-02",
        edition: "morning",
        path: "Briefings/2026-08-02/morning.md",
        entry_ref: "entry:briefing-today",
        version: 2,
        generated_at: "2026-08-02T14:00:00Z",
        summary_md: ["The **morning briefing** is ready."],
        section_titles: ["Projects", "Outside world"],
        item_count: 8,
      },
    ],
    limit: 7,
    truncated: false,
    next: null,
    workspace_generation: 12,
  },
};

describe("landing dashboard", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("becomes the authenticated root and links directly to today's briefing", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-08-02T18:00:00Z"));
    installApiMock({
      "GET /api/v1/workspace/briefings": todayBriefing,
    });
    renderApp("/");

    const shortcuts = await screen.findByRole("navigation", {
      name: "Dashboard shortcuts",
    });
    expect(screen.getByRole("heading", { name: "Dashboard" })).toBeInTheDocument();
    const todayLink = await within(shortcuts).findByRole("link", {
      name: /Read today’s briefing/,
    });
    expect(todayLink).toHaveAttribute(
      "href",
      "/briefings/2026-08-02?edition=morning",
    );
    expect(within(shortcuts).getByRole("link", { name: "All briefings" })).toHaveAttribute(
      "href",
      "/briefings",
    );
    expect(within(shortcuts).getByRole("link", { name: "Search memory" })).toHaveAttribute(
      "href",
      "/explore",
    );
    expect(screen.queryByText(/A quiet view of what your memory is holding/)).not.toBeInTheDocument();
  });

  it("shows separate storage, activity, charts, and access-client state", async () => {
    installApiMock();
    renderApp("/dashboard");

    expect(await screen.findByText("brunn")).toBeInTheDocument();
    expect(
      await screen.findByRole("heading", { name: "What Brunn is holding" }),
    ).toBeInTheDocument();

    const textCard = screen.getByText("Text artifacts").closest("article");
    expect(textCard).not.toBeNull();
    expect(within(textCard!).getByText("128")).toBeInTheDocument();
    expect(within(textCard!).getByText("2.5 MB")).toBeInTheDocument();

    const binaryCard = screen.getByText("S3 object versions").closest("article");
    expect(binaryCard).not.toBeNull();
    expect(within(binaryCard!).getByText("14")).toBeInTheDocument();
    expect(within(binaryCard!).getByText("18 MB")).toBeInTheDocument();

    const reads = screen.getByText("Reads today").closest("article");
    const writes = screen.getByText("Writes today").closest("article");
    expect(within(reads!).getByText("37")).toBeInTheDocument();
    expect(within(writes!).getByText("6")).toBeInTheDocument();

    expect(
      screen.getByRole("table", { name: "Operations over the last 7 days" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("table", { name: "Data moved over the last 7 days" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Brunn Web")).toBeInTheDocument();
    expect(screen.getByText("iPhone")).toBeInTheDocument();
    expect(screen.getByText("This client")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Connected clients" }),
    ).toBeInTheDocument();
    expect(screen.getAllByText("scope:root")).toHaveLength(2);
  });

  it("requests local-time aggregation and keeps metrics visible if briefings fail", async () => {
    let dashboardRequest: Request | undefined;
    installApiMock({
      "GET /api/v1/workspace/dashboard": (request: Request) => {
        dashboardRequest = request;
        return defaultDashboard;
      },
      "GET /api/v1/workspace/briefings": {
        status: 503,
        body: {
          error: { code: "unavailable", message: "briefing index unavailable" },
        },
      },
    });
    renderApp("/dashboard");

    expect(await screen.findByRole("link", { name: "Open briefings" })).toHaveAttribute(
      "href",
      "/briefings",
    );
    expect(await screen.findByText("Text artifacts")).toBeInTheDocument();
    await waitFor(() => expect(dashboardRequest).toBeDefined());
    expect(new URL(dashboardRequest!.url).searchParams.get("timezone")).toBeTruthy();
  });

  it("does not present unavailable tracking or object inventory as trustworthy zeroes", async () => {
    installApiMock({
      "GET /api/v1/workspace/dashboard": {
        ...defaultDashboard,
        data: {
          ...defaultDashboard.data,
          tracking: {
            status: "disabled",
            dropped_events: 0,
            flush_failures: 0,
          },
          storage: {
            ...defaultDashboard.data.storage,
            binary: {
              count: null,
              size_bytes: null,
              semantics: "physical_object_versions",
              status: "unavailable",
              observed_at: null,
            },
          },
        },
      },
    });
    renderApp("/dashboard");

    expect(
      await screen.findByText(
        "Usage tracking is unavailable; today’s zeroes are not authoritative.",
      ),
    ).toBeInTheDocument();
    expect(screen.getAllByText("Unavailable").length).toBeGreaterThanOrEqual(4);
    expect(screen.getByText("Physical inventory unavailable")).toBeInTheDocument();
  });
});
