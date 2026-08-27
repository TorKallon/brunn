import { ApiError } from "../lib/api";
import type {
  MessagingAgent,
  MessagingConversation,
  MessagingMessage,
} from "./types";

export type {
  MessagingAgent,
  MessagingConversation,
  MessagingMessage,
} from "./types";

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
export const MAX_MESSAGE_BYTES = 16 * 1024;

export function hasCapability(
  capabilities: string[] | undefined,
  capability: string,
): boolean {
  return Boolean(
    capabilities?.includes(capability) || capabilities?.includes("admin"),
  );
}

export function sortConversations(
  conversations: MessagingConversation[],
): MessagingConversation[] {
  return conversations.slice().sort((left, right) => {
    if (left.needs_human !== right.needs_human) {
      return left.needs_human ? -1 : 1;
    }
    const leftTime = Date.parse(left.last_message_at ?? "") || 0;
    const rightTime = Date.parse(right.last_message_at ?? "") || 0;
    if (leftTime !== rightTime) return rightTime - leftTime;
    return left.conversation_id.localeCompare(right.conversation_id);
  });
}

export function mergeConversations(
  pages: Array<{ conversations: MessagingConversation[] }>,
): MessagingConversation[] {
  const byId = new Map<string, MessagingConversation>();
  for (const page of pages) {
    for (const conversation of page.conversations) {
      const current = byId.get(conversation.conversation_id);
      if (
        !current ||
        conversation.latest_sync_cursor >= current.latest_sync_cursor
      ) {
        byId.set(conversation.conversation_id, conversation);
      }
    }
  }
  return sortConversations(Array.from(byId.values()));
}

export function mergeMessages(
  pages: Array<{ messages: MessagingMessage[] }>,
): MessagingMessage[] {
  const byKey = new Map<string, MessagingMessage>();
  for (const page of pages) {
    for (const message of page.messages) {
      byKey.set(`${message.conversation_id}:${message.seq}`, message);
    }
  }
  return Array.from(byKey.values()).sort((left, right) => left.seq - right.seq);
}

export function conversationTitle(
  conversation: MessagingConversation,
  agents: MessagingAgent[],
): string {
  if (conversation.subject?.trim()) return conversation.subject.trim();
  const names = new Map(agents.map((agent) => [agent.agent_id, agent.display_name]));
  const nonOwners = conversation.participants.filter((participant) => {
    const agent = agents.find((candidate) => candidate.agent_id === participant.agent_id);
    return agent?.principal_kind !== "owner";
  });
  const visible = nonOwners.length > 0 ? nonOwners : conversation.participants;
  const labels = visible.map(
    (participant) => names.get(participant.agent_id) ?? participant.agent_id,
  );
  return labels.join(", ") || "Conversation";
}

export function agentName(agentId: string | null | undefined, agents: MessagingAgent[]) {
  if (!agentId) return "Straylight";
  return agents.find((agent) => agent.agent_id === agentId)?.display_name ?? agentId;
}

export function utf8ByteLength(value: string): number {
  return new TextEncoder().encode(value).byteLength;
}

export function newClientKey(): string {
  const random = new Uint8Array(10);
  globalThis.crypto.getRandomValues(random);
  let value = BigInt(Date.now());
  for (const byte of random) value = (value << 8n) | BigInt(byte);

  let encoded = "";
  for (let index = 0; index < 26; index += 1) {
    encoded = CROCKFORD[Number(value & 31n)] + encoded;
    value >>= 5n;
  }
  return encoded;
}

export function genericMessagingError(error: unknown): string {
  if (
    (error instanceof ApiError && error.status === 0) ||
    error instanceof TypeError
  ) {
    return "Agents is offline. Check your connection and try again.";
  }
  if (error instanceof ApiError && error.status === 404) {
    return "Agent messaging is not available in this environment.";
  }
  if (error instanceof ApiError && error.status === 403) {
    return "This session does not have permission for that messaging action.";
  }
  return "The messaging request could not be completed. Try again.";
}
