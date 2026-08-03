import axe from "axe-core";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { NotificationItem } from "../lib/types";
import { briefingEditionFixture, briefingListFixture } from "./briefingFixtures";
import { installApiMock, renderApp } from "./renderApp";

const notificationRef = "notification:019f8800000070008000000000000001";
const deliveryRef = "delivery:019f8800000070008000000000000002";

const briefingAlert = {
  notification_ref: notificationRef,
  kind: "news_alert",
  importance: "important",
  title: "A material update",
  body: "The underlying **fact changed** after the morning edition.",
  source: {
    type: "briefing_item",
    ref: "entry:019f8800-0000-7000-8000-000000000001",
    version_ref: "entry:019f8800-0000-7000-8000-000000000001@2",
  },
  target: {
    type: "briefing",
    date: "2026-08-02",
    edition: "morning",
    item_id: "material-update",
  },
  occurred_at: "2026-08-02T18:00:00Z",
  expires_at: "2026-08-03T18:00:00Z",
  opened_at: null,
  acknowledged_at: null,
  deliveries: [{
    delivery_ref: deliveryRef,
    state: "accepted_by_apns",
    accepted_at: "2026-08-02T18:00:03Z",
    failed_at: null,
    last_error_code: null,
  }, {
    delivery_ref: "delivery:019f8800000070008000000000000006",
    state: "suppressed",
    accepted_at: null,
    failed_at: null,
    last_error_code: "transport_disabled",
  }],
} satisfies NotificationItem;

