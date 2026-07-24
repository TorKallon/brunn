import { useInfiniteQuery, useMutation } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import type { ColumnDef } from "@tanstack/react-table";
import { ChevronsDown, Clock3, FolderOpen, Plus, RotateCcw } from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";
import { DataTable } from "../components/DataTable";
import { Metric, Page, PageHeader, Section } from "../components/Page";
import {
  EmptyState,
  ErrorState,
  LoadingState,
  ProtocolNotice,
  StatusBadge,
} from "../components/StateViews";
import { useApi } from "../lib/auth";
import { useCurrent, useReadOnly } from "../lib/current";
import { formatDate, formatRelative, shortId } from "../lib/format";
import { asItems, type SessionSummary } from "../lib/types";

export function WorkPage() {
  const api = useApi();
  const navigate = useNavigate();
  const current = useCurrent();
  const readOnly = useReadOnly();
  const [newWorkspaceOpen, setNewWorkspaceOpen] = useState(false);
  const [task, setTask] = useState("");
  const [rootRefs, setRootRefs] = useState("");
  const sessionsQuery = useInfiniteQuery({
    queryKey: ["sessions"],
    queryFn: ({ pageParam }) => api.sessions(pageParam || undefined),
    initialPageParam: "",
    getNextPageParam: (lastPage) =>
      Array.isArray(lastPage.data)
        ? undefined
        : lastPage.data.continuation_token ?? undefined,
  });
  const openMutation = useMutation({
    mutationFn: () =>
      api.open({
        task,
        mode: "continuation",
        hints: {
          authorization_scope: current.data.active_scope?.id ?? "authorized",
          root_refs: rootRefs
            .split("\n")
            .map((value) => value.trim())
            .filter(Boolean),
          open_object_refs: [],
        },
        token_budget: 24_000,
      }),
    onSuccess: (result) => {
      const sessionId = result.data.session_id ?? result.session_id ?? result.data.id;
      void navigate({ to: "/sessions/$sessionId", params: { sessionId } });
    },
  });

  const sessions = sessionsQuery.data?.pages.flatMap((page) => asItems(page.data)) ?? [];
  const sessionTotal = (() => {
    const first = sessionsQuery.data?.pages[0]?.data;
    return first && !Array.isArray(first) ? first.total ?? sessions.length : sessions.length;
  })();
  const latestSessionEnvelope = sessionsQuery.data?.pages.at(-1);
  const goals = sessions.flatMap((session) => session.goals ?? []);
  const gates = sessions.flatMap((session) => session.gates ?? []);
  const actions = sessions.flatMap((session) => session.next_actions ?? []);
  const activeSessions = sessions.filter((session) => !["closed", "expired"].includes(session.status ?? "active"));
  const blockedGates = gates.filter((gate) => ["blocked", "failed"].includes(gate.status ?? ""));
  const dueActions = actions.filter((action) => action.status !== "complete" && action.due_at);

  const sessionColumns = useMemo<ColumnDef<SessionSummary, unknown>[]>(
    () => [
      {
        accessorKey: "title",
        header: "Session",
        cell: ({ row }) => (
          <div className="primary-cell">
            <strong>{row.original.title ?? "Untitled workspace"}</strong>
            <code>{shortId(row.original.id)}</code>
          </div>
        ),
      },
      {
        accessorKey: "status",
        header: "Status",
        cell: ({ getValue }) => <StatusBadge status={getValue<string>()} />,
      },
      {
        accessorKey: "corpus_revision",
        header: "Revision",
        cell: ({ getValue }) => <code>{shortId(getValue<string>())}</code>,
      },
      {
        accessorKey: "updated_at",
        header: "Updated",
        cell: ({ getValue }) => formatRelative(getValue<string>()),
      },
    ],
    [],
  );

  function submitWorkspace(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!readOnly && task.trim()) openMutation.mutate();
  }

  return (
    <Page>
      <PageHeader
        title="Work"
        description="Active continuity, attention, and resumable sessions"
        actions={
          <button
            className="button primary"
            type="button"
            onClick={() => setNewWorkspaceOpen((value) => !value)}
            disabled={readOnly}
            title={readOnly ? "Requires a read/write credential" : undefined}
          >
            {newWorkspaceOpen ? <RotateCcw size={17} /> : <Plus size={17} />}
            {newWorkspaceOpen ? "Cancel" : "New workspace"}
          </button>
        }
      />

      <ProtocolNotice
        status={latestSessionEnvelope?.status}
        gaps={latestSessionEnvelope?.gaps?.length}
        conflicts={latestSessionEnvelope?.conflicts?.length}
      />

      {newWorkspaceOpen ? (
        <Section title="Open workspace">
          <form className="form-grid" onSubmit={submitWorkspace}>
            <label className="field field-span-2">
              <span>Task</span>
              <textarea
                value={task}
                onChange={(event) => setTask(event.target.value)}
                rows={3}
                required
                disabled={readOnly}
              />
            </label>
            <label className="field field-span-2">
              <span>Root references</span>
              <textarea
                value={rootRefs}
                onChange={(event) => setRootRefs(event.target.value)}
                rows={2}
                placeholder="One object or source reference per line"
                disabled={readOnly}
              />
            </label>
            <div className="form-actions field-span-2">
              <button
                className="button primary"
                type="submit"
                disabled={readOnly || !task.trim() || openMutation.isPending}
                title={readOnly ? "Requires a read/write credential" : undefined}
              >
                <FolderOpen size={17} aria-hidden="true" />
                {openMutation.isPending ? "Opening" : "Open"}
              </button>
              {openMutation.isError ? <span className="field-error">{openMutation.error.message}</span> : null}
            </div>
          </form>
        </Section>
      ) : null}

      <div className="metric-grid">
        <Metric label="Active sessions" value={activeSessions.length} detail={`${sessionTotal} total`} />
        <Metric label="Active goals" value={goals.filter((goal) => goal.status !== "complete").length} detail={`${goals.length} tracked`} />
        <Metric label="Blocked gates" value={blockedGates.length} detail={blockedGates[0]?.title ?? "No blocked gates"} />
        <Metric label="Due actions" value={dueActions.length} detail={dueActions[0]?.due_at ? formatDate(dueActions[0].due_at) : "Nothing scheduled"} />
      </div>

      {(blockedGates.length || dueActions.length) ? (
        <Section title="Attention" meta={`${blockedGates.length + dueActions.length} items`}>
          <div className="attention-list">
            {blockedGates.map((gate, index) => (
              <div key={gate.id ?? `${gate.title}-${index}`}>
                <StatusBadge status={gate.status} />
                <div>
                  <strong>{gate.title}</strong>
                  {gate.reason ? <span>{gate.reason}</span> : null}
                </div>
              </div>
            ))}
            {dueActions.map((action, index) => (
              <div key={action.id ?? `${action.title}-${index}`}>
                <Clock3 size={17} aria-hidden="true" />
                <div>
                  <strong>{action.title}</strong>
                  <span>{formatDate(action.due_at)}{action.owner ? ` · ${action.owner}` : ""}</span>
                </div>
              </div>
            ))}
          </div>
        </Section>
      ) : null}

      <Section
        title="Recent sessions"
        meta={sessions.length ? `${sessions.length} of ${sessionTotal} available` : undefined}
        actions={sessionsQuery.isError ? (
          <button className="icon-button" type="button" onClick={() => void sessionsQuery.refetch()} aria-label="Retry sessions" title="Retry">
            <RotateCcw size={16} />
          </button>
        ) : undefined}
      >
        {sessionsQuery.isPending ? <LoadingState label="Loading sessions" /> : null}
        {sessionsQuery.isError ? <ErrorState error={sessionsQuery.error} retry={() => void sessionsQuery.refetch()} /> : null}
        {sessionsQuery.isSuccess ? (
          sessions.length ? (
            <>
              <DataTable
                data={sessions}
                columns={sessionColumns}
                onRowClick={(session) => void navigate({ to: "/sessions/$sessionId", params: { sessionId: session.id } })}
                getRowLabel={(session) => `Open ${session.title ?? session.id}`}
              />
              {sessionsQuery.hasNextPage ? (
                <div className="section-footer">
                  <button
                    className="button secondary"
                    type="button"
                    onClick={() => void sessionsQuery.fetchNextPage()}
                    disabled={sessionsQuery.isFetchingNextPage}
                  >
                    <ChevronsDown size={16} aria-hidden="true" />
                    {sessionsQuery.isFetchingNextPage ? "Loading" : "Load more"}
                  </button>
                </div>
              ) : null}
            </>
          ) : (
            <EmptyState
              title="No sessions"
              action={
                <button
                  className="button secondary"
                  type="button"
                  onClick={() => setNewWorkspaceOpen(true)}
                  disabled={readOnly}
                  title={readOnly ? "Requires a read/write credential" : undefined}
                >
                  <Plus size={16} />
                  Open workspace
                </button>
              }
            />
          )
        ) : null}
      </Section>
    </Page>
  );
}
