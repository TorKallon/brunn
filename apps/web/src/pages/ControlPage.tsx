import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";
import {
  Activity,
  Ban,
  Check,
  Copy,
  FileClock,
  KeyRound,
  Plus,
  RefreshCw,
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
import { TabPanel, Tabs } from "../components/Tabs";
import { useApi } from "../lib/auth";
import { useCurrent, useReadOnly } from "../lib/current";
import { formatDate, formatRelative, humanize, shortId } from "../lib/format";
import {
  asItems,
  type CredentialSummary,
  type WorkspaceChange,
  type WorkspaceManifestEntry,
  type WorkspaceUsageEntry,
  type WorkspaceUsageSort,
} from "../lib/types";
import { workspaceManifestTimestamp } from "../lib/workspace";

const numberFormat = new Intl.NumberFormat();

export function ControlPage() {
  const api = useApi();
  const current = useCurrent();
  const readOnly = useReadOnly();
  const queryClient = useQueryClient();
  const canManageCredentials =
    current.data.capabilities?.includes("credential:manage") ?? false;
  const [activeTab, setActiveTab] = useState("changes");
  const [usageRanking, setUsageRanking] =
    useState<WorkspaceUsageSort>("most_used");
  const [selectedCheckpoint, setSelectedCheckpoint] =
    useState<WorkspaceManifestEntry | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [credentialName, setCredentialName] = useState("");
  const [credentialAccess, setCredentialAccess] = useState<
    "read_only" | "read_write"
  >("read_only");
  const [copied, setCopied] = useState(false);

  const manifestQuery = useQuery({
    queryKey: ["workspace-manifest", "activity"],
    queryFn: () => api.workspaceManifest(0, 1_000),
  });
  const generation = manifestQuery.data?.data.workspace_generation ?? 0;
  const changesQuery = useQuery({
    queryKey: ["workspace-changes", "recent", generation],
    queryFn: () => api.workspaceChanges(Math.max(0, generation - 200), 200),
    enabled: manifestQuery.isSuccess,
  });
  const usageQuery = useQuery({
    queryKey: ["workspace-usage", usageRanking],
    queryFn: () => api.workspaceUsage(usageRanking, 0, 100),
    enabled: activeTab === "usage",
  });
  const checkpointQuery = useQuery({
    queryKey: ["workspace-checkpoint-entry", selectedCheckpoint?.entry_ref],
    queryFn: () =>
      api.workspaceRead({
        requests: [{ ref: selectedCheckpoint!.entry_ref, view: "full" }],
      }),
    enabled: Boolean(selectedCheckpoint),
  });
  const credentialsQuery = useQuery({
    queryKey: ["credentials"],
    queryFn: () => api.credentials(),
    enabled: activeTab === "access",
  });
  const statusQuery = useQuery({
    queryKey: ["service-status"],
    queryFn: () => api.status(),
    enabled: activeTab === "health",
    refetchInterval: activeTab === "health" ? 30_000 : false,
  });
  const createCredentialMutation = useMutation({
    mutationFn: () =>
      api.createCredential({
        name: credentialName.trim(),
        access: credentialAccess,
        scope_ids: current.data.active_scope?.id
          ? [current.data.active_scope.id]
          : [],
      }),
    onSuccess: () => {
      setCredentialName("");
      setCreateOpen(false);
      void queryClient.invalidateQueries({ queryKey: ["credentials"] });
    },
  });
  const revokeMutation = useMutation({
    mutationFn: (id: string) => api.revokeCredential(id),
    onSuccess: () =>
      void queryClient.invalidateQueries({ queryKey: ["credentials"] }),
  });

  const entries = manifestQuery.data?.data.entries ?? [];
  const checkpoints = entries.filter((entry) =>
    entry.path.startsWith(".straylight/checkpoints/"),
  );
  const changes = changesQuery.data?.data.changes ?? [];
  const usageEntries = usageQuery.data?.data.entries ?? [];
  const credentials = asItems(credentialsQuery.data?.data);
  const createdCredential = createCredentialMutation.data?.data;
  const selectedCheckpointEntry = checkpointQuery.data?.data.items[0];

  const changeColumns = useMemo<ColumnDef<WorkspaceChange, unknown>[]>(
    () => [
      {
        accessorKey: "generation",
        header: "Generation",
        cell: ({ getValue }) => `g${getValue<number>()}`,
      },
      {
        accessorKey: "operation",
        header: "Operation",
        cell: ({ getValue }) => (
          <StatusBadge status={getValue<string>()} />
        ),
      },
      {
        accessorKey: "path",
        header: "Path",
        cell: ({ getValue }) => <code>{getValue<string>()}</code>,
      },
      {
        accessorKey: "version",
        header: "Version",
        cell: ({ getValue }) => `v${getValue<number>()}`,
      },
      {
        accessorKey: "recorded_at",
        header: "Recorded",
        cell: ({ getValue }) => formatRelative(getValue<string>()),
      },
    ],
    [],
  );
  const usageColumns = useMemo<ColumnDef<WorkspaceUsageEntry, unknown>[]>(
    () => [
      {
        id: "entry",
        header: "Entry",
        cell: ({ row }) => (
          <div className="primary-cell">
            <strong>{row.original.title}</strong>
            <code>{row.original.path}</code>
          </div>
        ),
      },
      {
        accessorKey: "kind",
        header: "Kind",
        cell: ({ getValue }) => humanize(getValue<string>()),
      },
      {
        accessorKey: "read_count",
        header: "Reads",
        cell: ({ getValue }) => numberFormat.format(getValue<number>()),
      },
      {
        accessorKey: "search_count",
        header: "Searches",
        cell: ({ getValue }) => numberFormat.format(getValue<number>()),
      },
      {
        accessorKey: "total_uses",
        header: "Total",
        cell: ({ getValue }) => numberFormat.format(getValue<number>()),
      },
      {
        accessorKey: "last_used_at",
        header: "Last used",
        cell: ({ getValue }) => formatDate(getValue<string | null>()),
      },
    ],
    [],
  );
  const checkpointColumns = useMemo<
    ColumnDef<WorkspaceManifestEntry, unknown>[]
  >(
    () => [
      {
        id: "checkpoint",
        header: "Checkpoint",
        cell: ({ row }) => (
          <div className="primary-cell">
            <strong>{row.original.title}</strong>
            <code>{row.original.path}</code>
          </div>
        ),
      },
      {
        accessorKey: "version",
        header: "Version",
        cell: ({ getValue }) => `v${getValue<number>()}`,
      },
      {
        id: "created_at",
        header: "Created",
        cell: ({ row }) =>
          formatDate(workspaceManifestTimestamp(row.original)),
      },
    ],
    [],
  );

  function submitCredential(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (canManageCredentials && credentialName.trim()) {
      createCredentialMutation.mutate();
    }
  }

  async function copyToken(credential: CredentialSummary) {
    if (!credential.token) return;
    await navigator.clipboard.writeText(credential.token);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 2_000);
  }

  return (
    <Page>
      <PageHeader
        title="Activity"
        description="Changes, usage, checkpoints, access, and service state"
      />
      {readOnly ? <ReadOnlyNotice /> : null}
      <Tabs
        tabs={[
          { id: "changes", label: "Changes", count: changes.length || undefined },
          {
            id: "usage",
            label: "Usage",
            count: usageEntries.length || undefined,
          },
          {
            id: "checkpoints",
            label: "Checkpoints",
            count: checkpoints.length || undefined,
          },
          {
            id: "access",
            label: "Access",
            count: credentials.length || undefined,
          },
          { id: "health", label: "Health" },
        ]}
        active={activeTab}
        onChange={setActiveTab}
      />

      <TabPanel id="changes" active={activeTab}>
        <div className="metric-grid">
          <Metric label="Workspace generation" value={generation} />
          <Metric label="Recent changes" value={changes.length} />
          <Metric label="Checkpoints loaded" value={checkpoints.length} />
          <Metric
            label="Change feed"
            value={
              <StatusBadge status={changesQuery.data?.status ?? "checking"} />
            }
          />
        </div>
        <Section
          title="Recent workspace changes"
          actions={<Activity size={18} aria-hidden="true" />}
        >
          {manifestQuery.isPending || changesQuery.isPending ? (
            <LoadingState label="Loading changes" />
          ) : null}
          {manifestQuery.isError ? (
            <ErrorState
              error={manifestQuery.error}
              retry={() => void manifestQuery.refetch()}
            />
          ) : null}
          {changesQuery.isError ? (
            <ErrorState
              error={changesQuery.error}
              retry={() => void changesQuery.refetch()}
            />
          ) : null}
          {changesQuery.isSuccess ? (
            <DataTable
              data={changes}
              columns={changeColumns}
              emptyTitle="No recent changes"
            />
          ) : null}
        </Section>
      </TabPanel>

      <TabPanel id="usage" active={activeTab}>
        <Section title="Entry usage">
          <div className="mode-control" aria-label="Usage ranking">
            {(
              [
                ["most_used", "Most used"],
                ["least_used", "Least used"],
                ["most_recently_used", "Most recent"],
                ["least_recently_used", "Least recent"],
              ] as const
            ).map(([value, label]) => (
              <button
                type="button"
                key={value}
                className={usageRanking === value ? "active" : undefined}
                onClick={() => setUsageRanking(value)}
              >
                {label}
              </button>
            ))}
          </div>
          {usageQuery.isPending ? (
            <LoadingState label="Loading usage" />
          ) : null}
          {usageQuery.isError ? (
            <ErrorState
              error={usageQuery.error}
              retry={() => void usageQuery.refetch()}
            />
          ) : null}
          {usageQuery.isSuccess ? (
            <DataTable
              data={usageEntries}
              columns={usageColumns}
              emptyTitle="No usage recorded"
            />
          ) : null}
        </Section>
      </TabPanel>

      <TabPanel id="checkpoints" active={activeTab}>
        <div className="workspace-split">
          <Section
            title="Checkpoint Markdown"
            actions={<FileClock size={18} aria-hidden="true" />}
          >
            {manifestQuery.isPending ? (
              <LoadingState label="Loading checkpoints" />
            ) : null}
            {manifestQuery.isError ? (
              <ErrorState
                error={manifestQuery.error}
                retry={() => void manifestQuery.refetch()}
              />
            ) : null}
            {manifestQuery.isSuccess ? (
              <DataTable
                data={checkpoints}
                columns={checkpointColumns}
                onRowClick={setSelectedCheckpoint}
                getRowLabel={(checkpoint) =>
                  `Read checkpoint ${checkpoint.title}`
                }
                emptyTitle="No checkpoints"
              />
            ) : null}
          </Section>
          <Section title="Selected checkpoint">
            {selectedCheckpoint && checkpointQuery.isPending ? (
              <LoadingState label="Reading checkpoint" />
            ) : null}
            {checkpointQuery.isError ? (
              <ErrorState
                error={checkpointQuery.error}
                retry={() => void checkpointQuery.refetch()}
              />
            ) : null}
            {selectedCheckpointEntry ? (
              <WorkspaceEntryView entry={selectedCheckpointEntry} />
            ) : !selectedCheckpoint ? (
              <EmptyState title="No checkpoint selected" />
            ) : null}
          </Section>
        </div>
      </TabPanel>

      <TabPanel id="access" active={activeTab}>
        <Section
          title="API credentials"
          actions={
            <button
              className="button primary"
              type="button"
              onClick={() => setCreateOpen((value) => !value)}
              disabled={!canManageCredentials}
              title={
                !canManageCredentials
                  ? "Requires an owner credential"
                  : undefined
              }
            >
              <Plus size={17} aria-hidden="true" />
              New credential
            </button>
          }
        >
          {createOpen ? (
            <form className="form-grid inset-form" onSubmit={submitCredential}>
              <label className="field">
                <span>Name</span>
                <input
                  value={credentialName}
                  onChange={(event) => setCredentialName(event.target.value)}
                  required
                />
              </label>
              <label className="field">
                <span>Access</span>
                <select
                  value={credentialAccess}
                  onChange={(event) =>
                    setCredentialAccess(
                      event.target.value as "read_only" | "read_write",
                    )
                  }
                >
                  <option value="read_only">Read only</option>
                  <option value="read_write">Read/write</option>
                </select>
              </label>
              <div className="form-actions field-span-2">
                <button
                  className="button primary"
                  type="submit"
                  disabled={
                    !credentialName.trim() ||
                    createCredentialMutation.isPending
                  }
                >
                  <KeyRound size={17} aria-hidden="true" />
                  Create
                </button>
              </div>
            </form>
          ) : null}
          {createdCredential?.token ? (
            <div className="secret-once" role="status">
              <div>
                <strong>Credential created</strong>
                <span>This token is displayed once.</span>
              </div>
              <code>{createdCredential.token}</code>
              <button
                className="icon-button"
                type="button"
                onClick={() => void copyToken(createdCredential)}
                aria-label="Copy token"
                title="Copy token"
              >
                {copied ? (
                  <Check size={17} aria-hidden="true" />
                ) : (
                  <Copy size={17} aria-hidden="true" />
                )}
              </button>
            </div>
          ) : null}
          {createCredentialMutation.isError ? (
            <ErrorState
              error={createCredentialMutation.error}
              title="Credential creation failed"
            />
          ) : null}
          {credentialsQuery.isPending ? (
            <LoadingState label="Loading credentials" />
          ) : null}
          {credentialsQuery.isError ? (
            <ErrorState
              error={credentialsQuery.error}
              retry={() => void credentialsQuery.refetch()}
            />
          ) : null}
          {credentialsQuery.isSuccess && !credentials.length ? (
            <EmptyState title="No credentials" />
          ) : null}
          {credentials.length ? (
            <div className="credential-list">
              {credentials.map((credential) => (
                <article key={credential.id}>
                  <div className="credential-icon">
                    <KeyRound size={18} aria-hidden="true" />
                  </div>
                  <div>
                    <strong>{credential.name}</strong>
                    <code>{shortId(credential.id, 14)}</code>
                    <span>{credential.access.replace("_", " ")}</span>
                  </div>
                  <StatusBadge
                    status={
                      credential.revoked_at ? "revoked" : credential.access
                    }
                  />
                  <div>
                    <span>Last used</span>
                    <strong>{formatDate(credential.last_used_at)}</strong>
                  </div>
                  <button
                    className="icon-button danger-icon"
                    type="button"
                    onClick={() => revokeMutation.mutate(credential.id)}
                    disabled={
                      !canManageCredentials ||
                      Boolean(credential.revoked_at) ||
                      revokeMutation.isPending
                    }
                    aria-label={`Revoke ${credential.name}`}
                    title="Revoke"
                  >
                    <Ban size={17} aria-hidden="true" />
                  </button>
                </article>
              ))}
            </div>
          ) : null}
        </Section>
      </TabPanel>

      <TabPanel id="health" active={activeTab}>
        {statusQuery.isPending ? (
          <LoadingState label="Loading service health" />
        ) : null}
        {statusQuery.isError ? (
          <ErrorState
            error={statusQuery.error}
            retry={() => void statusQuery.refetch()}
          />
        ) : null}
        {statusQuery.data ? (
          <>
            <div className="metric-grid">
              <Metric
                label="Service"
                value={<StatusBadge status={statusQuery.data.data.status} />}
                detail={statusQuery.data.data.version}
              />
              <Metric
                label="Reported revision"
                value={
                  <code>
                    {shortId(statusQuery.data.data.corpus_revision)}
                  </code>
                }
              />
              <Metric
                label="Dependencies"
                value={
                  Object.keys(statusQuery.data.data.dependencies ?? {}).length
                }
              />
              <Metric
                label="Indexes"
                value={Object.keys(statusQuery.data.data.indexes ?? {}).length}
              />
            </div>
            <div className="two-column-layout">
              <Section
                title="Dependencies"
                actions={<Activity size={18} aria-hidden="true" />}
              >
                {Object.entries(
                  statusQuery.data.data.dependencies ?? {},
                ).length ? (
                  <div className="health-list">
                    {Object.entries(
                      statusQuery.data.data.dependencies ?? {},
                    ).map(([name, dependency]) => (
                      <div key={name}>
                        <div>
                          <strong>{humanize(name)}</strong>
                          <span>
                            {dependency.detail ??
                              `${dependency.latency_ms ?? 0} ms`}
                          </span>
                        </div>
                        <StatusBadge status={dependency.status} />
                      </div>
                    ))}
                  </div>
                ) : (
                  <EmptyState title="No dependency details" />
                )}
              </Section>
              <Section
                title="Indexes"
                actions={<RefreshCw size={18} aria-hidden="true" />}
              >
                {Object.entries(statusQuery.data.data.indexes ?? {}).length ? (
                  <div className="health-list">
                    {Object.entries(statusQuery.data.data.indexes ?? {}).map(
                      ([name, index]) => (
                        <div key={name}>
                          <div>
                            <strong>{humanize(name)}</strong>
                            <span>{formatDate(index.updated_at)}</span>
                          </div>
                          <StatusBadge status={index.status} />
                        </div>
                      ),
                    )}
                  </div>
                ) : (
                  <EmptyState title="No index details" />
                )}
              </Section>
            </div>
          </>
        ) : null}
      </TabPanel>
    </Page>
  );
}
