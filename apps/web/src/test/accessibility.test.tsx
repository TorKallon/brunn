import axe from "axe-core";
import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { installApiMock, renderApp } from "./renderApp";


async function expectNoAutomatedViolations(container: HTMLElement) {
  const result = await axe.run(container, {
    rules: {
      // JSDOM cannot calculate rendered foreground/background colors.
      "color-contrast": { enabled: false },
    },
  });
  expect(
    result.violations.map((violation) => ({
      id: violation.id,
      impact: violation.impact,
      targets: violation.nodes.map((node) => node.target),
    })),
  ).toEqual([]);
}


describe("accessibility contracts", () => {
  it("has no automated violations on the token entry screen", async () => {
    const { container } = renderApp("/work");
    expect(
      await screen.findByRole("heading", { name: "Straylight" }),
    ).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on the authenticated work surface", async () => {
    installApiMock();
    const { container } = renderApp("/work", "read-write-token");
    expect(await screen.findByRole("heading", { name: "Work" })).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });
});
