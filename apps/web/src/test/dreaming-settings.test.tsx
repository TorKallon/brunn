import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
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

  it("immediately replaces Connect with a focused authorization step and copyable code", async () => {
    installApiMock({
      "GET /api/v1/workspace/dreaming/status": disconnectedStatus,
      "POST /api/v1/workspace/dreaming/connect/start": {
        status: "complete",
        data: {
          state: "pending",
          url: "https://auth.openai.com/codex/device",
          code: "ABCD-EFGH",
        },
      },
    });
    const user = userEvent.setup();
    renderApp("/settings");

    await user.click(await screen.findByRole("button", { name: /Connect/ }));
    const heading = await screen.findByRole("heading", { name: "Finish connecting Dreamer" });
    await waitFor(() => expect(heading).toHaveFocus());
    expect(screen.getByText("ABCD-EFGH")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Authorize in ChatGPT" })).toHaveAttribute("href", "https://auth.openai.com/codex/device");
    expect(screen.getByRole("link", { name: "Authorize in ChatGPT" })).toHaveAttribute("target", "_blank");
    expect(screen.queryByRole("button", { name: "Connect" })).not.toBeInTheDocument();
    const copy = vi.spyOn(navigator.clipboard, "writeText");
    await user.click(screen.getByRole("button", { name: "Copy code" }));
    expect(copy).toHaveBeenCalledWith("ABCD-EFGH");
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    expect(screen.queryByText(/Connect failed/)).not.toBeInTheDocument();
  });

  it("keeps the code usable when clipboard access is denied", async () => {
    installApiMock({
      "GET /api/v1/workspace/dreaming/status": {
        ...disconnectedStatus,
        data: { ...disconnectedStatus.data, dreamer: { connect: { state: "pending", url: "https://auth.openai.com/codex/device", code: "ABCD-EFGH" }, runtime: {} } },
      },
    });
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("Clipboard denied"));
    renderApp("/settings");
    await user.click(await screen.findByRole("button", { name: "Copy code" }));
    expect(screen.getByText(/Select and copy the code above/)).toBeInTheDocument();
    expect(screen.getByText("ABCD-EFGH")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Authorize in ChatGPT" })).toBeInTheDocument();
  });

  it("shows preparation while the sign-in request is in flight", async () => {
    let resolve!: (value: unknown) => void;
    const response = new Promise((done) => { resolve = done; });
    installApiMock({
      "GET /api/v1/workspace/dreaming/status": disconnectedStatus,
      "POST /api/v1/workspace/dreaming/connect/start": () => response,
    });
    const user = userEvent.setup();
    renderApp("/settings");
    await user.click(await screen.findByRole("button", { name: "Connect" }));
    expect(await screen.findByText("Preparing your sign-in link…")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Connect" })).not.toBeInTheDocument();
    resolve({ status: "complete", data: { state: "pending", url: "https://auth.openai.com/codex/device", code: "ABCD-EFGH" } });
    expect(await screen.findByRole("link", { name: "Authorize in ChatGPT" })).toBeInTheDocument();
  });

  it("finishes a verifying login and replaces the instructions with the connected account", async () => {
    let finished = false;
    installApiMock({
      "GET /api/v1/workspace/dreaming/status": () => finished ? connectedStatus : {
        ...disconnectedStatus,
        data: { ...disconnectedStatus.data, dreamer: { connect: { state: "verifying" }, runtime: {} } },
      },
      "GET /api/v1/workspace/dreaming/connect/wait": () => {
        finished = true;
        return { status: "complete", data: connectedStatus.data.dreamer.connect };
      },
    });
    renderApp("/settings");
    expect(await screen.findByText("Checking your connection…")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Connect" })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("acct_dreamer (pro)")).toBeInTheDocument(), { timeout: 5000 });
    expect(screen.queryByText("Checking your connection…")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Disconnect" })).toBeInTheDocument();
  }, 7000);

  it("reports a connection-check failure and recovers without starting another login", async () => {
    let checks = 0;
    installApiMock({
      "GET /api/v1/workspace/dreaming/status": () => checks > 1 ? connectedStatus : {
        ...disconnectedStatus,
        data: { ...disconnectedStatus.data, dreamer: { connect: { state: "pending", url: "https://auth.openai.com/codex/device", code: "ABCD-EFGH" }, runtime: {} } },
      },
      "GET /api/v1/workspace/dreaming/connect/wait": () => {
        checks++;
        return checks === 1 ? { status: 503, body: { error: { code: "unavailable", message: "Temporary failure" } } } : { status: "complete", data: connectedStatus.data.dreamer.connect };
      },
    });
    renderApp("/settings");
    await waitFor(() => expect(screen.getByText(/We couldn’t check your connection/)).toBeInTheDocument(), { timeout: 5000 });
    expect(screen.getByRole("link", { name: "Authorize in ChatGPT" })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("acct_dreamer (pro)")).toBeInTheDocument(), { timeout: 5000 });
    expect(screen.queryByText(/We couldn’t check your connection/)).not.toBeInTheDocument();
    expect(checks).toBe(2);
  }, 10000);

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
    expect(screen.getByRole("link", { name: "Open Review" })).toHaveAttribute("href", "/dreams");
    expect(screen.queryByText("Full mode after")).not.toBeInTheDocument();
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
