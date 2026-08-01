import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  briefingEditionFixture,
  briefingListFixture,
  legacyEditionFixture,
} from "./briefingFixtures";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

function readOnlyMe() {
  const me = structuredClone(defaultMe);
  me.data.read_only = true;
  me.data.capabilities = ["open", "query", "read", "status"];
  me.data.active_scope.access = "read_only";
  return me;
}

describe("briefings daily thread", () => {
  it("lists briefing editions with summaries and section chips", async () => {
    installApiMock({
      "GET /api/v1/workspace/briefings": briefingListFixture,
    });
    renderApp("/briefings", "read-token");

    expect(
      await screen.findByRole("heading", { name: "Briefings" }),
    ).toBeInTheDocument();
    const card = await screen.findByRole("link", {
      name: /Morning briefing - 2026-08-01/,
    });
    expect(card).toBeInTheDocument();
    expect(within(card).getByText(/eval harness/)).toBeInTheDocument();
    expect(within(card).getByText("Portfolio")).toBeInTheDocument();
    expect(within(card).getByText("6 items")).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: /Morning briefing - 2026-07-31/ }),
    ).toBeInTheDocument();
  });

  it("renders the edition header, summary disclosure, and section groups", async () => {
    installApiMock({
      "GET /api/v1/workspace/briefings": briefingListFixture,
      "GET /api/v1/workspace/briefings/2026-08-01/morning":
        briefingEditionFixture,
    });
    const user = userEvent.setup();
    renderApp("/briefings/2026-08-01?edition=morning", "read-token");

    expect(
      await screen.findByRole("heading", {
        name: "Morning briefing - 2026-08-01",
      }),
    ).toBeInTheDocument();
    expect(
      await screen.findByText(/^Generated .+ · Updated .+$/),
    ).toBeInTheDocument();

    expect(screen.getByText(/Sleep score 82/)).toBeInTheDocument();
    expect(
      screen.queryByText(/Discord digest/),
    ).not.toBeInTheDocument();
    const disclosure = screen.getByRole("button", { name: "2 more" });
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    await user.click(disclosure);
    expect(screen.getByText(/Discord digest/)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Show less" }),
    ).toHaveAttribute("aria-expanded", "true");

    expect(
      screen.getByRole("heading", { name: "Frontier labs" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Portfolio" }),
    ).toBeInTheDocument();
    expect(screen.getByText("New")).toBeInTheDocument();
    expect(screen.getByText("Update")).toBeInTheDocument();
    expect(screen.getByText("Seen")).toBeInTheDocument();
  });

  it("expands an item row in place to show detail, sources, and times", async () => {
    installApiMock({
      "GET /api/v1/workspace/briefings": briefingListFixture,
      "GET /api/v1/workspace/briefings/2026-08-01/morning":
        briefingEditionFixture,
    });
    const user = userEvent.setup();
    renderApp("/briefings/2026-08-01?edition=morning", "read-token");

    const row = await screen.findByRole("button", {
      name: /OpenAI ships o5/,
    });
    expect(row).toHaveAttribute("aria-expanded", "false");
    expect(
      screen.queryByText(/state-of-the-art results/),
    ).not.toBeInTheDocument();

    await user.click(row);
    expect(row).toHaveAttribute("aria-expanded", "true");
    const detail = screen.getByRole("region", {
      name: "Frontier labs item detail",
    });
    expect(
      within(detail).getByText(/state-of-the-art results/),
    ).toBeInTheDocument();
    expect(within(detail).getByText("What changed:")).toBeInTheDocument();
    expect(
      within(detail).getByText("First release since o4-mini."),
    ).toBeInTheDocument();
    const source = within(detail).getByRole("link", { name: "openai.com" });
    expect(source).toHaveAttribute("href", "https://openai.com/blog/o5");
    expect(within(detail).getByText(/Published .+ · First seen .+/)).toBeInTheDocument();

    await user.click(row);
    expect(row).toHaveAttribute("aria-expanded", "false");
    expect(
      screen.queryByText(/state-of-the-art results/),
    ).not.toBeInTheDocument();
  });

  it("sends item actions with the expected payloads", async () => {
    const actionPayloads: unknown[] = [];
    installApiMock({
      "GET /api/v1/workspace/briefings": briefingListFixture,
      "GET /api/v1/workspace/briefings/2026-08-01/morning":
        briefingEditionFixture,
      "POST /api/v1/workspace/briefings/items/action": async (
        request: Request,
      ) => {
        actionPayloads.push(await request.json());
        return {
          status: "committed",
          data: {
            action: "read",
            path: "Briefings/2026/Morning briefing - 2026-08-01.md",
            entry_ref: "entry:aug1",
            version: 3,
            content_hash: "sha256:action",
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/briefings/2026-08-01?edition=morning", "read-token");

    await user.click(
      await screen.findByRole("button", { name: /OpenAI ships o5/ }),
    );
    await user.click(screen.getByRole("button", { name: "Mark read" }));
    await user.click(screen.getByRole("button", { name: "Go deeper" }));
    await user.click(screen.getByRole("button", { name: "Useful" }));
    await user.click(screen.getByRole("button", { name: "Repeated" }));
    await user.click(screen.getByRole("button", { name: "Mute topic" }));

    expect(actionPayloads).toEqual([
      { action: "read", edition_ref: "entry:aug1", item_id: "openai-o5" },
      { action: "expand", edition_ref: "entry:aug1", item_id: "openai-o5" },
      {
        action: "feedback",
        edition_ref: "entry:aug1",
        item_id: "openai-o5",
        verdict: "useful",
      },
      {
        action: "feedback",
        edition_ref: "entry:aug1",
        item_id: "openai-o5",
        verdict: "repeated",
      },
      { action: "mute_topic", topic_slug: "frontier-labs" },
    ]);
  });

  it("disables item actions for read-only credentials", async () => {
    installApiMock({
      "GET /api/v1/me": readOnlyMe(),
      "GET /api/v1/workspace/briefings": briefingListFixture,
      "GET /api/v1/workspace/briefings/2026-08-01/morning":
        briefingEditionFixture,
    });
    const user = userEvent.setup();
    renderApp("/briefings/2026-08-01?edition=morning", "readonly-token");

    expect(
      (await screen.findAllByText("Read-only credential active")).length,
    ).toBeGreaterThan(0);
    await user.click(
      await screen.findByRole("button", { name: /OpenAI ships o5/ }),
    );
    for (const label of [
      "Mark read",
      "Go deeper",
      "Useful",
      "Repeated",
      "Mute topic",
    ]) {
      expect(screen.getByRole("button", { name: label })).toBeDisabled();
    }
  });

  it("falls back to raw markdown when the edition has no structured payload", async () => {
    installApiMock({
      "GET /api/v1/workspace/briefings": briefingListFixture,
      "GET /api/v1/workspace/briefings/2026-07-31/morning":
        legacyEditionFixture,
    });
    renderApp("/briefings/2026-07-31?edition=morning", "read-token");

    expect(
      await screen.findByRole("heading", {
        name: "Morning briefing - 2026-07-31",
      }),
    ).toBeInTheDocument();
    expect(
      await screen.findByText("Structured Payload Unavailable"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Legacy body without a structured payload."),
    ).toBeInTheDocument();
  });

  it("shows an error state when the date has no edition", async () => {
    installApiMock({
      "GET /api/v1/workspace/briefings": briefingListFixture,
      "GET /api/v1/workspace/briefings/2026-08-02/morning": {
        status: 404,
        body: {
          error: {
            code: "briefing_not_found",
            message: "no briefing edition for 2026-08-02/morning",
          },
        },
      },
    });
    renderApp("/briefings/2026-08-02?edition=morning", "read-token");

    expect(
      await screen.findByText("no briefing edition for 2026-08-02/morning"),
    ).toBeInTheDocument();
    expect(screen.getByText("briefing_not_found")).toBeInTheDocument();
  });
});
