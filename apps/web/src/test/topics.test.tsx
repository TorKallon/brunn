import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { briefingTopicsFixture } from "./briefingFixtures";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

const STOCKS_DOCUMENT =
  "---\n" +
  "kind: briefing_topic\n" +
  "slug: stocks\n" +
  "name: Stock watchlist\n" +
  "section_order: 70\n" +
  "mode: on_material_delta\n" +
  "editions: [morning, health-update]\n" +
  "schedule: 10:00 America/Los_Angeles\n" +
  "entities: [Alphabet, Microsoft]\n" +
  "symbols: [GOOGL, MSFT]\n" +
  "suppress_unchanged: false\n" +
  "freshness_hours: 24\n" +
  "---\n" +
  "\n" +
  "Absolute dollar changes, not percentages.\n";

function writeReceipt(path: string, version: number) {
  return {
    status: "committed",
    data: {
      path,
      entry_ref: "entry:topic-written",
      version,
      content_hash: "sha256:topic",
      workspace_generation: 42,
      no_op: false,
    },
  };
}

function readOnlyMe() {
  const me = structuredClone(defaultMe);
  me.data.read_only = true;
  me.data.capabilities = ["open", "query", "read", "status"];
  me.data.active_scope.access = "read_only";
  return me;
}

describe("briefing topics management", () => {
  it("renders the topics table with mode badges and a parse-error flag", async () => {
    installApiMock({
      "GET /api/v1/workspace/briefings/topics": briefingTopicsFixture,
    });
    renderApp("/topics", "owner-token");

    const row = await screen.findByRole("row", {
      name: "Edit topic Stock watchlist",
    });
    expect(within(row).getByText("On Material Delta")).toBeInTheDocument();
    expect(within(row).getByText("70")).toBeInTheDocument();
    expect(
      within(row).getByText("morning, health-update"),
    ).toBeInTheDocument();
    expect(within(row).getByText("v5")).toBeInTheDocument();
    expect(screen.getAllByText("Every Briefing").length).toBeGreaterThan(0);
    const broken = screen.getByRole("row", { name: "Edit topic broken" });
    expect(within(broken).getByText("Parse error")).toBeInTheDocument();
  });

  it("serializes frontmatter and body into the workspace write", async () => {
    const writePayloads: unknown[] = [];
    installApiMock({
      "GET /api/v1/workspace/briefings/topics": briefingTopicsFixture,
      "POST /api/v1/workspace/write": async (request: Request) => {
        writePayloads.push(await request.json());
        return writeReceipt("Briefings/Topics/stocks.md", 6);
      },
    });
    const user = userEvent.setup();
    renderApp("/topics", "owner-token");

    await user.click(
      await screen.findByRole("row", { name: "Edit topic Stock watchlist" }),
    );
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue(
      "Stock watchlist",
    );
    expect(screen.getByRole("textbox", { name: "Slug" })).toHaveAttribute(
      "readonly",
    );
    await user.click(screen.getByRole("button", { name: "Save topic" }));

    expect(writePayloads).toEqual([
      {
        path: "Briefings/Topics/stocks.md",
        content: STOCKS_DOCUMENT,
        expected_version: 5,
      },
    ]);
  });

  it("creates a new topic under Briefings/Topics with expected version 0", async () => {
    const writePayloads: unknown[] = [];
    installApiMock({
      "GET /api/v1/workspace/briefings/topics": briefingTopicsFixture,
      "POST /api/v1/workspace/write": async (request: Request) => {
        writePayloads.push(await request.json());
        return writeReceipt("Briefings/Topics/chips.md", 1);
      },
    });
    const user = userEvent.setup();
    renderApp("/topics", "owner-token");

    await user.click(
      await screen.findByRole("button", { name: "New topic" }),
    );
    await user.type(screen.getByRole("textbox", { name: "Slug" }), "chips");
    await user.type(
      screen.getByRole("textbox", { name: "Name" }),
      "Chip supply",
    );
    await user.type(
      screen.getByRole("textbox", { name: /Instructions/ }),
      "Track fab capacity.",
    );
    await user.click(screen.getByRole("button", { name: "Save topic" }));

    expect(writePayloads).toEqual([
      {
        path: "Briefings/Topics/chips.md",
        content:
          "---\n" +
          "kind: briefing_topic\n" +
          "slug: chips\n" +
          "name: Chip supply\n" +
          "section_order: 1000\n" +
          "mode: every_briefing\n" +
          "editions: [morning]\n" +
          "entities: []\n" +
          "symbols: []\n" +
          "suppress_unchanged: true\n" +
          "freshness_hours: 48\n" +
          "---\n" +
          "\n" +
          "Track fab capacity.\n",
        expected_version: 0,
      },
    ]);
  });

  it("renders parse-error topics with a warning and a raw document editor", async () => {
    const writePayloads: unknown[] = [];
    installApiMock({
      "GET /api/v1/workspace/briefings/topics": briefingTopicsFixture,
      "POST /api/v1/workspace/write": async (request: Request) => {
        writePayloads.push(await request.json());
        return writeReceipt("Briefings/Topics/broken.md", 2);
      },
    });
    const user = userEvent.setup();
    renderApp("/topics", "owner-token");

    await user.click(
      await screen.findByRole("row", { name: "Edit topic broken" }),
    );
    expect(
      screen.getByText(
        "frontmatter is missing or not closed; raw content preserved as body",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "Name" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("textbox", { name: /Raw document/ }),
    ).toHaveValue("kind: briefing_topic\nno closing fence\n");
    await user.click(screen.getByRole("button", { name: "Save topic" }));

    expect(writePayloads).toEqual([
      {
        path: "Briefings/Topics/broken.md",
        content: "kind: briefing_topic\nno closing fence\n",
        expected_version: 1,
      },
    ]);
  });

  it("renders pending requests and the feedback tail", async () => {
    installApiMock({
      "GET /api/v1/workspace/briefings/topics": briefingTopicsFixture,
    });
    renderApp("/topics", "owner-token");

    expect(await screen.findByText("openai-o5")).toBeInTheDocument();
    expect(screen.getByText("Wants eval-harness detail")).toBeInTheDocument();
    expect(screen.getByText("Pending")).toBeInTheDocument();
    expect(
      screen.getByText(/2026-08-01 repeated nvda-earnings/),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Briefings/Feedback/2026-08.md"),
    ).toBeInTheDocument();
  });

  it("keeps topic writes disabled for read-only credentials", async () => {
    installApiMock({
      "GET /api/v1/me": readOnlyMe(),
      "GET /api/v1/workspace/briefings/topics": briefingTopicsFixture,
    });
    const user = userEvent.setup();
    renderApp("/topics", "readonly-token");

    expect(
      await screen.findByRole("button", { name: "New topic" }),
    ).toBeDisabled();
    await user.click(
      await screen.findByRole("row", { name: "Edit topic Stock watchlist" }),
    );
    expect(screen.getByRole("textbox", { name: "Name" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Save topic" })).toBeDisabled();
  });
});
