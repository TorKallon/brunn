import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import {
  ArrowLeft,
  CalendarClock,
  Check,
  CircleDotDashed,
  Pin,
  RotateCcw,
  ShieldCheck,
  ShieldMinus,
  Trash2,
} from "lucide-react";
import { type FormEvent, useState } from "react";
import { DefinitionList, Page, PageHeader, Section } from "../components/Page";
import { ErrorState, LoadingState, StatusBadge } from "../components/StateViews";
import { useApi } from "../lib/auth";
import { useCapability } from "../lib/current";
import { formatDate, humanize } from "../lib/format";
import type { JsonObject, JsonValue, TaskDetail } from "../lib/types";
import { newOperationId } from "../lib/workspace";

export function TaskDetailPage() {
  const { taskRef } = useParams({ from: "/authenticated/tasks/$taskRef" });
  const api = useApi();
  const queryClient = useQueryClient();
  const canWrite = useCapability("task.write");
  const query = useQuery({
    queryKey: ["task", taskRef],
    queryFn: () => api.taskGet(taskRef),
  });
  const task = query.data?.data.task;
  const mutation = useMutation({
    mutationFn: (operation: JsonObject) => {
      if (!task) throw new Error("The task is not loaded.");
      return api.taskUpdate(task.task_ref, {
        expected_version: task.version,
        idempotency_key: newOperationId(`web_task_${String(operation.type)}`),
        operation,
      });
    },
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["task", taskRef] }),
        queryClient.invalidateQueries({ queryKey: ["task-candidates"] }),
        queryClient.invalidateQueries({ queryKey: ["task-done"] }),
      ]);
    },
  });

  return (
    <Page>
      <PageHeader
        title={task?.title ?? "Task"}
        description={task ? `Updated ${formatDate(task.updated_at)}` : "Loading task detail"}
        actions={
          <Link className="button secondary" to="/tasks">
            <ArrowLeft size={16} aria-hidden="true" />
            All tasks
          </Link>
        }
      />
      {query.isPending ? <LoadingState label="Loading task" /> : null}
      {query.isError ? (
        <ErrorState error={query.error} retry={() => void query.refetch()} title="Unable to load task" />
      ) : null}
      {mutation.isError ? <ErrorState error={mutation.error} title="Task action failed" /> : null}
      {mutation.isSuccess ? (
        <p className="task-feedback" role="status">
          <Check size={16} aria-hidden="true" />
          {humanize(mutation.data.data.action)} saved
        </p>
      ) : null}
      {task ? (
        <>
          <Section
            title="Task state"
            meta={<StatusBadge status={task.status} />}
            className="task-detail-state"
          >
            <DefinitionList items={taskDefinitionItems(task)} />
            {canWrite ? (
              <TaskDetailActions
                task={task}
                pending={mutation.isPending}
                mutate={(operation) => mutation.mutate(operation)}
              />
            ) : (
              <p className="readonly-notice">Task actions are view only</p>
            )}
          </Section>
          <Section title="Provenance" meta="Every enriched field keeps its source">
            <div className="task-provenance-list">
              {taskFieldEntries(task).map(([field, cell]) => (
                <article key={field}>
                  <CircleDotDashed size={16} aria-hidden="true" />
                  <span>
                    <strong>{humanize(field)}</strong>
                    <small>{formatTaskValue(cell.value)}</small>
                  </span>
                  <span>
                    <code>{cell.source}</code>
                    <small>{formatDate(cell.set_at)}</small>
                  </span>
                  {cell.note ? <p>{cell.note}</p> : null}
                </article>
              ))}
              {!taskFieldEntries(task).length ? (
                <p className="task-card-empty">No enriched fields have been set.</p>
              ) : null}
            </div>
          </Section>
          {canWrite ? (
            <TitleCorrectionForm
              task={task}
              pending={mutation.isPending}
              submit={(title) =>
                mutation.mutate({
                  type: "correct",
                  field: "title",
                  value: title,
                  source: "owner",
                  reason: "owner correction from Web task detail",
                })
              }
            />
          ) : null}
        </>
      ) : null}
    </Page>
  );
}

