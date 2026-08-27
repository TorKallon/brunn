import { useMutation } from "@tanstack/react-query";
import { Plus, X } from "lucide-react";
import { type FormEvent, useState } from "react";
import { genericMessagingError } from "./model";
import type { MessagingAgent } from "./types";

export function NewConversationPicker({
  open,
  agents,
  onClose,
  onCreate,
}: {
  open: boolean;
  agents: MessagingAgent[];
  onClose: () => void;
  onCreate: (participants: string[], subject?: string) => Promise<void>;
}) {
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [subject, setSubject] = useState("");
  const candidates = agents.filter(
    (agent) => agent.principal_kind !== "owner" && !agent.archived,
  );
  const createMutation = useMutation({
    mutationFn: () => onCreate(Array.from(selected).sort(), subject.trim() || undefined),
    onSuccess: () => {
      setSelected(new Set());
      setSubject("");
      onClose();
    },
  });

  if (!open) return null;

  function toggle(agentId: string) {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(agentId)) next.delete(agentId);
      else next.add(agentId);
      return next;
    });
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (selected.size > 0 && !createMutation.isPending) createMutation.mutate();
  }

  return (
    <div className="messaging-dialog-backdrop">
      <section
        className="messaging-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-conversation-title"
      >
        <header>
          <div>
            <h2 id="new-conversation-title">New conversation</h2>
            <p>Choose one agent for a direct thread or several for a group.</p>
          </div>
          <button className="icon-button" type="button" onClick={onClose} aria-label="Close">
            <X size={18} aria-hidden="true" />
          </button>
        </header>
        <form onSubmit={submit}>
          <fieldset className="messaging-agent-picker">
            <legend>Participants</legend>
            {candidates.length > 0 ? (
              candidates.map((agent) => (
                <label key={agent.agent_id}>
                  <input
                    type="checkbox"
                    checked={selected.has(agent.agent_id)}
                    onChange={() => toggle(agent.agent_id)}
                  />
                  <span className={`messaging-presence ${agent.online ? "online" : ""}`}>
                    {agent.online ? "Online" : "Offline"}
                  </span>
                  <strong>{agent.display_name}</strong>
                  <code>{agent.agent_id}</code>
                </label>
              ))
            ) : (
              <p>No active agents are registered yet.</p>
            )}
          </fieldset>
          <label className="field">
            <span>Subject</span>
            <input
              value={subject}
              maxLength={240}
              onChange={(event) => setSubject(event.target.value)}
              placeholder="Optional for direct conversations"
            />
          </label>
          {createMutation.isError ? (
            <p className="messaging-error" role="alert">
              {genericMessagingError(createMutation.error)}
            </p>
          ) : null}
          <div className="messaging-dialog-actions">
            <button className="button secondary" type="button" onClick={onClose}>
              Cancel
            </button>
            <button
              className="button primary"
              type="submit"
              disabled={selected.size === 0 || createMutation.isPending}
            >
              <Plus size={16} aria-hidden="true" />
              {createMutation.isPending ? "Creating…" : "Create conversation"}
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}
