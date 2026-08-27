import axe from "axe-core";
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  briefingEditionFixture,
  briefingListFixture,
  briefingTopicsFixture,
} from "./briefingFixtures";
import { publishedDocumentFixture } from "./documentFixtures";
import { installApiMock, renderApp } from "./renderApp";
import {
  nextCandidates,
  taskDetail,
  taskProjectState,
} from "./taskFixtures";


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
  it("has no automated violations on the sign-in screen", async () => {
    const { container } = renderApp("/login");
    expect(
      await screen.findByRole("heading", { name: "Sign in" }),
    ).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on the password-recovery screen", async () => {
    const { container } = renderApp("/forgot-password");
    expect(
      await screen.findByRole("heading", { name: "Reset your password" }),
    ).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on the password-reset screen", async () => {
    window.history.replaceState({}, "", "/reset-password#token=test-token");
    const { container } = renderApp("/reset-password");
    expect(
      await screen.findByRole("heading", { name: "Choose a new password" }),
    ).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on the authenticated work surface", async () => {
    installApiMock();
    const { container } = renderApp("/work", "read-write-token");
    expect(await screen.findByRole("heading", { name: "Workspace" })).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on the landing dashboard", async () => {
    installApiMock();
    const { container } = renderApp("/dashboard", "read-write-token");
    expect(
      await screen.findByRole("navigation", { name: "Dashboard shortcuts" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("Urgent task 1")).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on settings", async () => {
    installApiMock();
    const { container } = renderApp("/settings", "read-write-token");
    expect(
      await screen.findByRole("heading", { name: "Settings" }),
    ).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Task operations" })).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on the deliberate task list", async () => {
    installApiMock();
    const { container } = renderApp("/tasks", "read-write-token");
    expect(await screen.findByText("Next task 1")).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on task detail", async () => {
    installApiMock({
      [`GET /api/v1/workspace/tasks/${nextCandidates[0].task_ref}`]: {
        status: "complete",
        data: taskDetail,
      },
    });
    const { container } = renderApp(
      `/tasks/${nextCandidates[0].task_ref}`,
      "read-write-token",
    );
    expect(
      await screen.findByRole("heading", { name: nextCandidates[0].title }),
    ).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on project state", async () => {
    installApiMock({
      "GET /api/v1/workspace/projects/straylight/state": {
        status: "complete",
        data: taskProjectState,
      },
    });
    const { container } = renderApp("/projects/straylight", "read-write-token");
    expect(
      await screen.findByRole("heading", { name: "Straylight" }),
    ).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on the briefings index", async () => {
    installApiMock({
      "GET /api/v1/workspace/briefings": briefingListFixture,
    });
    const { container } = renderApp("/briefings", "read-write-token");
    expect(
      await screen.findByRole("link", { name: /Morning briefing - 2026-08-01/ }),
    ).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on an expanded briefing edition", async () => {
    installApiMock({
      "GET /api/v1/workspace/briefings": briefingListFixture,
      "GET /api/v1/workspace/briefings/2026-08-01/morning":
        briefingEditionFixture,
    });
    const user = userEvent.setup();
    const { container } = renderApp(
      "/briefings/2026-08-01?edition=morning",
      "read-write-token",
    );
    await user.click(
      await screen.findByRole("button", { name: /OpenAI ships o5/ }),
    );
    await user.click(screen.getByRole("button", { name: "2 more" }));
    expect(
      screen.getByRole("region", { name: "Frontier labs item detail" }),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Feedback" }));
    expect(
      screen.getByRole("button", { name: "Follow closer" }),
    ).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on a published document", async () => {
    installApiMock({
      "GET /api/v1/workspace/documents/switzerland-itinerary":
        publishedDocumentFixture,
    });
    const { container } = renderApp(
      "/documents/switzerland-itinerary",
      "read-write-token",
    );
    expect(
      await screen.findByRole("heading", {
        level: 1,
        name: "Switzerland summer itinerary",
      }),
    ).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });

  it("has no automated violations on the topics page with the editor open", async () => {
    installApiMock({
      "GET /api/v1/workspace/briefings/topics": briefingTopicsFixture,
    });
    const user = userEvent.setup();
    const { container } = renderApp("/topics", "read-write-token");
    await user.click(
      await screen.findByRole("row", { name: "Edit topic Stock watchlist" }),
    );
    expect(
      screen.getByRole("textbox", { name: "Name" }),
    ).toBeInTheDocument();
    await expectNoAutomatedViolations(container);
  });
});
