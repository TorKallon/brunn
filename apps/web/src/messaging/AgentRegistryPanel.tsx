import { useMutation } from "@tanstack/react-query";
import { Archive, Plus, Save } from "lucide-react";
import { type FormEvent, useState } from "react";
import type { CredentialSummary } from "../lib/types";
import { formatRelative } from "../lib/format";
import { genericMessagingError } from "./model";
import type {
  CreateAgentInput,
  DeliveryMode,
  MessagingAgent,
  UpdateAgentInput,
} from "./types";

const KEEP_BINDING = "__keep__";
const REMOVE_BINDING = "__remove__";

function AgentRegistryRow({
  agent,
  credentials,
  canManage,
  onUpdate,
  onBind,
}: {
  agent: MessagingAgent;
  credentials: CredentialSummary[];
  canManage: boolean;
  onUpdate: (agentId: string, input: UpdateAgentInput) => Promise<void>;
  onBind: (agentId: string, credentialId: string | null) => Promise<void>;
}) {
  const [deliveryMode, setDeliveryMode] = useState<DeliveryMode>(
    agent.delivery_mode as DeliveryMode,
  );
  const [credentialId, setCredentialId] = useState(KEEP_BINDING);
  const updateMutation = useMutation({
    mutationFn: (input: UpdateAgentInput) => onUpdate(agent.agent_id, input),
  });
  const bindingMutation = useMutation({
    mutationFn: (nextCredentialId: string | null) =>
      onBind(agent.agent_id, nextCredentialId),
    onSuccess: () => setCredentialId(KEEP_BINDING),
  });
  const error = updateMutation.error ?? bindingMutation.error;

  return (
    <fieldset className="messaging-registry-row" aria-label={`${agent.display_name} settings`}>
      <legend>{agent.display_name}</legend>
      <div className="messaging-agent-identity">
        <span className={`messaging-presence ${agent.online ? "online" : ""}`}>
          {agent.online ? "Online" : "Offline"}
        </span>
        <code>{agent.agent_id}</code>
        <span>{agent.principal_kind.replaceAll("-", " ")}</span>
        <span>
          {agent.last_seen_at ? `Seen ${formatRelative(agent.last_seen_at)}` : "Never seen"}
        </span>
        {agent.archived ? <span className="messaging-badge">Archived</span> : null}
      </div>
      {agent.credential_names?.length ? (
        <p className="messaging-binding-names">
          Bound: {agent.credential_names.join(", ")}
        </p>
      ) : (
        <p className="messaging-binding-names">No credential bound</p>
      )}
      {canManage && !agent.archived ? (
        <div className="messaging-registry-controls">
          <label className="field">
            <span>Delivery</span>
            <select
              value={deliveryMode}
              onChange={(event) => setDeliveryMode(event.target.value as DeliveryMode)}
              disabled={updateMutation.isPending}
            >
              <option value="pull">Pull</option>
              <option value="apns">APNs</option>
              <option value="webhook">Webhook (reserved)</option>
            </select>
          </label>
          <button
            className="button secondary"
            type="button"
            disabled={
              updateMutation.isPending || deliveryMode === agent.delivery_mode
            }
            onClick={() => updateMutation.mutate({ delivery_mode: deliveryMode })}
          >
            <Save size={15} aria-hidden="true" />
            Save delivery
          </button>
          <label className="field">
            <span>Credential</span>
            <select
              value={credentialId}
              onChange={(event) => setCredentialId(event.target.value)}
              disabled={bindingMutation.isPending}
            >
              <option value={KEEP_BINDING}>Keep current binding</option>
              <option value={REMOVE_BINDING}>Remove all bindings</option>
              {credentials
                .filter((credential) => !credential.revoked_at)
                .map((credential) => (
                  <option value={credential.id} key={credential.id}>
                    {credential.name}
                  </option>
                ))}
            </select>
          </label>
          <button
            className="button secondary"
            type="button"
            disabled={bindingMutation.isPending || credentialId === KEEP_BINDING}
            onClick={() =>
              bindingMutation.mutate(
                credentialId === REMOVE_BINDING ? null : credentialId,
              )
            }
          >
            Apply binding
          </button>
          {agent.principal_kind !== "owner" ? (
            <button
              className="button danger"
              type="button"
              disabled={updateMutation.isPending}
              onClick={() => updateMutation.mutate({ archived: true })}
            >
              <Archive size={15} aria-hidden="true" />
              Archive
            </button>
          ) : null}
        </div>
      ) : null}
      {error ? (
        <p className="messaging-error" role="alert">
          {genericMessagingError(error)}
        </p>
      ) : null}
    </fieldset>
  );
}

