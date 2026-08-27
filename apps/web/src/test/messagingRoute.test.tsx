import axe from "axe-core";
import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

const now = "2026-08-27T09:00:00Z";

function messagingStatus(enabled: boolean) {
  return {
    status: "complete",
    data: {
      status: "healthy",
      version: "test",
      corpus_revision: "rev_001",
      feature_flags: { messaging_enabled: enabled },
      dependencies: {},
      indexes: {},
    },
  };
}

function messagingMe() {
  const me = structuredClone(defaultMe);
  me.data.capabilities.push("message.read", "message.write");
  return me;
}

const emptySync = {
  status: "complete",
  data: {
    status: "complete",
    cursor: 0,
    resume_cursor: null,
    has_more: false,
    messages: [],
    conversations: [],
    presence: [],
    unread: {},
    as_of: now,
  },
};

describe("messaging runtime route", () => {
  it("leaves the established navigation and route absent when the gate is off", async () => {
    installApiMock({
      "GET /api/v1/status": messagingStatus(false),
    });
    const dashboard = renderApp("/dashboard");
    const navigation = await screen.findByRole("navigation", {
      name: "Primary navigation",
    });
    expect(
      within(navigation).queryByRole("link", { name: "Agents" }),
    ).not.toBeInTheDocument();
    expect(
      within(navigation)
        .getAllByRole("link")
        .map((link) => link.textContent),
    ).toEqual([
      "Overview",
      "Alerts",
      "Briefings",
      "Search",
      "Detailed Activity",
    ]);
    dashboard.unmount();

    renderApp("/agents");
    expect(
      await screen.findByText("Route not found", { exact: true }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("navigation", { name: "Primary navigation" }),
    ).not.toBeInTheDocument();
  });

  it("exposes one accessible Agents destination when the gate is on", async () => {
    installApiMock({
      "GET /api/v1/status": messagingStatus(true),
      "GET /api/v1/me": messagingMe(),
      "GET /api/v1/workspace/messaging/sync": emptySync,
      "GET /api/v1/workspace/messaging/agents": {
        status: "complete",
        data: { agents: [], as_of: now },
      },
      "GET /api/v1/credentials": {
        status: "complete",
        data: { items: [] },
      },
    });
    const { container } = renderApp("/agents");

    expect(
      await screen.findByRole("heading", { name: "Agents" }),
    ).toBeInTheDocument();
    const navigation = screen.getByRole("navigation", {
      name: "Primary navigation",
    });
    expect(
      within(navigation).getByRole("link", { name: "Agents" }),
    ).toHaveAttribute("href", "/agents");
    const accessibility = await axe.run(container, {
      rules: { "color-contrast": { enabled: false } },
    });
    expect(accessibility.violations).toEqual([]);
  });
});
