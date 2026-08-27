import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import axe from "axe-core";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { AuthProvider } from "../lib/auth";
import { CurrentProvider } from "../lib/current";
import { defaultMe, installApiMock } from "../test/renderApp";
import type { ApiEnvelope, MeData } from "../lib/types";
import { AgentsPage } from "./AgentsPage";
import {
  genericMessagingError,
  sortConversations,
  type MessagingConversation,
} from "./model";

const firstConversationId = "019f9000-0000-7000-8000-000000000001";
const attentionConversationId = "019f9000-0000-7000-8000-000000000002";
const messageId = "019f9000-0000-7000-8000-000000000003";
const now = "2026-08-27T09:00:00Z";

const agents = [
  {
    agent_id: "aether",
    display_name: "Aether",
    principal_kind: "owner",
    delivery_mode: "pull",
    online: true,
    last_seen_at: now,
    lease_expires_at: "2026-08-27T09:01:00Z",
    archived: false,
    credential_names: [],
  },
  {
    agent_id: "echo",
    display_name: "Echo",
    principal_kind: "resident",
    delivery_mode: "pull",
    online: true,
    last_seen_at: now,
    lease_expires_at: "2026-08-27T09:01:00Z",
    archived: false,
    credential_names: ["Echo resident"],
  },
  {
    agent_id: "codex",
    display_name: "Codex",
    principal_kind: "task-time",
    delivery_mode: "pull",
    online: false,
    last_seen_at: "2026-08-27T08:30:00Z",
    lease_expires_at: null,
    archived: false,
    credential_names: [],
  },
];

const conversations: MessagingConversation[] = [
  {
    conversation_id: firstConversationId,
    conversation_kind: "direct",
    subject: null,
    status: "open",
    participants: [
      { agent_id: "aether", role: "participant" },
      { agent_id: "echo", role: "participant" },
    ],
    last_seq: 1,
    last_message_at: "2026-08-27T08:59:00Z",
    last_read_seq: 1,
    unread_count: 0,
    needs_human: false,
    continues_from: null,
    continuation_id: null,
    latest_sync_cursor: 2,
  },
  {
    conversation_id: attentionConversationId,
    conversation_kind: "group",
    subject: "Release decision",
    status: "paused_for_human",
    participants: [
      { agent_id: "aether", role: "observer" },
      { agent_id: "codex", role: "participant" },
      { agent_id: "echo", role: "participant" },
    ],
    last_seq: 4,
    last_message_at: "2026-08-27T08:45:00Z",
    last_read_seq: 2,
    unread_count: 2,
    needs_human: true,
    continues_from: null,
    continuation_id: null,
    latest_sync_cursor: 7,
  },
];

const attentionMessage = {
  conversation_id: attentionConversationId,
  seq: 4,
  message_id: messageId,
  from_agent_id: "codex",
  client_key: "01K3N0RTH00000000000000000",
  kind: "question",
  body_md: "Should we ship the guarded route?",
  refs: [],
  in_reply_to: null,
  correlation_id: null,
  expects_reply: true,
  reply_by: null,
  sync_cursor: 7,
  created_at: "2026-08-27T08:45:00Z",
};

function syncResponse(
  options: {
    messages?: typeof attentionMessage[];
    exactConversation?: MessagingConversation;
    cursor?: number;
    hasMore?: boolean;
  } = {},
) {
  return {
    status: "complete",
    data: {
      status: "complete",
      cursor: options.cursor ?? 7,
      resume_cursor: null,
      has_more: options.hasMore ?? false,
      messages: options.messages ?? [],
      conversations: options.exactConversation
        ? [options.exactConversation]
        : conversations,
      presence: agents,
      unread: {
        [firstConversationId]: 0,
        [attentionConversationId]: 2,
      },
      as_of: now,
    },
  };
}

function messagingMe(): ApiEnvelope<MeData> {
  const me = structuredClone(defaultMe) as ApiEnvelope<MeData>;
  me.data.capabilities = [
    ...(me.data.capabilities ?? []),
    "message.read",
    "message.write",
  ];
  return me;
}

function renderAgents(me = messagingMe()) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: Number.POSITIVE_INFINITY },
      mutations: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <AuthProvider>
        <CurrentProvider value={me}>
          <AgentsPage />
        </CurrentProvider>
      </AuthProvider>
    </QueryClientProvider>,
  );
}

