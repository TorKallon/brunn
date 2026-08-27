import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Archive,
  ArrowRightLeft,
  CloudOff,
  KeyRound,
  Plus,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
import { type FormEvent, useState } from "react";
import { Section } from "./Page";
import { ErrorState, LoadingState, StatusBadge } from "./StateViews";
import { useApi } from "../lib/auth";
import { useCapability } from "../lib/current";
import { formatDate, humanize } from "../lib/format";
import type {
  TaskContext,
  TaskContextListData,
  TaskGuardStatusData,
  TaskSettings,
  TodoistStatusData,
} from "../lib/types";
import { newOperationId } from "../lib/workspace";

export function TaskSettingsPanels() {
  const api = useApi();
  const canRead = useCapability("task.read");
  const canWrite = useCapability("task.write");
  const canManageIntegrations = useCapability("integration.manage");
  const contextsQuery = useQuery({
    queryKey: ["task-contexts", "settings"],
    queryFn: () => api.taskContexts(true),
    enabled: canRead,
  });
  const settingsQuery = useQuery({
    queryKey: ["task-settings"],
    queryFn: () => api.taskSettings(),
    enabled: canRead,
  });
  const todoistQuery = useQuery({
    queryKey: ["todoist-status"],
    queryFn: () => api.todoistStatus(),
    enabled: canRead,
    refetchInterval: 60_000,
  });
  const guardQuery = useQuery({
    queryKey: ["task-guard-status"],
    queryFn: () => api.taskGuardStatus(),
    enabled: canRead,
    refetchInterval: 60_000,
  });

  if (!canRead) return null;
  const anyPending =
    contextsQuery.isPending ||
    settingsQuery.isPending ||
    todoistQuery.isPending ||
    guardQuery.isPending;
  const error =
    contextsQuery.error ?? settingsQuery.error ?? todoistQuery.error ?? guardQuery.error;
  return (
    <>
      {!canWrite ? (
        <p className="readonly-notice task-settings-boundary" role="status">
          Task actions are view only
        </p>
      ) : null}
      {anyPending ? <LoadingState label="Loading task settings" /> : null}
      {error ? <ErrorState error={error} title="Unable to load task settings" /> : null}
      {contextsQuery.data ? (
        <ContextSettings
          key={`contexts-${contextsQuery.data.data.surface_defaults.web?.version ?? 0}`}
          data={contextsQuery.data.data}
          canWrite={canWrite}
        />
      ) : null}
      {settingsQuery.data ? (
        <EngineSettings
          key={`engine-${settingsQuery.data.data.settings.version}`}
          settings={settingsQuery.data.data.settings}
          canWrite={canWrite}
        />
      ) : null}
      {todoistQuery.data ? (
        <TodoistSettings
          key={`todoist-${todoistQuery.data.data.configuration_generation}`}
          status={todoistQuery.data.data}
          canManage={canManageIntegrations}
        />
      ) : null}
      {settingsQuery.data && todoistQuery.data && guardQuery.data ? (
        <OperationalPanel
          settings={settingsQuery.data.data.settings}
          todoist={todoistQuery.data.data}
          guard={guardQuery.data.data}
        />
      ) : null}
    </>
  );
}

