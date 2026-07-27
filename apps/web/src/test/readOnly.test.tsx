import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

function readOnlyMe() {
  const me = structuredClone(defaultMe);
  me.data.read_only = true;
  me.data.capabilities = ["open", "query", "read", "status"];
  me.data.active_scope.access = "read_only";
  return me;
}

describe("read-only capabilities", () => {
  it("allows context open while keeping checkpoint mutation disabled", async () => {
    const user = userEvent.setup();
    installApiMock({ "GET /api/v1/me": readOnlyMe() });
    renderApp("/work", "readonly-token");

    const open = await screen.findByRole("button", { name: "Open" });
    expect(open).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "Goal" }), "Inspect current plans");
    expect(open).toBeEnabled();
    await user.click(open);

    expect(
      await screen.findByRole("button", { name: "Save checkpoint" }),
    ).toBeDisabled();
  });

  it("disables capture, Markdown, and binary writes", async () => {
    const user = userEvent.setup();
    installApiMock({ "GET /api/v1/me": readOnlyMe() });
    renderApp("/capture", "readonly-token");

    expect(
      (await screen.findAllByText("Read-only credential active")).length,
    ).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "Capture" })).toBeDisabled();

    await user.click(screen.getByRole("tab", { name: "Markdown" }));
    expect(screen.getByRole("button", { name: "Write" })).toBeDisabled();

    await user.click(screen.getByRole("tab", { name: "Binary" }));
    expect(screen.getByRole("button", { name: "Upload" })).toBeDisabled();
  });
});