function TaskDetailActions({
  task,
  pending,
  mutate,
}: {
  task: TaskDetail;
  pending: boolean;
  mutate: (operation: JsonObject) => void;
}) {
  const hardDue = sourcedCell(task.task.hard_due);
  const inferredHard = Boolean(
    hardDue &&
      (hardDue.source === "derived" || hardDue.source.startsWith("agent:")),
  );
  const active = task.status === "open" || task.status === "waiting";
  return (
    <div className="task-detail-actions" aria-label="Task actions">
      {!active ? (
        <button className="button secondary" type="button" disabled={pending} onClick={() => mutate({ type: "reopen", source: "owner" })}>
          <RotateCcw size={16} aria-hidden="true" />
          Reopen
        </button>
      ) : (
        <button className="button task-complete-button" type="button" disabled={pending} onClick={() => mutate({ type: "complete", source: "owner", completed_via: "web" })}>
          <Check size={16} aria-hidden="true" />
          Complete
        </button>
      )}
      {active ? (
        <>
          <button className="button secondary" type="button" disabled={pending} onClick={() => mutate({ type: "snooze", source: "owner", days: 1 })}>
            <CalendarClock size={16} aria-hidden="true" />
            Snooze tomorrow
          </button>
          <button className="button secondary" type="button" disabled={pending} onClick={() => mutate({ type: "pin_today", source: "owner" })}>
            <Pin size={16} aria-hidden="true" />
            Pin today
          </button>
        </>
      ) : null}
      {active && inferredHard ? (
        <button className="button secondary" type="button" disabled={pending} onClick={() => mutate({ type: "confirm_hard", source: "owner" })}>
          <ShieldCheck size={16} aria-hidden="true" />
          Confirm hard deadline
        </button>
      ) : null}
      {active && hardDue ? (
        <button className="button secondary" type="button" disabled={pending} onClick={() => mutate({ type: "downgrade_to_soft", source: "owner" })}>
          <ShieldMinus size={16} aria-hidden="true" />
          Downgrade to soft due
        </button>
      ) : null}
      {task.status !== "dropped" ? (
        <button
          className="button secondary task-drop-button"
          type="button"
          disabled={pending}
          onClick={() =>
            mutate({
              type: "drop",
              source: "owner",
              reason: "owner dropped from Web",
            })
          }
        >
          <Trash2 size={16} aria-hidden="true" />
          Drop
        </button>
      ) : null}
    </div>
  );
}

function TitleCorrectionForm({
  task,
  pending,
  submit,
}: {
  task: TaskDetail;
  pending: boolean;
  submit: (title: string) => void;
}) {
  const [title, setTitle] = useState(task.title);
  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (title.trim() && title.trim() !== task.title) submit(title.trim());
  }
  return (
    <Section title="Correct title" meta="Owner corrections take precedence">
      <form className="task-correction-form" onSubmit={onSubmit}>
        <label>
          <span>Title</span>
          <input value={title} onChange={(event) => setTitle(event.target.value)} maxLength={500} required />
        </label>
        <button className="button primary" type="submit" disabled={pending || !title.trim() || title.trim() === task.title}>
          Save owner correction
        </button>
      </form>
    </Section>
  );
}

function taskDefinitionItems(task: TaskDetail) {
  const cells = Object.fromEntries(taskFieldEntries(task));
  return [
    { label: "Status", value: humanize(task.status) },
    { label: "Project", value: formatTaskValue(cells.project?.value) },
    { label: "Ready", value: formatTaskValue(cells.ready_at?.value) },
    { label: "Soft due", value: formatTaskValue(cells.soft_due?.value) },
    { label: "Hard deadline", value: formatTaskValue(cells.hard_due?.value) },
    { label: "Contexts", value: formatTaskValue(cells.required_contexts?.value) },
    { label: "Estimate", value: cells.estimate_minutes ? `${formatTaskValue(cells.estimate_minutes.value)} min` : "Not set" },
    { label: "Cost of delay", value: formatTaskValue(cells.cost_of_delay?.value) },
  ];
}

function taskFieldEntries(task: TaskDetail): Array<[string, { value: JsonValue; source: string; set_at: string; note?: string | null }]> {
  return Object.entries(task.task).flatMap(([field, value]) => {
    const cell = sourcedCell(value);
    return cell ? [[field, cell]] : [];
  });
}

function sourcedCell(value: JsonValue | undefined) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const cell = value as Record<string, JsonValue>;
  if (typeof cell.source !== "string" || typeof cell.set_at !== "string" || !("value" in cell)) {
    return null;
  }
  return {
    value: cell.value,
    source: cell.source,
    set_at: cell.set_at,
    note: typeof cell.note === "string" ? cell.note : null,
  };
}

function formatTaskValue(value: JsonValue | undefined): string {
  if (value === undefined || value === null || value === "") return "Not set";
  if (Array.isArray(value)) return value.map((item) => formatTaskValue(item)).join(", ");
  if (typeof value === "object") return JSON.stringify(value);
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (typeof value === "string" && /^\d{4}-\d{2}-\d{2}T/u.test(value)) return formatDate(value);
  return String(value);
}
