import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { LoaderCircle, MessageCircle, Plus, RefreshCw } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { EmptyState } from "../components/StateViews";
import { Page, PageHeader } from "../components/Page";
import { useApi } from "../lib/auth";
import { useCurrent } from "../lib/current";
import { formatRelative } from "../lib/format";
import type {
  ApiEnvelope,
  CredentialSummary,
  ListData,
} from "../lib/types";
import { AgentRegistryPanel } from "./AgentRegistryPanel";
import { createMessagingApiClient } from "./api";
import { ConversationThread } from "./ConversationThread";
import {
  conversationTitle,
  genericMessagingError,
  hasCapability,
  mergeConversations,
  mergeMessages,
} from "./model";
import { NewConversationPicker } from "./NewConversationPicker";
import type {
  CreateAgentInput,
  MessagingConversation,
  SendMessageInput,
  UpdateAgentInput,
} from "./types";
import "./messaging.css";

function credentialItems(
  envelope:
    | ApiEnvelope<ListData<CredentialSummary> | CredentialSummary[]>
    | undefined,
): CredentialSummary[] {
  if (!envelope) return [];
  return Array.isArray(envelope.data) ? envelope.data : envelope.data.items;
}

export function AgentsPage() {
  const current = useCurrent();
  const coreApi = useApi();
  const messagingApi = useMemo(createMessagingApiClient, []);
  const queryClient = useQueryClient();
  const [chosenConversationId, setChosenConversationId] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const markedRead = useRef(new Map<string, number>());
  const capabilities = current.data.capabilities;
  const canRead = hasCapability(capabilities, "message.read");
  const canWrite = hasCapability(capabilities, "message.write");
  const canManage = hasCapability(capabilities, "credential:manage");

  const inboxQuery = useInfiniteQuery({
    queryKey: ["messaging", "inbox"],
    queryFn: ({ pageParam }: { pageParam: number }) =>
      messagingApi.sync({ cursor: pageParam, limit: 200 }),
    initialPageParam: 0,
    getNextPageParam: (page) =>
      page.data.has_more ? page.data.cursor : undefined,
    enabled: canRead,
    refetchInterval: 5_000,
  });
  const inboxPages = inboxQuery.data?.pages.map((page) => page.data) ?? [];
  const conversations = mergeConversations(inboxPages);
  const inboxPresence = inboxPages.at(-1)?.presence ?? [];
  const selectedConversationId =
    chosenConversationId ?? conversations[0]?.conversation_id ?? null;

  const threadQuery = useInfiniteQuery({
    queryKey: ["messaging", "thread", selectedConversationId],
    queryFn: ({ pageParam }: { pageParam: number }) =>
      messagingApi.sync({
        conversationId: selectedConversationId ?? undefined,
        afterSeq: pageParam,
        limit: 200,
      }),
    initialPageParam: 0,
    getNextPageParam: (page) => {
      if (!page.data.has_more) return undefined;
      return page.data.messages.at(-1)?.seq;
    },
    enabled: canRead && selectedConversationId !== null,
    refetchInterval: 2_500,
  });
  const threadPages = threadQuery.data?.pages.map((page) => page.data) ?? [];
  const messages = mergeMessages(threadPages);
  const exactConversation = threadPages
    .flatMap((page) => page.conversations)
    .find((conversation) => conversation.conversation_id === selectedConversationId);
  const selectedConversation =
    exactConversation ??
    conversations.find(
      (conversation) => conversation.conversation_id === selectedConversationId,
    );

  const registryQuery = useQuery({
    queryKey: ["messaging", "agents"],
    queryFn: () => messagingApi.listAgents(),
    enabled: canRead,
    refetchInterval: 15_000,
  });
  const agents = registryQuery.data?.data.agents ?? inboxPresence;
  const credentialsQuery = useQuery({
    queryKey: ["credentials"],
    queryFn: () => coreApi.credentials(),
    enabled: canRead && canManage,
  });
  const credentials = credentialItems(credentialsQuery.data);

  const readMutation = useMutation({
    mutationFn: ({ conversationId, lastReadSeq }: {
      conversationId: string;
      lastReadSeq: number;
    }) => messagingApi.markRead(conversationId, lastReadSeq),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["messaging", "inbox"] });
    },
  });

  useEffect(() => {
    if (!canWrite || !selectedConversation || messages.length === 0) return;
    const lastSeq = messages.at(-1)?.seq ?? 0;
    const alreadyMarked = markedRead.current.get(selectedConversation.conversation_id) ?? 0;
    if (
      lastSeq <= selectedConversation.last_read_seq ||
      lastSeq <= alreadyMarked
    ) {
      return;
    }
    markedRead.current.set(selectedConversation.conversation_id, lastSeq);
    readMutation.mutate(
      {
        conversationId: selectedConversation.conversation_id,
        lastReadSeq: lastSeq,
      },
      {
        onError: () => markedRead.current.delete(selectedConversation.conversation_id),
      },
    );
  }, [canWrite, messages, readMutation, selectedConversation]);

  async function refreshMessaging() {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["messaging", "inbox"] }),
      queryClient.invalidateQueries({ queryKey: ["messaging", "thread"] }),
      queryClient.invalidateQueries({ queryKey: ["messaging", "agents"] }),
    ]);
  }

  async function createConversation(participants: string[], subject?: string) {
    const response = await messagingApi.createConversation(participants, subject);
    setChosenConversationId(response.data.conversation_id);
    await refreshMessaging();
  }

  async function sendMessage(input: SendMessageInput) {
    if (!selectedConversationId) throw new Error("No conversation selected");
    const response = await messagingApi.sendMessage(selectedConversationId, input);
    if (response.data.continuation_id) {
      setChosenConversationId(response.data.continuation_id);
    }
    await refreshMessaging();
  }

  async function createAgent(input: CreateAgentInput) {
    await messagingApi.createAgent(input);
    await queryClient.invalidateQueries({ queryKey: ["messaging", "agents"] });
  }

  async function updateAgent(agentId: string, input: UpdateAgentInput) {
    await messagingApi.updateAgent(agentId, input);
    await queryClient.invalidateQueries({ queryKey: ["messaging", "agents"] });
  }

  async function bindCredential(agentId: string, credentialId: string | null) {
    await messagingApi.bindCredential(agentId, credentialId);
    await queryClient.invalidateQueries({ queryKey: ["messaging", "agents"] });
  }

  if (!canRead) {
    return (
      <Page>
        <PageHeader title="Agents" description="Durable conversations with resident agents" />
        <EmptyState
          title="Messaging is not available"
          detail="This session needs the message.read capability."
        />
      </Page>
    );
  }

  return (
    <Page>
      <PageHeader
        title="Agents"
        description="Durable conversations with resident and task-time agents"
        actions={
          <>
            <button
              className="button secondary"
              type="button"
              onClick={() => void refreshMessaging()}
              disabled={inboxQuery.isFetching}
            >
              <RefreshCw size={16} aria-hidden="true" />
              Refresh
            </button>
            <button
              className="button primary"
              type="button"
              onClick={() => setPickerOpen(true)}
              disabled={!canWrite}
            >
              <Plus size={16} aria-hidden="true" />
              New conversation
            </button>
          </>
        }
      />

      {!canWrite ? (
        <div className="messaging-view-only page-notice" role="status">
          Messaging is view only
        </div>
      ) : null}
      {inboxQuery.isError ? (
        <div className="messaging-error messaging-page-error" role="alert">
          <span>{genericMessagingError(inboxQuery.error)}</span>
          <button
            className="button secondary"
            type="button"
            onClick={() => void inboxQuery.refetch()}
          >
            Try again
          </button>
        </div>
      ) : null}

      <div className="messaging-layout">
        <section className="messaging-inbox" aria-label="Conversation inbox">
          <header>
            <div>
              <h2>Conversations</h2>
              <span>{conversations.length}</span>
            </div>
            {inboxQuery.isFetching ? (
              <LoaderCircle className="spin" size={17} aria-label="Refreshing conversations" />
            ) : null}
          </header>
          {inboxQuery.isLoading ? (
            <div className="messaging-inline-state" role="status">
              <LoaderCircle className="spin" size={18} aria-hidden="true" />
              Loading conversations
            </div>
          ) : null}
          {!inboxQuery.isLoading && conversations.length === 0 ? (
            <div className="messaging-empty-inbox">
              <MessageCircle size={22} aria-hidden="true" />
              <strong>No conversations yet</strong>
              <span>Start one with an active agent.</span>
            </div>
          ) : null}
          <ul className="messaging-conversation-list" aria-label="Conversations">
            {conversations.map((conversation) => (
              <li key={conversation.conversation_id}>
                <button
                  type="button"
                  className={
                    conversation.conversation_id === selectedConversationId
                      ? "selected"
                      : undefined
                  }
                  aria-pressed={conversation.conversation_id === selectedConversationId}
                  onClick={() => setChosenConversationId(conversation.conversation_id)}
                >
                  <span className="messaging-conversation-title">
                    <strong>{conversationTitle(conversation, agents)}</strong>
                    <time dateTime={conversation.last_message_at ?? undefined}>
                      {formatRelative(conversation.last_message_at)}
                    </time>
                  </span>
                  <span className="messaging-conversation-meta">
                    {conversation.needs_human ? (
                      <span className="messaging-badge attention">Needs human</span>
                    ) : null}
                    {conversation.unread_count > 0 ? (
                      <span className="messaging-badge unread">
                        {conversation.unread_count} unread
                      </span>
                    ) : null}
                    <span>{conversation.status.replaceAll("_", " ")}</span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
          {inboxQuery.hasNextPage ? (
            <button
              className="button secondary messaging-load-more"
              type="button"
              disabled={inboxQuery.isFetchingNextPage}
              onClick={() => void inboxQuery.fetchNextPage()}
            >
              {inboxQuery.isFetchingNextPage ? "Loading…" : "Load more conversations"}
            </button>
          ) : null}
        </section>

        {selectedConversation ? (
          <ConversationThread
            key={selectedConversation.conversation_id}
            conversation={selectedConversation}
            messages={messages}
            agents={agents}
            canWrite={canWrite}
            loading={threadQuery.isLoading}
            hasMore={Boolean(threadQuery.hasNextPage)}
            loadingMore={threadQuery.isFetchingNextPage}
            onLoadMore={() => void threadQuery.fetchNextPage()}
            onSend={sendMessage}
          />
        ) : selectedConversationId ? (
          <section className="messaging-thread messaging-inline-state" role="status">
            <LoaderCircle className="spin" size={18} aria-hidden="true" />
            Loading conversation
          </section>
        ) : (
          <section className="messaging-thread messaging-empty-selection">
            <MessageCircle size={24} aria-hidden="true" />
            <strong>Select a conversation</strong>
          </section>
        )}
      </div>

      {threadQuery.isError ? (
        <div className="messaging-error messaging-page-error" role="alert">
          {genericMessagingError(threadQuery.error)}
        </div>
      ) : null}

      <AgentRegistryPanel
        agents={agents}
        credentials={credentials}
        canManage={canManage}
        loading={registryQuery.isLoading}
        error={registryQuery.error ?? credentialsQuery.error}
        onCreate={createAgent}
        onUpdate={updateAgent}
        onBind={bindCredential}
      />
      <NewConversationPicker
        open={pickerOpen}
        agents={agents}
        onClose={() => setPickerOpen(false)}
        onCreate={createConversation}
      />
    </Page>
  );
}

export type { MessagingConversation };
