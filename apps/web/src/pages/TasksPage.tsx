import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronLeft, ChevronRight, ListFilter } from "lucide-react";
import { useState } from "react";
import { Page, PageHeader } from "../components/Page";
import { EmptyState, ErrorState, LoadingState } from "../components/StateViews";
import { TaskRow } from "../components/TaskRow";
import { useApi } from "../lib/auth";
import { useCapability } from "../lib/current";
import { taskQuickOperation, type TaskQuickAction } from "../lib/taskOperations";
import type {
  TaskCandidate,
  TaskDateTypeFilter,
  TaskSourceFilter,
  TaskStatus,
} from "../lib/types";
import { newOperationId } from "../lib/workspace";

type StatusFilter = "all" | TaskStatus;

export function TasksPage() {
  const api = useApi();
  const queryClient = useQueryClient();
  const canWrite = useCapability("task.write");
  const [status, setStatus] = useState<StatusFilter>("all");
  const [project, setProject] = useState("");
  const [context, setContext] = useState("");
  const [dateType, setDateType] = useState<TaskDateTypeFilter>("all");
  const [source, setSource] = useState<TaskSourceFilter>("all");
  const [includeParked, setIncludeParked] = useState(false);
  const [includeWaiting, setIncludeWaiting] = useState(false);
  const [pageCursors, setPageCursors] = useState<string[]>([]);
  const cursor = pageCursors.at(-1);

  const contextsQuery = useQuery({
    queryKey: ["task-contexts"],
    queryFn: () => api.taskContexts(),
  });
  const projectsQuery = useQuery({
    queryKey: ["task-projects"],
    queryFn: () => api.taskProjects(),
  });
  const query = useQuery({
    queryKey: [
      "task-candidates",
      "all",
      cursor,
      status,
      project,
      context,
      includeWaiting,
      includeParked,
      dateType,
      source,
    ],
    queryFn: () =>
      api.taskCandidates({
        view: "all",
        limit: 25,
        deliberate_all: true,
        cursor,
        status,
        project: project || undefined,
        context: context || undefined,
        date_type: dateType,
        source,
        include_waiting: includeWaiting,
        include_parked: includeParked,
      }),
  });
  const actionMutation = useMutation({
    mutationFn: ({ item, action }: { item: TaskCandidate; action: TaskQuickAction }) =>
      api.taskUpdate(item.task_ref, {
        expected_version: item.version,
        idempotency_key: newOperationId(`web_all_${action}`),
        operation: taskQuickOperation(action),
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["task-candidates"] });
      void queryClient.invalidateQueries({ queryKey: ["task-done"] });
    },
  });

  const items = query.data?.data.items ?? [];
  const resetPages = () => setPageCursors([]);

  return (
    <Page>
      <PageHeader
        title="All tasks"
        description="The deliberate backlog view. Results remain paginated in groups of 25."
        actions={<ListFilter size={19} aria-hidden="true" />}
      />
      <fieldset className="task-filter-panel">
        <legend>Task filters</legend>
        <label>
          <span>Status</span>
          <select
            value={status}
            onChange={(event) => {
              setStatus(event.target.value as StatusFilter);
              resetPages();
            }}
          >
            <option value="all">All statuses</option>
            <option value="open">Open</option>
            <option value="waiting">Waiting</option>
            <option value="done">Done</option>
            <option value="dropped">Dropped</option>
          </select>
        </label>
        <label>
          <span>Project</span>
          <select
            value={project}
            onChange={(event) => {
              setProject(event.target.value);
              resetPages();
            }}
          >
            <option value="">Any project</option>
            {(projectsQuery.data?.data.projects ?? []).map((item) => (
              <option key={item.slug} value={item.slug}>{item.title}</option>
            ))}
          </select>
        </label>
        <label>
          <span>Context</span>
          <select
            value={context}
            onChange={(event) => {
              setContext(event.target.value);
              resetPages();
            }}
          >
            <option value="">Any context</option>
            {(contextsQuery.data?.data.contexts ?? [])
              .filter((item) => !item.archived)
              .map((item) => (
                <option key={item.slug} value={item.slug}>{item.display_name}</option>
              ))}
          </select>
        </label>
        <label>
          <span>Date type</span>
          <select
            value={dateType}
            onChange={(event) => {
              setDateType(event.target.value as TaskDateTypeFilter);
              resetPages();
            }}
          >
            <option value="all">All date types</option>
            <option value="hard">Hard deadline</option>
            <option value="cost">Cost of delay</option>
            <option value="soft">Soft due</option>
            <option value="none">No pressure date</option>
          </select>
        </label>
        <label>
          <span>Source</span>
          <select
            value={source}
            onChange={(event) => {
              setSource(event.target.value as TaskSourceFilter);
              resetPages();
            }}
          >
            <option value="all">All sources</option>
            <option value="owner">Owner-set</option>
            <option value="agent">Agent-set</option>
            <option value="derived">Derived</option>
            <option value="todoist">Todoist</option>
          </select>
        </label>
        <label className="task-filter-check">
          <input
            type="checkbox"
            checked={includeParked}
            onChange={(event) => {
              setIncludeParked(event.target.checked);
              resetPages();
            }}
          />
          Include parked
        </label>
        <label className="task-filter-check">
          <input
            type="checkbox"
            checked={includeWaiting}
            onChange={(event) => {
              setIncludeWaiting(event.target.checked);
              resetPages();
            }}
          />
          Include waiting
        </label>
      </fieldset>

      <p className="task-list-summary" role="status">
        Page {pageCursors.length + 1} · {query.data?.data.backlog_total ?? 0} matching
        tasks · {query.data?.data.next_remaining ?? 0} after this page
      </p>
      {query.isPending ? <LoadingState label="Loading the deliberate task page" /> : null}
      {query.isError ? (
        <ErrorState error={query.error} retry={() => void query.refetch()} title="Unable to load tasks" />
      ) : null}
      {actionMutation.isError ? (
        <ErrorState error={actionMutation.error} title="Task action failed" />
      ) : null}
      {!query.isPending && !query.isError && items.length === 0 ? (
        <EmptyState
          title="No tasks match this page"
          detail="Change a filter or move to another page. Nothing was removed."
        />
      ) : null}
      <section className="all-task-list" aria-label="Filtered tasks">
        {items.map((item) => (
          <TaskRow
            key={item.task_ref}
            item={item}
            canWrite={canWrite}
            pending={actionMutation.isPending}
            onAction={(task, action) => actionMutation.mutate({ item: task, action })}
          />
        ))}
      </section>
      <nav className="task-pagination" aria-label="Task pages">
        <button
          className="button secondary"
          type="button"
          disabled={pageCursors.length === 0}
          onClick={() => setPageCursors((values) => values.slice(0, -1))}
        >
          <ChevronLeft size={16} aria-hidden="true" />
          Previous page
        </button>
        <button
          className="button secondary"
          type="button"
          disabled={!query.data?.data.next_cursor}
          onClick={() => {
            const nextCursor = query.data?.data.next_cursor;
            if (nextCursor) setPageCursors((values) => [...values, nextCursor]);
          }}
        >
          Next page
          <ChevronRight size={16} aria-hidden="true" />
        </button>
      </nav>
    </Page>
  );
}