function installMessagingRoutes(
  routes: Record<
    string,
    unknown | ((request: Request) => unknown | Promise<unknown>)
  > = {},
) {
  return installApiMock({
    "GET /api/v1/workspace/messaging/sync": (request: Request) => {
      const query = new URL(request.url).searchParams;
      return query.has("conversation_id")
        ? syncResponse({
            messages: [attentionMessage],
            exactConversation: conversations[1],
          })
        : syncResponse();
    },
    "GET /api/v1/workspace/messaging/agents": {
      status: "complete",
      data: { agents, as_of: now },
    },
    "GET /api/v1/credentials": {
      status: "complete",
      data: {
        items: [
          {
            id: "credential:019f9000-0000-7000-8000-000000000010",
            name: "Echo replacement",
            access: "read_write",
            scope_ids: ["scope:root"],
          },
        ],
      },
    },
    [`POST /api/v1/workspace/messaging/conversations/${attentionConversationId}/read`]: {
      status: "no_op",
      data: {
        conversation_id: attentionConversationId,
        last_read_seq: 4,
        cursor: 7,
        duplicate: true,
      },
    },
    ...routes,
  });
}

describe("Agents page model", () => {
  it("puts needs-human conversations ahead of newer routine traffic", () => {
    expect(sortConversations(conversations).map((item) => item.conversation_id)).toEqual([
      attentionConversationId,
      firstConversationId,
    ]);
  });

  it("uses one generic, non-sensitive offline message", () => {
    expect(genericMessagingError(new TypeError("private network detail"))).toBe(
      "Agents is offline. Check your connection and try again.",
    );
  });
});