describe("durable alert inbox", () => {
  it("lists private alerts and applies server-backed filters", async () => {
    const fetchMock = installApiMock({
      "GET /api/v1/workspace/notifications": {
        items: [briefingAlert],
        next_cursor: null,
        unread_count: 1,
      },
    });
    const user = userEvent.setup();
    const { container } = renderApp("/alerts");

    expect(await screen.findByRole("heading", { name: "Alerts" })).toBeInTheDocument();
    const card = await screen.findByRole("link", { name: /A material update/ });
    expect(screen.getByRole("group", { name: "Alert filters" })).toBeInTheDocument();
    expect(card).toHaveClass("is-unread");
    expect(within(card).getByText("Important")).toBeInTheDocument();
    expect(within(card).getByText("Unread")).toBeInTheDocument();
    expect(card).toHaveAttribute(
      "href",
      `/alerts/${encodeURIComponent(notificationRef)}`,
    );

    await user.click(screen.getByRole("button", { name: "Important" }));
    await waitFor(() => {
      const calls = fetchMock.mock.calls.map(([input]) => String(input));
      expect(calls.some((url) => url.includes("importance=important"))).toBe(true);
    });
    await user.click(screen.getByRole("button", { name: /Unread/ }));
    await waitFor(() => {
      const calls = fetchMock.mock.calls.map(([input]) => String(input));
      expect(calls.some((url) => url.includes("unread=true"))).toBe(true);
    });
    const accessibility = await axe.run(container, {
      rules: { "color-contrast": { enabled: false } },
    });
    expect(accessibility.violations).toEqual([]);
  });

  it("opens the exact detail, records receipts, and links the typed briefing target", async () => {
    const receiptBodies: unknown[] = [];
    installApiMock({
      [`GET /api/v1/workspace/notifications/${encodeURIComponent(notificationRef)}`]: {
        notification: briefingAlert,
      },
      [`POST /api/v1/workspace/notifications/${encodeURIComponent(notificationRef)}/receipts`]: async (request: Request) => {
        const body = await request.json() as { kind: "opened" | "acknowledged" };
        receiptBodies.push(body);
        return {
          notification_ref: notificationRef,
          kind: body.kind,
          delivery_ref: null,
          recorded_at: "2026-08-02T18:01:00Z",
          replayed: false,
          opened_at: "2026-08-02T18:01:00Z",
          acknowledged_at:
            body.kind === "acknowledged" ? "2026-08-02T18:02:00Z" : null,
        };
      },
    });
    const user = userEvent.setup();
    const { container } = renderApp(`/alerts/${notificationRef}`);

    expect(
      await screen.findByRole("heading", { name: "A material update" }),
    ).toHaveFocus();
    expect(screen.getByText(/fact changed/)).toBeInTheDocument();
    expect(screen.getByText(/Accepted by provider/)).toBeInTheDocument();
    expect(screen.getByText("Suppressed")).toBeInTheDocument();
    expect(screen.getByText("Not sent to the push provider")).toBeInTheDocument();
    expect(screen.getByText("transport_disabled")).toBeInTheDocument();
    const target = screen.getByRole("link", { name: "Open briefing" });
    expect(target).toHaveAttribute(
      "href",
      "/briefings/2026-08-02?edition=morning&item=material-update",
    );
    await waitFor(() => expect(receiptBodies).toContainEqual({ kind: "opened" }));

    await user.click(screen.getByRole("button", { name: "Acknowledge" }));
    await waitFor(() => {
      expect(receiptBodies).toContainEqual({ kind: "acknowledged" });
      expect(screen.getByRole("button", { name: "Acknowledged" })).toBeDisabled();
    });

    const accessibility = await axe.run(container, {
      rules: { "color-contrast": { enabled: false } },
    });
    expect(accessibility.violations).toEqual([]);
  });

  it("resolves an entry target inside the app while retaining pinned source metadata", async () => {
    const entryAlert = {
      ...briefingAlert,
      notification_ref: "notification:019f8800000070008000000000000003",
      kind: "operational",
      title: "Review the recovery record",
      target: {
        type: "entry",
        entry_ref: "entry:019f8800-0000-7000-8000-000000000004",
      },
      source: {
        type: "entry",
        ref: "entry:019f8800-0000-7000-8000-000000000004",
        version_ref: "entry-version:019f8800-0000-7000-8000-000000000005",
      },
      deliveries: [],
      opened_at: "2026-08-02T18:01:00Z",
    };
    let readBody: unknown;
    const encodedRef = encodeURIComponent(entryAlert.notification_ref);
    installApiMock({
      [`GET /api/v1/workspace/notifications/${encodedRef}`]: {
        notification: entryAlert,
      },
      "POST /api/v1/workspace/read": async (request: Request) => {
        readBody = await request.json();
        return {
          status: "complete",
          data: {
            items: [{
              reference: entryAlert.target.entry_ref,
              path: "Projects/Straylight/Recovery.md",
              title: "Recovery record",
              version: 3,
              content_hash: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
              media_type: "text/markdown",
              view: "full",
              text: "# Recovery\n\nThe service recovered.",
              metadata: {},
            }],
          },
        };
      },
    });
    renderApp(`/alerts/${entryAlert.notification_ref}`);

    expect(await screen.findByText("Recovery record")).toBeInTheDocument();
    expect(screen.getByLabelText("Recovery record content")).toHaveTextContent(
      "The service recovered.",
    );
    expect(screen.getByText(entryAlert.source.version_ref)).toBeInTheDocument();
    expect(readBody).toEqual({
      requests: [{ ref: entryAlert.target.entry_ref, view: "full" }],
    });
  });

  it("carries a briefing item target through the route and opens that item", async () => {
    installApiMock({
      "GET /api/v1/workspace/briefings": briefingListFixture,
      "GET /api/v1/workspace/briefings/2026-08-01/morning": briefingEditionFixture,
    });
    renderApp("/briefings/2026-08-01?edition=morning&item=openai-o5");

    const row = await screen.findByRole("button", { name: /OpenAI ships o5/ });
    expect(row).toHaveAttribute("aria-expanded", "true");
    expect(
      screen.getByRole("region", { name: "Frontier labs item detail" }),
    ).toBeInTheDocument();
    expect(row.closest(".briefing-item")).toHaveClass("is-targeted");
  });
});
