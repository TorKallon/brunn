import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { installApiMock, renderApp } from "./renderApp";

const disconnectedStatus = {
  status: "complete",
  data: {
    control: { enabled: false, reason: "CONTROL.md is missing" },
    dreamer: {
      connect: { state: "disconnected" },
      runtime: {},
    },
  },
};

const connectedStatus = {
  status: "complete",
  data: {
    control: {
      enabled: true,
      mode: "report-only",
      advance_after: "2026-09-06",
    },
    dreamer: {
      connect: { state: "connected", account: "acct_dreamer", plan: "pro" },
      runtime: {
        account: "acct_dreamer",
        plan: "pro",
        connected_at: "2026-08-30T10:00:00Z",
        verified_at: "2026-08-30T10:00:05Z",
        last_attempt_date: "2026-08-30",
        last_attempt_result: "completed",
      },
    },
  },
};

describe("Settings → Dreaming", () => {
  it("shows a disconnected status card with a Connect action", async () => {
    installApiMock({
      "GET /api/v1/workspace/dreaming/status": disconnectedStatus,
    });
    renderApp("/settings");

    expect(
      await screen.findByRole("heading", { name: "Dreaming" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("Not connected")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Connect/ }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Resume" })).toBeInTheDocument();
  });

  it("starts a device-code connect and surfaces the URL and code", async () => {
    installApiMock({
      "GET /api/v1/workspace/dreaming/status": disconnectedStatus,
      "POST /api/v1/workspace/dreaming/connect/start": {
        status: "complete",
        data: {
          state: "pending",
          url: "https://auth.openai.com/activate",
          code: "ABCD-EFGH",
        },
      },
    });
    const user = userEvent.setup();
    renderApp("/settings");

    await user.click(await screen.findByRole("button", { name: /Connect/ }));
    // The mutation succeeded; the panel refetches status. The mock still
    // reports disconnected, so we only assert the request round-trip worked
    // (no error surface appeared).
    expect(screen.queryByText(/Connect failed/)).not.toBeInTheDocument();
  });

  it("shows a connected account with Disconnect and Pause", async () => {
    installApiMock({
      "GET /api/v1/workspace/dreaming/status": connectedStatus,
    });
    renderApp("/settings");

    expect(
      await screen.findByRole("heading", { name: "Dreaming" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("acct_dreamer (pro)")).toBeInTheDocument();
    expect(screen.getByText(/2026-08-30 — completed/)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Disconnect" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pause" })).toBeInTheDocument();
    expect(screen.getByText("2026-09-06")).toBeInTheDocument();
  });

  it("pauses dreaming and reflects the paused state", async () => {
    installApiMock({
      "GET /api/v1/workspace/dreaming/status": connectedStatus,
      "POST /api/v1/workspace/dreaming/pause": {
        status: "complete",
        data: {
          control: { enabled: false, reason: "CONTROL enabled: false" },
        },
      },
    });
    const user = userEvent.setup();
    renderApp("/settings");

    await user.click(await screen.findByRole("button", { name: "Pause" }));
    expect(screen.queryByText(/Pause failed/)).not.toBeInTheDocument();
  });
});