function ContextSettings({ data, canWrite }: { data: TaskContextListData; canWrite: boolean }) {
  const api = useApi();
  const queryClient = useQueryClient();
  const active = data.contexts.filter((context) => !context.archived);
  const [from, setFrom] = useState(active[0]?.slug ?? "");
  const [into, setInto] = useState(active.find((context) => context.slug !== from)?.slug ?? "");
  const [defaults, setDefaults] = useState(
    () => data.surface_defaults.web?.contexts_available ?? [],
  );
  const [newName, setNewName] = useState("");
  const [contextMessage, setContextMessage] = useState<string | null>(null);
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["task-contexts"] });
  const mergeMutation = useMutation({
    mutationFn: () => {
      const source = data.contexts.find((context) => context.slug === from);
      const target = data.contexts.find((context) => context.slug === into);
      if (!source || !target) throw new Error("Choose two active contexts.");
      return api.taskContextsMerge({
        from,
        into,
        expected_from_version: source.version,
        expected_into_version: target.version,
        source: "owner",
        reason: "owner merge from Web settings",
        idempotency_key: newOperationId("web_context_merge"),
      });
    },
    onSuccess: refresh,
  });
  const archiveMutation = useMutation({
    mutationFn: (context: TaskContext) =>
      api.taskContextArchive(context.slug, {
        archived: !context.archived,
        expected_version: context.version,
        source: "owner",
        idempotency_key: newOperationId("web_context_archive"),
      }),
    onSuccess: refresh,
  });
  const defaultsMutation = useMutation({
    mutationFn: () =>
      api.taskContextsSetAvailable("web", {
        contexts_available: defaults,
        expected_version: data.surface_defaults.web?.version ?? 0,
        source: "owner",
        idempotency_key: newOperationId("web_context_defaults"),
      }),
    onSuccess: refresh,
  });
  const createMutation = useMutation({
    mutationFn: () =>
      api.taskContextCreate({
        display_name: newName.trim(),
        aliases: [],
        source: "owner",
        confirm_new: false,
        idempotency_key: newOperationId("web_context_create"),
      }),
    onSuccess: (response) => {
      setContextMessage(
        response.status === "needs_review"
          ? "A similar context exists. Review the suggestion before confirming a new one."
          : "Context created.",
      );
      if (response.status !== "needs_review") setNewName("");
      void refresh();
    },
  });
  const mutationError = mergeMutation.error ?? archiveMutation.error ?? defaultsMutation.error ?? createMutation.error;
  function submitNew(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (newName.trim()) createMutation.mutate();
  }
  return (
    <Section title="Contexts" meta="Dynamic, agent-mintable, never auto-merged">
      <div className="context-settings-list">
        {data.contexts.map((context) => (
          <article className={context.archived ? "is-archived" : ""} key={context.slug}>
            <span>
              <strong>{context.display_name}</strong>
              <code>{context.slug}</code>
            </span>
            <span className="context-aliases">
              {context.aliases.length ? context.aliases.map((alias) => <code key={alias}>{alias}</code>) : "No aliases"}
            </span>
            <span>{context.active_task_count} active</span>
            {isArchiveSuggestion(context) ? (
              <span className="status-badge muted">Suggested archive</span>
            ) : null}
            {canWrite ? (
              <button
                className="button secondary"
                type="button"
                disabled={archiveMutation.isPending}
                onClick={() => archiveMutation.mutate(context)}
              >
                <Archive size={15} aria-hidden="true" />
                {context.archived ? "Restore" : "Archive"}
              </button>
            ) : null}
          </article>
        ))}
      </div>
      {canWrite ? (
        <>
          <form className="context-create-form" onSubmit={submitNew}>
            <label>
              <span>New context</span>
              <input value={newName} onChange={(event) => setNewName(event.target.value)} placeholder="for example, office" maxLength={120} />
            </label>
            <button className="button secondary" type="submit" disabled={!newName.trim() || createMutation.isPending}>
              <Plus size={15} aria-hidden="true" />
              Create context
            </button>
          </form>
          <fieldset className="context-merge-form">
            <legend>Merge contexts</legend>
            <label>
              <span>Merge from</span>
              <select value={from} onChange={(event) => setFrom(event.target.value)}>
                {active.map((context) => <option key={context.slug} value={context.slug}>{context.display_name}</option>)}
              </select>
            </label>
            <ArrowRightLeft size={17} aria-hidden="true" />
            <label>
              <span>Merge into</span>
              <select value={into} onChange={(event) => setInto(event.target.value)}>
                {active.map((context) => <option key={context.slug} value={context.slug}>{context.display_name}</option>)}
              </select>
            </label>
            <button className="button secondary" type="button" disabled={!from || !into || from === into || mergeMutation.isPending} onClick={() => mergeMutation.mutate()}>
              Merge contexts
            </button>
          </fieldset>
          <fieldset className="context-defaults">
            <legend>Web defaults</legend>
            <div>
              {active.map((context) => (
                <label key={context.slug}>
                  <input
                    type="checkbox"
                    checked={defaults.includes(context.slug)}
                    onChange={(event) =>
                      setDefaults((values) =>
                        event.target.checked
                          ? [...new Set([...values, context.slug])]
                          : values.filter((value) => value !== context.slug),
                      )
                    }
                  />
                  {context.display_name}
                </label>
              ))}
            </div>
            <button className="button secondary" type="button" disabled={defaultsMutation.isPending} onClick={() => defaultsMutation.mutate()}>
              Save Web contexts
            </button>
          </fieldset>
        </>
      ) : null}
      {contextMessage ? <p className="settings-note" role="status">{contextMessage}</p> : null}
      {mutationError ? <ErrorState error={mutationError} title="Context change failed" /> : null}
    </Section>
  );
}

