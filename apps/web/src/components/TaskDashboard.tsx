import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  ArrowRight,
  CheckCircle2,
  ClipboardCheck,
  FolderKanban,
  Inbox,
  Plus,
  Sparkles,
} from "lucide-react";
import { type FormEvent, useState } from "react";
import { useApi } from "../lib/auth";
import { useCapability } from "../lib/current";
import { taskQuickOperation, type TaskQuickAction } from "../lib/taskOperations";
import type { TaskCandidate } from "../lib/types";
import { newOperationId } from "../lib/workspace";
import { ErrorState, LoadingState } from "./StateViews";
import { TaskRow } from "./TaskRow";

const DEFAULT_WEB_CONTEXTS = ["online"];

export function TaskDashboard() {
  const api = useApi();
  const queryClient = useQueryClient();
  const canRead = useCapability("task.read");
  const canWrite = useCapability("task.write");
  const [nextLimit, setNextLimit] = useState(5);
  const [doneExpanded, setDoneExpanded] = useState(false);
  const [feedback, setFeedback] = useState<string | null>(null);

  const contextsQuery = useQuery({
    queryKey: ["task-contexts"],
    queryFn: () => api.taskContexts(),
    enabled: canRead,
  });
  const availableContexts =
    contextsQuery.data?.data.surface_defaults.web?.contexts_available ??
    DEFAULT_WEB_CONTEXTS;
  const contextKey = availableContexts.join(",");
  const urgentQuery = useQuery({
    queryKey: ["task-candidates", "urgent", contextKey],
    queryFn: () =>
      api.taskCandidates({
        view: "urgent",
        contexts_available: availableContexts,
      }),
    enabled: canRead,
    refetchInterval: 60_000,
  });
  const nextQuery = useQuery({
    queryKey: ["task-candidates", "next", nextLimit, contextKey],
    queryFn: () =>
      api.taskCandidates({
        view: "next",
        limit: nextLimit,
        contexts_available: availableContexts,
      }),
    enabled: canRead,
    refetchInterval: 60_000,
  });
  const doneQuery = useQuery({
    queryKey: ["task-done", "today"],
    queryFn: () => api.taskDoneSummary({ limit: 5 }),
    enabled: canRead,
    refetchInterval: 60_000,
  });
  const projectsQuery = useQuery({
    queryKey: ["task-projects"],
    queryFn: () => api.taskProjects(),
    enabled: canRead,
    refetchInterval: 60_000,
  });
  const actionMutation = useMutation({
    mutationFn: ({ item, action }: { item: TaskCandidate; action: TaskQuickAction }) =>
      api.taskUpdate(item.task_ref, {
        expected_version: item.version,
        idempotency_key: newOperationId(`web_task_${action}`),
        operation: taskQuickOperation(action),
      }),
    onSuccess: (response) => {
      const count = response.data.done_today_count;
      setFeedback(
        response.data.action === "complete"
          ? `Task complete${count === null || count === undefined ? "" : ` · ${count} done today`}`
          : response.data.action === "snooze"
            ? "Task snoozed"
            : response.data.action === "confirm_hard"
              ? "Hard deadline confirmed"
              : response.data.action === "drop"
                ? "Task dropped"
                : "Task reopened",
      );
      void invalidateTaskReads(queryClient);
    },
  });

  if (!canRead) return null;
  const contextsReady = contextsQuery.isSuccess;
  const urgentItems = contextsReady ? (urgentQuery.data?.data.items ?? []) : [];
  const nextItems = contextsReady ? (nextQuery.data?.data.items ?? []) : [];
  const visibleTasks = selectDashboardTasks(urgentItems, nextItems, nextLimit);
  const urgent = visibleTasks.urgent;
  const next = visibleTasks.next;
  const done = (doneQuery.data?.data.items ?? []).slice(0, 5);
  const projects = (projectsQuery.data?.data.projects ?? []).slice(0, 5);
  const taskError =
    contextsQuery.error ??
    urgentQuery.error ??
    nextQuery.error ??
    doneQuery.error ??
    projectsQuery.error;

  return (
    <section className="dashboard-section task-dashboard" aria-labelledby="tasks-heading">
      <div className="dashboard-section-heading">
        <div>
          <span className="dashboard-eyebrow">Today</span>
          <h2 id="tasks-heading">What needs your attention</h2>
        </div>
        <span>{availableContexts.join(" · ") || "Anywhere"}</span>
      </div>

      <TaskCapture canWrite={canWrite} />
      {feedback ? (
        <p className="task-feedback" role="status">
          <CheckCircle2 size={16} aria-hidden="true" />
          {feedback}
        </p>
      ) : null}
      {actionMutation.isError ? (
        <ErrorState error={actionMutation.error} title="Task action failed" />
      ) : null}
      {contextsQuery.isPending || urgentQuery.isPending || nextQuery.isPending ? (
        <LoadingState label="Loading today’s tasks" />
      ) : null}
      {taskError ? (
        <ErrorState
          error={taskError}
          title="Unable to load tasks"
          retry={() => void invalidateTaskReads(queryClient)}
        />
      ) : null}

      {contextsReady && !urgentQuery.isPending && !urgentQuery.isError && urgent.length === 0 ? (
        <p className="task-nothing-urgent" role="status">
          <Sparkles size={16} aria-hidden="true" />
          Nothing urgent
        </p>
      ) : null}
      {urgent.length ? (
        <section className="task-card task-card-urgent" aria-label="Urgent tasks">
          <header>
            <div>
              <span>Urgent</span>
              <strong>{urgentQuery.data?.data.urgent_total ?? urgent.length}</strong>
            </div>
            <small>Hard deadlines and active cost only</small>
          </header>
          <div className="task-list">
            {urgent.map((item) => (
              <TaskRow
                key={item.task_ref}
                item={item}
                canWrite={canWrite}
                pending={actionMutation.isPending}
                onAction={(task, action) => actionMutation.mutate({ item: task, action })}
              />
            ))}
          </div>
          {(urgentQuery.data?.data.urgent_total ?? 0) > urgent.length ? (
            <p className="task-card-note">
              {(urgentQuery.data?.data.urgent_total ?? 0) - urgent.length} more urgent
              task(s) held behind Show all.
            </p>
          ) : null}
        </section>
      ) : null}

      <div className="task-dashboard-grid">
        <section className="task-card task-card-next" aria-label="Next tasks">
          <header>
            <div>
              <span>Next {nextLimit}</span>
              <strong>{next.length}</strong>
            </div>
            <small>Ranked now, no model wait</small>
          </header>
          <div className="task-list">
            {next.map((item) => (
              <TaskRow
                key={item.task_ref}
                item={item}
                canWrite={canWrite}
                pending={actionMutation.isPending}
                onAction={(task, action) => actionMutation.mutate({ item: task, action })}
              />
            ))}
            {!nextQuery.isPending && next.length === 0 ? (
              <div className="task-card-empty">
                <Inbox size={18} aria-hidden="true" />
                Nothing ready in these contexts
              </div>
            ) : null}
          </div>
          <footer>
            {nextLimit === 5 &&
            ((nextQuery.data?.data.next_remaining ?? 0) > 0 || visibleTasks.hidden) ? (
              <button className="button secondary" type="button" onClick={() => setNextLimit(10)}>
                <Plus size={15} aria-hidden="true" />
                5 more
              </button>
            ) : null}
            <Link className="button secondary" to="/tasks">
              Show all
              <ArrowRight size={15} aria-hidden="true" />
            </Link>
            <span>{nextQuery.data?.data.backlog_total ?? 0} tasks held out of view</span>
          </footer>
        </section>

        <div className="task-dashboard-side">
          <section className="task-card task-card-done" aria-label="Done today">
            <header>
              <div>
                <span>Done today</span>
                <strong>{doneQuery.data?.data.done_today_count ?? 0}</strong>
              </div>
              <ClipboardCheck size={19} aria-hidden="true" />
            </header>
            {done.length ? (
              <button
                className="task-done-toggle"
                type="button"
                aria-expanded={doneExpanded}
                onClick={() => setDoneExpanded((expanded) => !expanded)}
              >
                {doneExpanded ? "Hide completed tasks" : "Show completed tasks"}
              </button>
            ) : (
              <p className="task-card-empty">Your first checkmark lands here.</p>
            )}
            {doneExpanded && done.length ? (
              <ol className="done-list">
                {done.map((item) => <li key={item.task_ref}>{item.title}</li>)}
              </ol>
            ) : null}
          </section>

          <section className="task-card task-card-projects" aria-label="Task projects">
            <header>
              <div>
                <span>Projects</span>
                <strong>{projects.length}</strong>
              </div>
              <FolderKanban size={19} aria-hidden="true" />
            </header>
            <div className="task-project-list">
              {projects.map((project) => (
                <Link
                  key={project.slug}
                  to="/projects/$slug"
                  params={{ slug: project.slug }}
                >
                  <span>
                    <strong>{project.title}</strong>
                    <small>{project.open_task_count} open</small>
                  </span>
                  <span className={`project-interest interest-${project.interest}`}>
                    {project.interest}
                  </span>
                </Link>
              ))}
            </div>
          </section>
        </div>
      </div>
    </section>
  );
}

