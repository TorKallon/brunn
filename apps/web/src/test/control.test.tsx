import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

function controlRoutes(me = defaultMe) {
  return {
    "GET /api/v1/me": me,
    "GET /api/v1/credentials": { status: "complete", data: { items: [] } },
    "GET /api/v1/scopes": { status: "complete", data: { items: [] } },
    "GET /api/v1/policies": { status: "complete", data: { items: [] } },
    "GET /api/v1/audit": { status: "complete", data: { items: [] } },
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
});
