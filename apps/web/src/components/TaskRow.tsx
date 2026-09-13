import { Link } from "@tanstack/react-router";
import {
  CalendarClock,
  Check,
  CircleDotDashed,
  RotateCcw,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import type { TaskCandidate } from "../lib/types";
import { confirmTaskDeletion, type TaskQuickAction } from "../lib/taskOperations";

export function TaskRow({
  item,
  canWrite,
  onAction,
  pending = false,
  showActions = true,
}: {
  item: TaskCandidate;
  canWrite: boolean;
  onAction?: (item: TaskCandidate, action: TaskQuickAction) => void;
  pending?: boolean;
  showActions?: boolean;
}) {
  const inferredHardDeadline =
    item.hard_due != null && (item.hard_due_source != null
      ? isInferredSource(item.hard_due_source)
      : item.timing == null && item.provenance_markers.some(isInferredSource));
  const active = item.status === "open" || item.status === "waiting";
  return (
    <article
      className={`task-row tier-${item.tier} ${item.pinned ? "is-pinned" : ""}`.trim()}
      data-testid={`task-row-${item.task_ref}`}
      data-task-ref={item.task_ref}
    >
      <span className="sr-only" data-testid="task-row">
        Task row
      </span>
      <span className="task-tier" aria-label={`Priority tier ${item.tier}`}>
        {item.pinned ? <CircleDotDashed size={15} aria-hidden="true" /> : item.tier}
      </span>
      <div className="task-row-copy">
        <div className="task-row-title">
          <Link
            to="/tasks/$taskRef"
            params={{ taskRef: item.task_ref }}
          >
            {item.title}
          </Link>
          {item.project ? (
            <Link
              className="task-project-chip"
              to="/projects/$slug"
              params={{ slug: item.project }}
            >
              {item.project}
            </Link>
          ) : null}
        </div>
        <div className="task-reason-row">
          <span>{item.reason}</span>
          {item.today_since ? <span>Today</span> : null}
          {item.ready_at && new Date(item.ready_at) > new Date() ? <span>Deferred until {new Date(item.ready_at).toLocaleString()}</span> : null}
          {item.timing?.timing_unknown && !item.hard_due ? <span>Timing needs clarification</span> : null}
          {item.status === "dropped" ? <span>Deleted</span> : null}
          {item.provenance_markers.map((source) => (
            <span
              className="task-provenance"
              role="img"
              aria-label={provenanceLabel(source)}
              title={provenanceLabel(source)}
              key={source}
            >
              <CircleDotDashed size={13} aria-hidden="true" />
              <span aria-hidden="true">est.</span>
            </span>
          ))}
        </div>
        {item.required_contexts.length ? (
          <span className="task-contexts" aria-label="Required contexts">
            {item.required_contexts.map((context) => (
              <span key={context}>{context}</span>
            ))}
          </span>
        ) : null}
      </div>
      {showActions ? (
        <div className="task-row-actions">
          {canWrite && onAction ? (
            <>
              {active ? (
                <>
                  <button
                    className="task-action task-action-complete"
                    type="button"
                    aria-label="Complete"
                    title="Complete"
                    disabled={pending}
                    onClick={() => onAction(item, "complete")}
                  >
                    <Check size={16} aria-hidden="true" />
                  </button>
                  <button
                    className="task-action task-action-tomorrow"
                    type="button"
                    aria-label="Tomorrow"
                    title="Tomorrow — next local morning; deadlines do not move"
                    disabled={pending}
                    onClick={() => onAction(item, "snooze")}
                  >
                    <CalendarClock size={16} aria-hidden="true" />
                    <span>Tomorrow</span>
                  </button>
                </>
              ) : (
                <button
                  className="task-action"
                  type="button"
                  aria-label={item.status === "dropped" ? "Restore task" : "Reopen"}
                  title={item.status === "dropped" ? "Restore task" : "Reopen"}
                  disabled={pending}
                  onClick={() => onAction(item, "reopen")}
                >
                  <RotateCcw size={16} aria-hidden="true" />
                </button>
              )}
              {active && inferredHardDeadline ? (
                <button
                  className="task-action"
                  type="button"
                  aria-label="Confirm hard deadline"
                  title="Confirm hard deadline"
                  disabled={pending}
                  onClick={() => onAction(item, "confirm_hard")}
                >
                  <ShieldCheck size={16} aria-hidden="true" />
                </button>
              ) : null}
              {item.status !== "dropped" ? (
                <button
                  className="task-action task-action-drop"
                  type="button"
                  aria-label="Delete task"
                  title="Delete task"
                  disabled={pending}
                  onClick={() => {
                    if (confirmTaskDeletion(item.title)) onAction(item, "drop");
                  }}
                >
                  <Trash2 size={16} aria-hidden="true" />
                </button>
              ) : null}
            </>
          ) : (
            <span className="task-view-only">View only</span>
          )}
        </div>
      ) : null}
    </article>
  );
}

function isInferredSource(source: string): boolean {
  return source === "derived" || source.startsWith("agent:");
}

function provenanceLabel(source: string): string {
  if (source === "todoist") return "Set by Todoist";
  if (source === "derived") return "Derived by Brunn";
  if (source.startsWith("agent:")) return `Inferred by ${source}`;
  return `Set by ${source}`;
}