function TaskCapture({ canWrite }: { canWrite: boolean }) {
  const api = useApi();
  const queryClient = useQueryClient();
  const [rawText, setRawText] = useState("");
  const mutation = useMutation({
    mutationFn: () =>
      api.taskCapture({
        idempotency_key: newOperationId("web_task_capture"),
        items: [{ raw_text: rawText.trim(), captured_from: "web:dashboard" }],
      }),
    onSuccess: () => {
      setRawText("");
      void invalidateTaskReads(queryClient);
    },
  });
  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (canWrite && rawText.trim()) mutation.mutate();
  }
  return (
    <form className="task-capture" onSubmit={submit}>
      <label htmlFor="task-capture-text">Capture a task</label>
      <input
        id="task-capture-text"
        value={rawText}
        onChange={(event) => setRawText(event.target.value)}
        placeholder="One sentence; agents can enrich it later"
        maxLength={20_000}
        disabled={!canWrite || mutation.isPending}
      />
      <button
        className="button primary"
        type="submit"
        disabled={!canWrite || !rawText.trim() || mutation.isPending}
      >
        {mutation.isPending ? "Capturing…" : "Capture"}
      </button>
      {!canWrite ? <span>View-only task access</span> : null}
      {mutation.isError ? <span className="field-error">Capture failed</span> : null}
    </form>
  );
}