describe("Agents page", () => {
  it("lists attention first, opens a thread, and is accessible", async () => {
    installMessagingRoutes();
    const { container } = renderAgents();

    expect(await screen.findByRole("heading", { name: "Agents" })).toBeInTheDocument();
    const list = await screen.findByRole("list", { name: "Conversations" });
    const items = within(list).getAllByRole("button");
    expect(items[0]).toHaveAccessibleName(/Release decision/);
    expect(within(items[0]).getByText("Needs human")).toBeInTheDocument();
    await waitFor(() =>
      expect(
        screen.getByText("Should we ship the guarded route?"),
      ).toBeInTheDocument(),
    );
    expect(screen.getAllByText("Codex").length).toBeGreaterThan(0);

    const accessibility = await axe.run(container, {
      rules: { "color-contrast": { enabled: false } },
    });
    expect(accessibility.violations).toEqual([]);
  });

  it("follows inbox and thread cursors through the current head", async () => {
    const inboxCursors: string[] = [];
    const threadCursors: string[] = [];
    const newestMessage = {
      ...attentionMessage,
      seq: 5,
      message_id: "019f9000-0000-7000-8000-000000000005",
      body_md: "The newest cursor is visible.",
      sync_cursor: 8,
    };
    const refreshedMessage = {
      ...newestMessage,
      seq: 6,
      message_id: "019f9000-0000-7000-8000-000000000006",
      body_md: "The next poll starts at the saved cursor.",
      sync_cursor: 9,
    };
    installMessagingRoutes({
      "GET /api/v1/workspace/messaging/sync": (request: Request) => {
        const query = new URL(request.url).searchParams;
        if (query.has("conversation_id")) {
          const afterSeq = query.get("after_seq") ?? "0";
          threadCursors.push(afterSeq);
          if (afterSeq === "0") {
            return syncResponse({
              messages: [attentionMessage],
              exactConversation: conversations[1],
              cursor: 7,
              hasMore: true,
            });
          }
          return afterSeq === "4"
            ? syncResponse({
                messages: [newestMessage],
                exactConversation: {
                  ...conversations[1],
                  last_seq: 5,
                  latest_sync_cursor: 8,
                },
                cursor: 8,
              })
            : syncResponse({
                messages: [refreshedMessage],
                exactConversation: {
                  ...conversations[1],
                  last_seq: 6,
                  latest_sync_cursor: 9,
                },
                cursor: 9,
              });
        }
        const cursor = query.get("cursor") ?? "0";
        inboxCursors.push(cursor);
        if (cursor === "0") {
          return syncResponse({ cursor: 7, hasMore: true });
        }
        return cursor === "7"
          ? syncResponse({
              cursor: 8,
              exactConversation: {
                ...conversations[1],
                last_seq: 5,
                latest_sync_cursor: 8,
              },
            })
          : syncResponse({
              cursor: 9,
              exactConversation: {
                ...conversations[1],
                last_seq: 6,
                latest_sync_cursor: 9,
              },
            });
      },
    });
    renderAgents();

    await waitFor(() => {
      expect(screen.getByText("The newest cursor is visible.")).toBeInTheDocument();
      expect(inboxCursors).toContain("7");
      expect(threadCursors).toContain("4");
    });
    const initialInboxZeroCalls = inboxCursors.filter((cursor) => cursor === "0").length;
    const initialThreadZeroCalls = threadCursors.filter((cursor) => cursor === "0").length;
    await userEvent.setup().click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => {
      expect(
        screen.getByText("The next poll starts at the saved cursor."),
      ).toBeInTheDocument();
      expect(inboxCursors).toContain("8");
      expect(threadCursors).toContain("5");
    });
    expect(inboxCursors.filter((cursor) => cursor === "0")).toHaveLength(
      initialInboxZeroCalls,
    );
    expect(threadCursors.filter((cursor) => cursor === "0")).toHaveLength(
      initialThreadZeroCalls,
    );
  });

  it("keeps the same client key when an ambiguous send is retried", async () => {
    const bodies: Array<Record<string, unknown>> = [];
    let attempts = 0;
    document.cookie = "straylight_csrf=csrf-test; path=/";
    installMessagingRoutes({
      [`POST /api/v1/workspace/messaging/conversations/${attentionConversationId}/messages`]: async (
        request: Request,
      ) => {
        bodies.push((await request.json()) as Record<string, unknown>);
        expect(request.headers.get("X-CSRF-Token")).toBe("csrf-test");
        attempts += 1;
        if (attempts === 1) throw new TypeError("socket closed after write");
        return {
          status: "committed",
          data: {
            conversation_id: attentionConversationId,
            seq: 5,
            message: { ...attentionMessage, seq: 5, body_md: "Ship it guarded." },
            duplicate: false,
            continuation_id: null,
          },
        };
      },
    });
    const user = userEvent.setup();
    renderAgents();

    const composer = await screen.findByRole("textbox", { name: "Message" });
    await user.type(composer, "Ship it guarded.");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Agents is offline. Check your connection and try again.",
    );
    await user.click(screen.getByRole("button", { name: "Retry send" }));
    await waitFor(() => expect(bodies).toHaveLength(2));

    expect(bodies[0].client_key).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);
    expect(bodies[1].client_key).toBe(bodies[0].client_key);
    expect(bodies[0]).not.toHaveProperty("from");
  });

  it("uses exact message.write capability for a view-only thread", async () => {
    installMessagingRoutes();
    const me = structuredClone(defaultMe) as ApiEnvelope<MeData>;
    me.data.capabilities = ["message.read", "credential:manage"];
    me.data.read_only = false;
    renderAgents(me);

    expect(await screen.findByText("Messaging is view only")).toBeInTheDocument();
    expect(await screen.findByRole("textbox", { name: "Message" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });

  it("creates a conversation from the picker without accepting sender identity", async () => {
    let body: Record<string, unknown> | undefined;
    document.cookie = "straylight_csrf=csrf-test; path=/";
    installMessagingRoutes({
      "POST /api/v1/workspace/messaging/conversations": async (request: Request) => {
        body = (await request.json()) as Record<string, unknown>;
        expect(request.headers.get("X-CSRF-Token")).toBe("csrf-test");
        return {
          status: "committed",
          data: {
            conversation_id: firstConversationId,
            conversation: conversations[0],
            duplicate: false,
          },
        };
      },
    });
    const user = userEvent.setup();
    renderAgents();

    await user.click(await screen.findByRole("button", { name: "New conversation" }));
    const picker = screen.getByRole("dialog", { name: "New conversation" });
    await user.click(within(picker).getByRole("checkbox", { name: /Echo/ }));
    await user.type(within(picker).getByRole("textbox", { name: "Subject" }), "Canary");
    await user.click(within(picker).getByRole("button", { name: "Create conversation" }));
    await waitFor(() => expect(body).toEqual({ participants: ["echo"], subject: "Canary" }));
    expect(body).not.toHaveProperty("from");
  });

  it("binds a credential from the owner registry panel with CSRF", async () => {
    let body: Record<string, unknown> | undefined;
    document.cookie = "straylight_csrf=csrf-test; path=/";
    installMessagingRoutes({
      "PUT /api/v1/workspace/messaging/agents/echo/credential": async (
        request: Request,
      ) => {
        body = (await request.json()) as Record<string, unknown>;
        expect(request.headers.get("X-CSRF-Token")).toBe("csrf-test");
        return {
          status: "committed",
          data: {
            agent_id: "echo",
            credential_id: body.credential_id,
            bound: true,
          },
        };
      },
    });
    const user = userEvent.setup();
    renderAgents();

    await user.click(await screen.findByText("Registry settings"));
    const row = screen.getByRole("group", { name: "Echo settings" });
    await user.selectOptions(
      within(row).getByRole("combobox", { name: "Credential" }),
      "credential:019f9000-0000-7000-8000-000000000010",
    );
    await user.click(within(row).getByRole("button", { name: "Apply binding" }));
    await waitFor(() =>
      expect(body).toEqual({
        credential_id: "019f9000-0000-7000-8000-000000000010",
      }),
    );
  });
});
