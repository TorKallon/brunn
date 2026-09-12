import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CloudOff, RefreshCw } from "lucide-react";
import { useState } from "react";
import { Section } from "./Page";
import { ErrorState, LoadingState } from "./StateViews";
import { useApi } from "../lib/auth";
import { useCapability } from "../lib/current";
import { humanize } from "../lib/format";
import type { TodoistStatusData } from "../lib/types";
import { newOperationId } from "../lib/workspace";

export function TodoistSettings() {
  const api = useApi();
  const canRead = useCapability("task.read");
  const canManage = useCapability("integration.manage");
  const todoistQuery = useQuery({
    queryKey: ["todoist-status"],
    queryFn: () => api.todoistStatus(),
    enabled: canRead,
    refetchInterval: 60_000,
  });

  if (!canRead) return null;
  return (
    <>
      {todoistQuery.isPending ? <LoadingState label="Loading Todoist status" /> : null}
      {todoistQuery.error ? <ErrorState error={todoistQuery.error} title="Unable to load Todoist status" /> : null}
      {todoistQuery.data ? (
        <TodoistPanel
          key={`todoist-${todoistQuery.data.data.configuration_generation}`}
          status={todoistQuery.data.data}
          canManage={canManage}
        />
      ) : null}
    </>
  );
}

function TodoistPanel({ status, canManage }: { status: TodoistStatusData; canManage: boolean }) {
  const api = useApi();
  const queryClient = useQueryClient();
  const [mode, setMode] = useState(status.saved_mode);
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["todoist-status"] });
  const configure = useMutation({
    mutationFn: () => api.todoistConfigure({ expected_generation: status.configuration_generation, idempotency_key: newOperationId("web_todoist_mode"), mode }),
    onSuccess: refresh,
  });
  const pull = useMutation({
    mutationFn: () => api.todoistPull({ idempotency_key: newOperationId("web_todoist_pull") }),
    onSuccess: refresh,
  });
  return (
    <Section title="Todoist" meta="Optional, one-way pull only">
      <div className="todoist-truth-grid">
        <div><span>Saved mode</span><strong>{humanize(status.saved_mode)}</strong></div>
        <div><span>Effective mode</span><strong>{humanize(status.effective_mode)}</strong></div>
        <div><span>Environment gate</span><strong>{status.environment_enabled ? "Enabled" : "Off"}</strong></div>
        <div><span>Credential</span><strong>{status.token_configured ? "Available" : "Missing"}</strong></div>
      </div>
      {!status.environment_enabled ? <p className="todoist-kill-switch" role="status"><CloudOff size={16} aria-hidden="true" />Environment kill switch is off</p> : null}
      {!canManage ? (
        <p className="readonly-notice" role="status">Todoist configuration is owner-only</p>
      ) : (
        <div className="todoist-controls">
          <label><span>Todoist mode</span><select value={mode} onChange={(event) => setMode(event.target.value as TodoistStatusData["saved_mode"])}><option value="off">Off</option><option value="import_once">Import once</option><option value="pull">Pull every five minutes</option></select></label>
          <button className="button primary" type="button" disabled={configure.isPending} onClick={() => configure.mutate()}>Save Todoist mode</button>
          <button
            className="button secondary"
            type="button"
            disabled={pull.isPending || status.effective_mode === "off"}
            title={status.effective_mode === "off" ? "Enable the environment gate, credential, and a pull mode first" : undefined}
            onClick={() => pull.mutate()}
          >
            <RefreshCw size={15} aria-hidden="true" />Pull now
          </button>
        </div>
      )}
      {configure.isError || pull.isError ? <ErrorState error={configure.error ?? pull.error} title="Todoist change failed" /> : null}
    </Section>
  );
}