function selectDashboardTasks(
  urgentItems: TaskCandidate[],
  nextItems: TaskCandidate[],
  limit: number,
): { urgent: TaskCandidate[]; next: TaskCandidate[]; hidden: boolean } {
  const urgent = uniqueTasks(urgentItems);
  const urgentRefs = new Set(urgent.map((item) => item.task_ref));
  const next = uniqueTasks(nextItems).filter((item) => !urgentRefs.has(item.task_ref));
  const combined = [...urgent, ...next];
  const selected =
    limit === 5
      ? selectDefaultDashboardTasks(combined)
      : combined.slice(0, Math.min(limit, 10));
  const selectedRefs = new Set(selected.map((item) => item.task_ref));
  return {
    urgent: urgent.filter((item) => selectedRefs.has(item.task_ref)),
    next: next.filter((item) => selectedRefs.has(item.task_ref)),
    hidden: selected.length < combined.length,
  };
}

function selectDefaultDashboardTasks(items: TaskCandidate[]): TaskCandidate[] {
  const pinnedRefs = new Set(
    items.filter((item) => item.pinned).slice(0, 2).map((item) => item.task_ref),
  );
  const unpinnedRefs = new Set(
    items.filter((item) => !item.pinned).slice(0, 5).map((item) => item.task_ref),
  );
  return items.filter(
    (item) => pinnedRefs.has(item.task_ref) || unpinnedRefs.has(item.task_ref),
  );
}

function uniqueTasks(items: TaskCandidate[]): TaskCandidate[] {
  const seen = new Set<string>();
  return items.filter((item) => {
    if (seen.has(item.task_ref)) return false;
    seen.add(item.task_ref);
    return true;
  });
}

async function invalidateTaskReads(
  queryClient: ReturnType<typeof useQueryClient>,
): Promise<void> {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: ["task-candidates"] }),
    queryClient.invalidateQueries({ queryKey: ["task-done"] }),
    queryClient.invalidateQueries({ queryKey: ["task-projects"] }),
  ]);
}
