import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { APPEARANCE_STORAGE_KEY } from "../lib/appearance";
import { installApiMock, renderApp } from "./renderApp";

describe("human-facing navigation and settings", () => {
  it("keeps the primary navigation focused and opens settings from the user menu", async () => {
    installApiMock();
    const user = userEvent.setup();
    renderApp("/dashboard");

    const navigation = await screen.findByRole("navigation", {
      name: "Primary navigation",
    });
    const links = within(navigation);

    expect(links.getByRole("link", { name: "Overview" })).toBeInTheDocument();
    expect(links.getByRole("link", { name: "Alerts" })).toBeInTheDocument();
    expect(links.getByRole("link", { name: "Briefings" })).toBeInTheDocument();
    expect(links.getByRole("link", { name: "Search" })).toBeInTheDocument();
    expect(
      links.getByRole("link", { name: "Detailed Activity" }),
    ).toBeInTheDocument();
    for (const hidden of ["Topics", "Workspace", "Write", "Binaries", "Background"]) {
      expect(links.queryByRole("link", { name: hidden })).not.toBeInTheDocument();
    }

    await user.click(
      screen.getByRole("button", { name: "User menu for Aether" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "Settings" }));
    expect(
      await screen.findByRole("heading", { name: "Settings" }),
    ).toBeInTheDocument();
  });

  it("saves and restores an explicit light or dark appearance", async () => {
    installApiMock();
    const user = userEvent.setup();
    const first = renderApp("/settings");

    expect(
      await screen.findByRole("heading", { name: "Settings" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /Dark/ })).toBeChecked();

    await user.click(screen.getByRole("radio", { name: /Light/ }));
    expect(document.documentElement).toHaveAttribute("data-theme", "light");
    expect(window.localStorage.getItem(APPEARANCE_STORAGE_KEY)).toBe("light");

    first.unmount();
    renderApp("/settings");
    expect(await screen.findByRole("radio", { name: /Light/ })).toBeChecked();

    await user.click(screen.getByRole("radio", { name: /Dark/ }));
    expect(document.documentElement).not.toHaveAttribute("data-theme");
    expect(window.localStorage.getItem(APPEARANCE_STORAGE_KEY)).toBe("dark");
  });
});
