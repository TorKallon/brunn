export type ConversationStatus = "open" | "paused_for_human" | "closed";
export type MessageKind = "text" | "question" | "system";
export type PrincipalKind = "owner" | "resident" | "task-time";
export type DeliveryMode = "pull" | "apns" | "webhook";

export interface MessagingParticipant {
  agent_id: string;
  role: "participant" | "observer" | string;
}

export interface MessagingConversation {
  conversation_id: string;
  conversation_kind: "direct" | "group" | string;
  subject?: string | null;
  status: ConversationStatus | string;
  participants: MessagingParticipant[];
  last_seq: number;
  last_message_at?: string | null;
  last_read_seq: number;
  unread_count: number;
  needs_human: boolean;
  continues_from?: string | null;
  continuation_id?: string | null;
  latest_sync_cursor: number;
}

export interface MessagingRef {
  entry_ref?: string;
  url?: string;
  label?: string;
}

export interface MessagingMessage {
  conversation_id: string;
  seq: number;
  message_id: string;
  from_agent_id?: string | null;
  client_key?: string | null;
  kind: MessageKind | string;
  body_md: string;
  refs: MessagingRef[];
  in_reply_to_conversation_id?: string | null;
  in_reply_to?: number | null;
  correlation_id?: string | null;
  expects_reply: boolean;
  reply_by?: string | null;
  sync_cursor: number;
  created_at: string;
}

export interface MessagingAgent {
  agent_id: string;
  display_name: string;
  principal_kind: PrincipalKind | string;
  delivery_mode: DeliveryMode | string;
  online: boolean;
  last_seen_at?: string | null;
  lease_expires_at?: string | null;
  archived: boolean;
  credential_names?: string[];
}

export interface MessagingSyncData {
  status: "complete" | "timeout" | string;
  cursor: number;
  resume_cursor?: number | null;
  has_more: boolean;
  messages: MessagingMessage[];
  conversations: MessagingConversation[];
  presence: MessagingAgent[];
  unread: Record<string, number>;
  as_of: string;
}

export interface MessagingAgentListData {
  agents: MessagingAgent[];
  as_of: string;
}

export interface CreateConversationData {
  conversation_id: string;
  conversation: MessagingConversation;
  duplicate: boolean;
}

export interface SendMessageData {
  conversation_id: string;
  seq: number;
  message: MessagingMessage;
  duplicate: boolean;
  continuation_id?: string | null;
}

export interface ReadConversationData {
  conversation_id: string;
  last_read_seq: number;
  cursor: number;
  duplicate: boolean;
}

export interface MessagingAgentMutationData {
  agent: MessagingAgent;
}

export interface CredentialBindingData {
  agent_id: string;
  credential_id?: string | null;
  bound: boolean;
}

export interface SendMessageInput {
  client_key: string;
  kind: "text" | "question";
  body_md: string;
  refs: MessagingRef[];
  in_reply_to?: number;
  correlation_id?: string;
  expects_reply: boolean;
  reply_by?: string;
}

export interface CreateAgentInput {
  agent_id: string;
  display_name: string;
  principal_kind: "resident" | "task-time";
  delivery_mode: DeliveryMode;
}

export interface UpdateAgentInput {
  display_name?: string;
  delivery_mode?: DeliveryMode;
  archived?: boolean;
}
