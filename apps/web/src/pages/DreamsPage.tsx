import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";
import {
  ChevronsDown,
  FileDiff,
  History,
  Play,
  RefreshCw,
  Sparkles,
} from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";
import { DataTable } from "../components/DataTable";
import { WorkspaceEntryView } from "../components/WorkspaceEntryView";
import {
  DefinitionList,
  Metric,
  Page,
  PageHeader,
  Section,
} from "../components/Page";
import {
  EmptyState,
  ErrorState,
  LoadingState,
  ReadOnlyNotice,
  StatusBadge,
} from "../components/StateViews";
import { useApi } from "../lib/auth";
import { useCurrent, useReadOnly } from "../lib/current";
import { formatDate, formatRelative, humanize, shortId } from "../lib/format";
import type {
  WorkspaceJob,
  WorkspaceManifestEntry,
} from "../lib/types";
import {
  isProposalEntry,
  workspaceEntryKind,
  workspaceManifestTimestamp,
} from "../lib/workspace";

const MANIFEST_PAGE_SIZE = 1_000;

export function DreamsPage() {
  const api = useApi();
  const current = useCurrent();
  const readOnly = useReadOnly();
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState<WorkspaceManifestEntry | null>(null);
  const [focus, setFocus] = useState("");
  const canDream =
    !readOnly && (current.data.capabilities?.includes("dream") ?? false);
  const manifestQuery = useInfiniteQuery({
    queryKey: ["workspace-manifest", "background"],
    queryFn: ({ pageParam }) =>
      api.workspaceManifest(pageParam, MANIFEST_PAGE_SIZE),
    initialPageParam: 0,
    getNextPageParam: (lastPage, pages) =>
      lastPage.data.truncated
        ? pages.reduce((total, page) => total + page.data.entries.length, 0)
        : undefined,
  });
  const generation =
    manifestQuery.data?.pages[0]?.data.workspace_generation ?? 0;
  const changesQuery = useQuery({
    queryKey: ["workspace-changes", "background", generation],
    queryFn: () => api.workspaceChanges(Math.max(0, generation - 500), 500),
    enabled: manifestQuery.isSuccess,
  });
  const jobsQuery = useQuery({
    queryKey: ["workspace-jobs"],
    queryFn: () => api.workspaceJobs(undefined, 0, 100),
    refetchInterval: (query) =>
      query.state.data?.data.jobs.some(
        (job) => job.status === "queued" || job.status === "running",
      )
        ? 5_000
        : 30_000,
  });
  const proposalQuery = useQuery({
    queryKey: ["workspace-proposal", selected?.entry_ref],
    queryFn: () =>
      api.workspaceRead({
        requests: [{ ref: selected!.entry_ref, view: "full" }],
      }),
    enabled: Boolean(selected),
  });
  const dreamMutation = useMutation({
    mutationFn: () =>
      api.workspaceDream({
        ...(focus.trim() ? { focus: focus.trim() } : {}),
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["workspace-jobs"] });
      void queryClient.invalidateQueries({ queryKey: ["workspace-manifest"] });
      void queryClient.invalidateQueries({ queryKey: ["workspace-changes"] });
    },
  });

  const entries =
    manifestQuery.data?.pages.flatMap((page) => page.data.entries) ?? [];
  const proposals = useMemo(
    () =>
      entries
        .filter(isProposalEntry)
        .sort((left, right) =>
          (workspaceManifestTimestamp(right) ?? "").localeCompare(
            workspaceManifestTimestamp(left) ?? "",
          ),
        ),
    [entries],
  );
  const backgroundChanges = useMemo(
    () =>
      (changesQuery.data?.data.changes ?? []).filter(
        (change) =>
          change.path.startsWith(".straylight/") ||
          change.path.startsWith("Inbox/Proposals/"),
      ),
    [changesQuery.data],
  );
  const selectedEntry = proposalQuery.data?.data.items[0];
  const jobs = jobsQuery.data?.data.jobs ?? [];
  const activeJobs = jobs.filter(
    (job) => job.status === "queued" || job.status === "running",
  ).length;
  const columns = useMemo<ColumnDef<WorkspaceManifestEntry, unknown>[]>(
    () => [
      {
        id: "proposal",
        header: "Proposal",
        cell: ({ row }) => (
          <div className="primary-cell">
            <strong>{row.original.title}</strong>
            <code>{row.original.path}</code>
          </div>
        ),
      },
      {
        id: "kind",
        header: "Kind",
        cell: ({ row }) => (
          <StatusBadge status={workspaceEntryKind(row.original)} />
        ),
      },
      {
        accessorKey: "version",
        header: "Version",
        cell: ({ getValue }) => `v${getValue<number>()}`,
      },
      {
        id: "updated_at",
        header: "Updated",
        cell: ({ row }) =>
          formatRelative(workspaceManifestTimestamp(row.original)),
      },
    ],
    [],
  );
  const jobColumns = useMemo<ColumnDef<WorkspaceJob, unknown>[]>(
    () => [
      {
        id: "job",
        header: "Job",
        cell: ({ row }) => (
          <div className="primary-cell">
            <strong>{humanize(row.original.kind)}</strong>
            <code title={row.original.job_ref}>
              {shortId(row.original.job_ref, 18)}
            </code>
            {row.original.last_error ? (
              <span className="field-error">{row.original.last_error}</span>
            ) : null}
          </div>
        ),
      },
      {
        accessorKey: "status",
        header: "Status",
        cell: ({ getValue }) => (
          <StatusBadge status={getValue<string>()} />
        ),
      },
      {
        accessorKey: "watermark",
        header: "Watermark",
        cell: ({ getValue }) => {
          const watermark = getValue<number | null>();
          return watermark === null || watermark === undefined
            ? "None"
            : `g${watermark}`;
        },
      },
      {
        accessorKey: "attempts",
        header: "Attempts",
      },
      {
        accessorKey: "created_at",
        header: "Created",
        cell: ({ getValue }) => formatRelative(getValue<string>()),
      },
    ],
    [],
  );

  function queueDream(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (canDream) dreamMutation.mutate();
  }

  function refreshBackground() {
    void queryClient.invalidateQueries({ queryKey: ["workspace-jobs"] });
    void queryClient.invalidateQueries({ queryKey: ["workspace-manifest"] });
    void queryClient.invalidateQueries({ queryKey: ["workspace-changes"] });
  }

  return (
    <Page>
      <PageHeader
        title="Background"
        description="Maintenance proposals and visible workspace changes"
        actions={
          <button
            className="icon-button"
            type="button"
            onClick={refreshBackground}
            aria-label="Refresh background state"
            title="Refresh"
          >
            <RefreshCw size={17} aria-hidden="true" />
          </button>
        }
      />
      {readOnly ? <ReadOnlyNotice /> : null}

      <div className="metric-grid">
        <Metric label="Workspace generation" value={generation} />
        <Metric label="Proposal files" value={proposals.length} />
        <Metric label="Background changes" value={backgroundChanges.length} />
        <Metric
          label="Background jobs"
          value={jobs.length}
          detail={
            jobsQuery.isError
              ? "Unavailable"
              : activeJobs
                ? `${activeJobs} active`
                : "No active jobs"
          }
        />
      </div>

      <Section title="Background state" actions={<Sparkles size={18} aria-hidden="true" />}>
        <DefinitionList
          items={[
            {
              label: "Proposal source",
              value: "Current Markdown entries",
            },
            {
              label: "Worker jobs",
              value: jobsQuery.isError
                ? "Job feed unavailable"
                : `${jobs.length} recent jobs loaded`,
            },
            {
              label: "Patch diff",
              value: "Stored proposal Markdown only",
            },
            {
              label: "Review and revert",
              value: "No workspace endpoint available",
            },
          ]}
        />
      </Section>

      <div className="workspace-split">
        <Section title="Dream pass" actions={<Sparkles size={18} aria-hidden="true" />}>
          <form className="form-grid" onSubmit={queueDream}>
            <label className="field field-span-2">
              <span>Focus</span>
              <textarea
                rows={4}
                value={focus}
                onChange={(event) => setFocus(event.target.value)}
                disabled={!canDream}
              />
            </label>
            <div className="form-actions field-span-2">
              <button
                className="button primary"
                type="submit"
                disabled={!canDream || dreamMutation.isPending}
                title={
                  canDream
                    ? undefined
                    : "Requires a credential with dream access"
                }
              >
                <Play size={17} aria-hidden="true" />
                {dreamMutation.isPending ? "Queueing" : "Run now"}
              </button>
            </div>
          </form>
          {dreamMutation.isError ? (
            <ErrorState
              error={dreamMutation.error}
              title="Unable to queue dream pass"
            />
          ) : null}
          {dreamMutation.data ? (
            <div className="receipt-heading" role="status">
              <StatusBadge
                status={
                  dreamMutation.data.data.status ?? dreamMutation.data.status
                }
              />
              <code>
                {dreamMutation.data.data.job_ref ??
                  dreamMutation.data.data.reason ??
                  `generation:${dreamMutation.data.data.workspace_generation}`}
              </code>
            </div>
          ) : null}
        </Section>

        <Section
          title="Recent jobs"
          meta={jobs.length ? `${jobs.length} loaded` : undefined}
        >
          {jobsQuery.isPending ? (
            <LoadingState label="Loading background jobs" />
          ) : null}
          {jobsQuery.isError ? (
            <ErrorState
              error={jobsQuery.error}
              retry={() => void jobsQuery.refetch()}
              title="Unable to load background jobs"
            />
          ) : null}
          {jobsQuery.isSuccess ? (
            <DataTable
              data={jobs}
              columns={jobColumns}
              emptyTitle="No background jobs"
            />
          ) : null}
        </Section>
      </div>

      <div className="workspace-split">
        <Section
          title="Proposal Markdown"
          meta={proposals.length ? `${proposals.length} loaded` : undefined}
          actions={<FileDiff size={18} aria-hidden="true" />}
        >
          {manifestQuery.isPending ? (
            <LoadingState label="Loading workspace manifest" />
          ) : null}
          {manifestQuery.isError ? (
            <ErrorState
              error={manifestQuery.error}
              retry={() => void manifestQuery.refetch()}
              title="Unable to load proposals"
            />
          ) : null}
          {manifestQuery.isSuccess && !proposals.length ? (
            <EmptyState title="No proposal Markdown" />
          ) : null}
          {proposals.length ? (
            <>
              <DataTable
                data={proposals}
                columns={columns}
                onRowClick={setSelected}
                getRowLabel={(entry) => `Read proposal ${entry.title}`}
              />
              {manifestQuery.hasNextPage ? (
                <div className="section-footer">
                  <button
                    className="button secondary"
                    type="button"
                    onClick={() => void manifestQuery.fetchNextPage()}
                    disabled={manifestQuery.isFetchingNextPage}
                  >
                    <ChevronsDown size={16} aria-hidden="true" />
                    {manifestQuery.isFetchingNextPage ? "Loading" : "Load more"}
                  </button>
                </div>
              ) : null}
            </>
          ) : null}
        </Section>

        <Section title="Selected proposal">
          {selected && proposalQuery.isPending ? (
            <LoadingState label="Reading proposal" />
          ) : null}
          {proposalQuery.isError ? (
            <ErrorState
              error={proposalQuery.error}
              retry={() => void proposalQuery.refetch()}
              title="Unable to read proposal"
            />
          ) : null}
          {selectedEntry ? (
            <WorkspaceEntryView entry={selectedEntry} />
          ) : !selected ? (
            <EmptyState title="No proposal selected" />
          ) : null}
        </Section>
      </div>

      <Section
        title="Recent background changes"
        actions={<History size={18} aria-hidden="true" />}
      >
        {changesQuery.isPending ? (
          <LoadingState label="Loading background changes" />
        ) : null}
        {changesQuery.isError ? (
          <ErrorState
            error={changesQuery.error}
            retry={() => void changesQuery.refetch()}
            title="Unable to load changes"
          />
        ) : null}
        {changesQuery.isSuccess && !backgroundChanges.length ? (
          <EmptyState title="No recent background changes" />
        ) : null}
        {backgroundChanges.length ? (
          <div className="change-list">
            {backgroundChanges.map((change) => (
              <div key={change.generation}>
                <StatusBadge status={change.operation} />
                <code>{change.path}</code>
                <span>{formatDate(change.recorded_at)}</span>
                <strong>g{change.generation}</strong>
              </div>
            ))}
          </div>
        ) : null}
      </Section>
    </Page>
  );
}
