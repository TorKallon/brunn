import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

describe("read-only capabilities", () => {
  it("disables all capture mutations for a read-only credential", async () => {
    const me = structuredClone(defaultMe);
    me.data.read_only = true;
    me.data.capabilities = ["read", "memory.query", "memory.verify"];
    me.data.active_scope.access = "read_only";
    installApiMock({ "GET /api/v1/me": me });
    const user = userEvent.setup();
    renderApp("/capture", "readonly-token");

    expect((await screen.findAllByText("Read-only credential active")).length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "Capture" })).toBeDisabled();

    await user.click(screen.getByRole("tab", { name: "Canonical save" }));
    expect(screen.getByRole("button", { name: "Save change" })).toBeDisabled();

    await user.click(screen.getByRole("tab", { name: "Stage and import" }));
    expect(screen.getByRole("button", { name: "Stage" })).toBeDisabled();
  });
});
