import { useMutation } from "@tanstack/react-query";
import { Clock3, CornerUpLeft, LoaderCircle, Send, TriangleAlert } from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";
import { MarkdownView } from "../components/MarkdownView";
import { formatDate } from "../lib/format";
import {
  agentName,
  conversationTitle,
  genericMessagingError,
  MAX_MESSAGE_BYTES,
  newClientKey,
  utf8ByteLength,
} from "./model";
import type {
  MessagingAgent,
  MessagingConversation,
  MessagingMessage,
  SendMessageInput,
} from "./types";

export function ConversationThread({
  conversation,
  messages,
  agents,
  canWrite,
  loading,
  hasMore,
  loadingMore,
  onLoadMore,
  onSend,
}: {
  conversation: MessagingConversation;
  messages: MessagingMessage[];
  agents: MessagingAgent[];
  canWrite: boolean;
  loading: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  onLoadMore: () => void;
  onSend: (input: SendMessageInput) => Promise<void>;
}) {
  const [body, setBody] = useState("");
  const [kind, setKind] = useState<"text" | "question">("text");
  const [replyBy, setReplyBy] = useState("");
  const [replyTo, setReplyTo] = useState<number | undefined>();
  const [failedAttempt, setFailedAttempt] = useState<SendMessageInput | null>(null);
  const bodyBytes = useMemo(() => utf8ByteLength(body), [body]);
  const sendMutation = useMutation({
    mutationFn: onSend,
    onSuccess: () => {
      setBody("");
      setKind("text");
      setReplyBy("");
      setReplyTo(undefined);
      setFailedAttempt(null);
    },
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmed = body.trim();
    if (!canWrite || !trimmed || bodyBytes > MAX_MESSAGE_BYTES) return;
    const attempt: SendMessageInput = {
      client_key: newClientKey(),
      kind,
      body_md: trimmed,
      refs: [],
      ...(replyTo ? { in_reply_to: replyTo } : {}),
      expects_reply: kind === "question",
      ...(kind === "question" && replyBy
        ? { reply_by: new Date(replyBy).toISOString() }
        : {}),
    };
    setFailedAttempt(attempt);
    sendMutation.mutate(attempt);
  }

  return (
    <section className="messaging-thread" aria-label="Conversation thread">
      <header className="messaging-thread-header">
        <div>
          <h2>{conversationTitle(conversation, agents)}</h2>
          <span>
            {conversation.participants.length} participant
            {conversation.participants.length === 1 ? "" : "s"}
          </span>
        </div>
        <div className="messaging-thread-status">
          {conversation.needs_human ? (
            <span className="messaging-badge attention">Needs human</span>
          ) : null}
          <span className="messaging-badge">{conversation.status.replaceAll("_", " ")}</span>
        </div>
      </header>

      <div className="messaging-message-scroll" aria-live="polite">
        {hasMore ? (
          <button
            className="button secondary messaging-load-more"
            type="button"
            disabled={loadingMore}
            onClick={onLoadMore}
          >
            {loadingMore ? "Loading…" : "Load more messages"}
          </button>
        ) : null}
        {loading && messages.length === 0 ? (
          <div className="messaging-inline-state" role="status">
            <LoaderCircle className="spin" size={18} aria-hidden="true" />
            Loading conversation
          </div>
        ) : null}
        {!loading && messages.length === 0 ? (
          <div className="messaging-empty-thread">
            <strong>No messages yet</strong>
            <span>Start this durable conversation below.</span>
          </div>
        ) : null}
        <ol className="messaging-message-list">
          {messages.map((message) => (
            <li
              className={`messaging-message ${message.kind === "system" ? "system" : ""}`.trim()}
              key={`${message.conversation_id}:${message.seq}`}
            >
              <header>
                <strong>{agentName(message.from_agent_id, agents)}</strong>
                <span>#{message.seq}</span>
                <time dateTime={message.created_at}>{formatDate(message.created_at)}</time>
                {message.kind === "question" ? (
                  <span className="messaging-badge question">Question</span>
                ) : null}
              </header>
              <MarkdownView markdown={message.body_md} />
              {message.reply_by ? (
                <p className="messaging-deadline">
                  <Clock3 size={14} aria-hidden="true" />
                  Reply requested by {formatDate(message.reply_by)}
                </p>
              ) : null}
              {canWrite && message.kind !== "system" ? (
                <button
                  className="messaging-reply-button"
                  type="button"
                  onClick={() => setReplyTo(message.seq)}
                  aria-label={`Reply to message ${message.seq}`}
                >
                  <CornerUpLeft size={14} aria-hidden="true" />
                  Reply
                </button>
              ) : null}
            </li>
          ))}
        </ol>
      </div>

      <form className="messaging-composer" onSubmit={submit}>
        {!canWrite ? (
          <div className="messaging-view-only" role="status">
            Messaging is view only
          </div>
        ) : null}
        {conversation.needs_human && canWrite ? (
          <div className="messaging-attention-note" role="status">
            Your reply will resume this conversation.
          </div>
        ) : null}
        {replyTo ? (
          <div className="messaging-replying-to">
            <span>Replying to #{replyTo}</span>
            <button type="button" onClick={() => setReplyTo(undefined)}>
              Cancel reply
            </button>
          </div>
        ) : null}
        <label className="field messaging-composer-body">
          <span>Message</span>
          <textarea
            value={body}
            onChange={(event) => setBody(event.target.value)}
            disabled={!canWrite || sendMutation.isPending || conversation.status === "closed"}
            placeholder="Write a short durable message"
            rows={4}
          />
        </label>
        <div className="messaging-composer-options">
          <label className="field">
            <span>Kind</span>
            <select
              value={kind}
              onChange={(event) => {
                const nextKind = event.target.value as "text" | "question";
                setKind(nextKind);
                if (nextKind === "text") setReplyBy("");
              }}
              disabled={!canWrite || sendMutation.isPending}
            >
              <option value="text">Message</option>
              <option value="question">Question</option>
            </select>
          </label>
          {kind === "question" ? (
            <label className="field">
              <span>Reply by (optional)</span>
              <input
                type="datetime-local"
                value={replyBy}
                onChange={(event) => setReplyBy(event.target.value)}
                disabled={!canWrite || sendMutation.isPending}
              />
            </label>
          ) : null}
          <span
            className={`messaging-byte-count ${bodyBytes > MAX_MESSAGE_BYTES ? "over" : ""}`.trim()}
          >
            {bodyBytes.toLocaleString()} / {MAX_MESSAGE_BYTES.toLocaleString()} bytes
          </span>
          <button
            className="button primary"
            type="submit"
            disabled={
              !canWrite ||
              !body.trim() ||
              bodyBytes > MAX_MESSAGE_BYTES ||
              sendMutation.isPending ||
              conversation.status === "closed"
            }
          >
            <Send size={16} aria-hidden="true" />
            {sendMutation.isPending ? "Sending…" : "Send"}
          </button>
        </div>
        {sendMutation.isError ? (
          <div className="messaging-error" role="alert">
            <TriangleAlert size={16} aria-hidden="true" />
            <span>{genericMessagingError(sendMutation.error)}</span>
            {failedAttempt ? (
              <button
                className="button secondary"
                type="button"
                disabled={sendMutation.isPending}
                onClick={() => sendMutation.mutate(failedAttempt)}
              >
                Retry send
              </button>
            ) : null}
          </div>
        ) : null}
      </form>
    </section>
  );
}