export function AgentRegistryPanel({
  agents,
  credentials,
  canManage,
  loading,
  error,
  onCreate,
  onUpdate,
  onBind,
}: {
  agents: MessagingAgent[];
  credentials: CredentialSummary[];
  canManage: boolean;
  loading: boolean;
  error: unknown;
  onCreate: (input: CreateAgentInput) => Promise<void>;
  onUpdate: (agentId: string, input: UpdateAgentInput) => Promise<void>;
  onBind: (agentId: string, credentialId: string | null) => Promise<void>;
}) {
  const [agentId, setAgentId] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [principalKind, setPrincipalKind] = useState<"resident" | "task-time">(
    "resident",
  );
  const [deliveryMode, setDeliveryMode] = useState<DeliveryMode>("pull");
  const createMutation = useMutation({
    mutationFn: onCreate,
    onSuccess: () => {
      setAgentId("");
      setDisplayName("");
      setPrincipalKind("resident");
      setDeliveryMode("pull");
    },
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!agentId.trim() || !displayName.trim() || createMutation.isPending) return;
    createMutation.mutate({
      agent_id: agentId.trim(),
      display_name: displayName.trim(),
      principal_kind: principalKind,
      delivery_mode: deliveryMode,
    });
  }

  return (
    <details className="messaging-registry">
      <summary>Registry settings</summary>
      <div className="messaging-registry-body">
        <header>
          <div>
            <h2>Agent registry</h2>
            <p>Presence is a lease. Credential values are never shown here.</p>
          </div>
          {!canManage ? (
            <span className="messaging-badge">View only</span>
          ) : null}
        </header>
        {error ? (
          <p className="messaging-error" role="alert">
            {genericMessagingError(error)}
          </p>
        ) : null}
        {loading ? <p className="messaging-inline-state">Loading registry…</p> : null}
        <div className="messaging-registry-list">
          {agents.map((agent) => (
            <AgentRegistryRow
              agent={agent}
              credentials={credentials}
              canManage={canManage}
              onUpdate={onUpdate}
              onBind={onBind}
              key={agent.agent_id}
            />
          ))}
        </div>
        {canManage ? (
          <form className="messaging-create-agent" onSubmit={submit}>
            <h3>Register agent</h3>
            <label className="field">
              <span>Principal ID</span>
              <input
                value={agentId}
                onChange={(event) => setAgentId(event.target.value)}
                placeholder="echo"
                pattern="[a-z0-9]+(?:[._-][a-z0-9]+)*"
                maxLength={80}
                required
              />
            </label>
            <label className="field">
              <span>Display name</span>
              <input
                value={displayName}
                onChange={(event) => setDisplayName(event.target.value)}
                maxLength={120}
                required
              />
            </label>
            <label className="field">
              <span>Kind</span>
              <select
                value={principalKind}
                onChange={(event) =>
                  setPrincipalKind(event.target.value as "resident" | "task-time")
                }
              >
                <option value="resident">Resident</option>
                <option value="task-time">Task-time</option>
              </select>
            </label>
            <label className="field">
              <span>Delivery</span>
              <select
                value={deliveryMode}
                onChange={(event) => setDeliveryMode(event.target.value as DeliveryMode)}
              >
                <option value="pull">Pull</option>
                <option value="apns">APNs</option>
                <option value="webhook">Webhook (reserved)</option>
              </select>
            </label>
            <button
              className="button primary"
              type="submit"
              disabled={!agentId.trim() || !displayName.trim() || createMutation.isPending}
            >
              <Plus size={16} aria-hidden="true" />
              {createMutation.isPending ? "Registering…" : "Register agent"}
            </button>
            {createMutation.isError ? (
              <p className="messaging-error" role="alert">
                {genericMessagingError(createMutation.error)}
              </p>
            ) : null}
          </form>
        ) : null}
      </div>
    </details>
  );
}