function EngineSettings({ settings, canWrite }: { settings: TaskSettings; canWrite: boolean }) {
  const api = useApi();
  const queryClient = useQueryClient();
  const [hardLead, setHardLead] = useState(settings.hard_lead_days);
  const [secondLead, setSecondLead] = useState(settings.hard_second_lead_hours);
  const [dueDayTime, setDueDayTime] = useState(settings.due_day_local_time.slice(0, 5));
  const [softWindow, setSoftWindow] = useState(settings.soft_window_days);
  const [quietStart, setQuietStart] = useState(settings.quiet_hours_start.slice(0, 5));
  const [quietEnd, setQuietEnd] = useState(settings.quiet_hours_end.slice(0, 5));
  const [override, setOverride] = useState(settings.quiet_override_enabled);
  const [overrideHours, setOverrideHours] = useState(settings.quiet_override_within_hours);
  const mutation = useMutation({
    mutationFn: () =>
      api.taskSettingsUpdate({
        expected_version: settings.version,
        idempotency_key: newOperationId("web_task_settings"),
        timezone: settings.timezone,
        hard_lead_days: hardLead,
        hard_second_lead_hours: secondLead,
        due_day_local_time: dueDayTime,
        soft_window_days: softWindow,
        quiet_hours_start: quietStart,
        quiet_hours_end: quietEnd,
        quiet_override_enabled: override,
        quiet_override_within_hours: overrideHours,
      }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["task-settings"] }),
  });
  return (
    <Section title="Readiness engine" meta="Deterministic windows and guard policy">
      <form className="engine-settings-form" onSubmit={(event) => { event.preventDefault(); if (canWrite) mutation.mutate(); }}>
        <label><span>Hard deadline window (days)</span><input type="number" min={1} max={90} value={hardLead} onChange={(event) => setHardLead(Number(event.target.value))} disabled={!canWrite} /></label>
        <label><span>Second hard lead (hours)</span><input type="number" min={1} max={2160} value={secondLead} onChange={(event) => setSecondLead(Number(event.target.value))} disabled={!canWrite} /></label>
        <label><span>Due-day notification time</span><input type="time" value={dueDayTime} onChange={(event) => setDueDayTime(event.target.value)} disabled={!canWrite} /></label>
        <label><span>Soft due window (days)</span><input type="number" min={1} max={90} value={softWindow} onChange={(event) => setSoftWindow(Number(event.target.value))} disabled={!canWrite} /></label>
        <label><span>Quiet hours start</span><input type="time" value={quietStart} onChange={(event) => setQuietStart(event.target.value)} disabled={!canWrite} /></label>
        <label><span>Quiet hours end</span><input type="time" value={quietEnd} onChange={(event) => setQuietEnd(event.target.value)} disabled={!canWrite} /></label>
        <label><span>Quiet override threshold (hours)</span><input type="number" min={1} max={168} value={overrideHours} onChange={(event) => setOverrideHours(Number(event.target.value))} disabled={!canWrite || !override} /></label>
        <label className="task-filter-check"><input type="checkbox" checked={override} onChange={(event) => setOverride(event.target.checked)} disabled={!canWrite} />Allow confirmed hard deadlines inside the threshold to break quiet hours</label>
        {canWrite ? <button className="button primary" type="submit" disabled={mutation.isPending}>{mutation.isPending ? "Saving…" : "Save engine settings"}</button> : null}
      </form>
      {mutation.isSuccess ? <p className="task-feedback" role="status"><ShieldCheck size={15} aria-hidden="true" />Engine settings saved</p> : null}
      {mutation.isError ? <ErrorState error={mutation.error} title="Engine settings failed" /> : null}
    </Section>
  );
}

function TodoistSettings({ status, canManage }: { status: TodoistStatusData; canManage: boolean }) {
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
    <Section title="Todoist inlet" meta="Optional, one-way pull only">
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

function OperationalPanel({
  settings,
  todoist,
  guard,
}: {
  settings: TaskSettings;
  todoist: TodoistStatusData;
  guard: TaskGuardStatusData;
}) {
  return (
    <Section title="Task operations" meta="Content-free status only">
      <div className="task-operations-grid">
        <article>
          <ShieldCheck size={18} aria-hidden="true" />
          <span><strong>Deadline guard</strong><small>{settings.hard_lead_days}d · {settings.hard_second_lead_hours}h · {settings.due_day_local_time.slice(0, 5)} local</small></span>
          <span>
            <StatusBadge status={guard.last_outcome ?? (guard.effective_enabled ? "scheduled" : "off")} />
            <small>Environment {guard.environment_enabled ? "enabled" : "off"} · Effective {guard.effective_enabled ? "enabled" : "off"}</small>
            <small>{guard.last_run_at ? `Last ${formatDate(guard.last_run_at)}` : "No run recorded"}</small>
            <small>{guard.next_run_at ? `Next ${formatDate(guard.next_run_at)}` : "No next run scheduled"}</small>
            {guard.last_error_code ? <code>{guard.last_error_code}</code> : null}
          </span>
        </article>
        <article>
          <KeyRound size={18} aria-hidden="true" />
          <span><strong>Todoist pull</strong><small>Effective mode {humanize(todoist.effective_mode)}</small></span>
          <span><StatusBadge status={todoist.last_outcome ?? "not run"} /><small>{todoist.last_run_at ? formatDate(todoist.last_run_at) : "No run recorded"}</small>{todoist.last_error_code ? <code>{todoist.last_error_code}</code> : null}</span>
        </article>
      </div>
    </Section>
  );
}

function isArchiveSuggestion(context: TaskContext): boolean {
  if (context.archived || context.active_task_count > 0) return false;
  const updatedAt = Date.parse(context.updated_at);
  return Number.isFinite(updatedAt) && Date.now() - updatedAt >= 90 * 24 * 60 * 60 * 1000;
}
