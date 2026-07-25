import { useInfiniteQuery, useMutation, useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import type { ColumnDef } from "@tanstack/react-table";
import {
  ChevronsDown,
  Download,
  FileArchive,
  FolderOpen,
  X,
} from "lucide-react";
import { useMemo, useState } from "react";
import { DataTable } from "../components/DataTable";
import { JsonView } from "../components/JsonView";
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
  ProtocolNotice,
  StatusBadge,
} from "../components/StateViews";
import { useApi } from "../lib/auth";
import {
  formatBytes,
  formatDate,
  formatRelative,
  humanize,
  shortId,
} from "../lib/format";
import type {
  AssetAccessTelemetry,
  AssetRecord,
  AssetSummary,
  SessionSummary,
} from "../lib/types";
import { asItems } from "../lib/types";

const ASSET_PAGE_SIZE = 100;

export function AssetsPage({ sessionId }: { sessionId?: string }) {
  return sessionId ? (
    <PinnedAssetsPage sessionId={sessionId} />
  ) : (
    <AssetSessionRequired />
  );
}

function AssetSessionRequired() {
  const api = useApi();
  const navigate = useNavigate();
  const sessionsQuery = useInfiniteQuery({
    queryKey: ["sessions", "asset-picker"],
    queryFn: ({ pageParam }) => api.sessions(pageParam || undefined),
    initialPageParam: "",
    getNextPageParam: (lastPage) =>
      Array.isArray(lastPage.data)
        ? undefined
        : lastPage.data.continuation_token ?? undefined,
  });
  const sessions =
    sessionsQuery.data?.pages.flatMap((page) => asItems(page.data)) ?? [];
  const columns = useMemo<ColumnDef<SessionSummary, unknown>[]>(
    () => [
      {
        accessorKey: "title",
        header: "Session",
        cell: ({ row }) => (
          <div className="primary-cell">
            <strong>{row.original.title ?? "Untitled workspace"}</strong>
            <code>{row.original.id}</code>
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
        header: "Pinned revision",
        cell: ({ getValue }) => (
          <code title={getValue<string>()}>{shortId(getValue<string>())}</code>
        ),
      },
      {
        accessorKey: "updated_at",
        header: "Updated",
        cell: ({ getValue }) => formatRelative(getValue<string>()),
      },
    ],
    [],
  );

  return (
    <Page>
      <PageHeader
        title="Assets"
        description="Native files and their projected audit records"
      />
      <Section title="Pinned session required">
        {sessionsQuery.isPending ? (
          <LoadingState label="Loading pinned sessions" />
        ) : null}
        {sessionsQuery.isError ? (
          <ErrorState
            error={sessionsQuery.error}
            retry={() => void sessionsQuery.refetch()}
            title="Unable to load sessions"
          />
        ) : null}
        {sessionsQuery.isSuccess && !sessions.length ? (
          <EmptyState
            title="No pinned sessions"
            detail="Open a workspace before auditing its assets."
            action={
              <Link className="button secondary" to="/work">
                <FolderOpen size={16} aria-hidden="true" />
                Open workspace
              </Link>
            }
          />
        ) : null}
        {sessions.length ? (
          <>
            <DataTable
              data={sessions}
              columns={columns}
              onRowClick={(session) =>
                void navigate({
                  to: "/assets/$sessionId",
                  params: { sessionId: session.id },
                })
              }
              getRowLabel={(session) =>
                `Audit assets pinned to ${session.title ?? session.id}`
              }
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
        ) : null}
      </Section>
    </Page>
  );
}

function PinnedAssetsPage({ sessionId }: { sessionId: string }) {
  const api = useApi();
  const [selected, setSelected] = useState<AssetSummary | null>(null);
  const sessionQuery = useQuery({
    queryKey: ["session", sessionId],
    queryFn: () => api.session(sessionId),
  });
  const assetsQuery = useInfiniteQuery({
    queryKey: ["assets", sessionId],
    queryFn: ({ pageParam }) =>
      api.assets(sessionId, pageParam, ASSET_PAGE_SIZE),
    initialPageParam: 0,
    getNextPageParam: (lastPage) => {
      if (!lastPage.data.has_more) return undefined;
      return (
        (lastPage.data.offset ?? 0) +
        (lastPage.data.items?.length ?? ASSET_PAGE_SIZE)
      );
    },
  });
  const metadataQuery = useQuery({
    queryKey: ["asset", sessionId, selected?.asset_ref],
    queryFn: () => api.asset(selected!.asset_ref, sessionId),
    enabled: Boolean(selected),
  });
  const downloadMutation = useMutation({
    mutationFn: (asset: AssetSummary) =>
      api.downloadAsset(
        asset.asset_ref,
        asset.version,
        sessionId,
        asset.content_hash,
        asset.size_bytes,
      ),
    onSuccess: (download, asset) => {
      const url = URL.createObjectURL(download.blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = download.filename ?? assetName(asset);
      anchor.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 0);
    },
  });

  const assets =
    assetsQuery.data?.pages.flatMap((page) => page.data.items ?? []) ?? [];
  const firstPage = assetsQuery.data?.pages[0];
  const loadedBytes = assets.reduce(
    (total, asset) => total + asset.size_bytes,
    0,
  );
  const described = assets.filter((asset) =>
    descriptionFor(asset).available,
  ).length;
  const accessed = assets.filter((asset) =>
    hasAccessTelemetry(accessFor(asset)),
  ).length;
  const total = firstPage?.data.total ?? assets.length;
  const session = sessionQuery.data?.data;
  const detail = metadataQuery.data?.data;
  const columns: ColumnDef<AssetSummary, unknown>[] = [
    {
      id: "asset",
      header: "Asset",
      cell: ({ row }) => (
        <div className="asset-primary">
          <FileArchive size={17} aria-hidden="true" />
          <div>
            <strong>{assetName(row.original)}</strong>
            <span title={row.original.path ?? undefined}>
              {row.original.path ?? row.original.asset_ref}
            </span>
          </div>
        </div>
      ),
    },
    {
      accessorKey: "media_type",
      header: "Media type",
      cell: ({ getValue }) => humanizeMediaType(getValue<string>()),
    },
    {
      accessorKey: "size_bytes",
      header: "Size",
      cell: ({ getValue }) => formatBytes(getValue<number>()),
    },
    {
      id: "version",
      header: "Version",
      cell: ({ row }) => (
        <div className="asset-version">
          <strong>v{row.original.version}</strong>
          <code title={row.original.content_hash}>
            {shortId(row.original.content_hash, 14)}
          </code>
        </div>
      ),
    },
    {
      id: "description",
      header: "Description",
      cell: ({ row }) => {
        const description = descriptionFor(row.original);
        return (
          <div className="asset-description">
            <StatusBadge status={description.status} />
            {description.method ? (
              <span>{humanize(description.method)}</span>
            ) : null}
          </div>
        );
      },
    },
    {
      id: "access",
      header: "Access",
      cell: ({ row }) => <AssetAccess value={accessFor(row.original)} />,
    },
    {
      id: "download",
      header: "Download",
      cell: ({ row }) => {
        const downloading =
          downloadMutation.isPending &&
          downloadMutation.variables?.asset_ref === row.original.asset_ref;
        return (
          <button
            className="icon-button"
            type="button"
            onClick={(event) => {
              event.stopPropagation();
              downloadMutation.mutate(row.original);
            }}
            disabled={downloading}
            aria-label={`Download ${assetName(row.original)}`}
            title="Download"
          >
            <Download size={17} aria-hidden="true" />
          </button>
        );
      },
    },
  ];

  return (
    <Page>
      <PageHeader
        title="Assets"
        description={`Pinned session ${sessionId}`}
        actions={
          <>
            <Link
              className="button secondary"
              to="/assets"
              activeOptions={{ exact: true }}
            >
              Change session
            </Link>
            <Link
              className="button secondary"
              to="/sessions/$sessionId"
              params={{ sessionId }}
            >
              <FolderOpen size={16} aria-hidden="true" />
              Workspace
            </Link>
          </>
        }
      />
      <ProtocolNotice
        status={firstPage?.status}
        gaps={firstPage?.gaps?.length}
        conflicts={firstPage?.conflicts?.length}
      />

      <div className="metric-grid">
        <Metric
          label="Projected assets"
          value={total}
          detail={`${assets.length} loaded`}
        />
        <Metric
          label="Loaded bytes"
          value={formatBytes(loadedBytes)}
          detail="Current result set"
        />
        <Metric
          label="Descriptions"
          value={described}
          detail={`${Math.max(0, assets.length - described)} unavailable`}
        />
        <Metric
          label="Access telemetry"
          value={accessed}
          detail={`${Math.max(0, assets.length - accessed)} not returned`}
        />
      </div>

      {sessionQuery.isPending ? (
        <LoadingState label="Loading pinned session" />
      ) : null}
      {sessionQuery.isError ? (
        <Section title="Pinned session">
          <ErrorState
            error={sessionQuery.error}
            retry={() => void sessionQuery.refetch()}
            title="Unable to load session metadata"
          />
        </Section>
      ) : null}
      {session ? (
        <Section title="Pinned projection">
          <DefinitionList
            items={[
              {
                label: "Session",
                value: session.title ?? session.id,
              },
              {
                label: "Revision",
                value: (
                  <code title={session.corpus_revision}>
                    {shortId(session.corpus_revision)}
                  </code>
                ),
              },
              {
                label: "Scope",
                value: session.scope_id ?? "Authorized scope",
              },
              {
                label: "Updated",
                value: formatDate(session.updated_at),
              },
            ]}
          />
        </Section>
      ) : null}

      <Section
        title="Projected assets"
        meta={assets.length ? `${assets.length} of ${total}` : undefined}
      >
        {assetsQuery.isPending ? (
          <LoadingState label="Loading projected assets" />
        ) : null}
        {assetsQuery.isError ? (
          <ErrorState
            error={assetsQuery.error}
            retry={() => void assetsQuery.refetch()}
            title="Unable to load assets"
          />
        ) : null}
        {assetsQuery.isSuccess && !assets.length ? (
          <EmptyState
            title="No projected assets"
            detail="This pinned revision has no authorized native assets."
          />
        ) : null}
        {assets.length ? (
          <>
            <DataTable
              data={assets}
              columns={columns}
              onRowClick={setSelected}
              getRowLabel={(asset) => `Inspect ${assetName(asset)} metadata`}
            />
            {assetsQuery.hasNextPage ? (
              <div className="section-footer">
                <button
                  className="button secondary"
                  type="button"
                  onClick={() => void assetsQuery.fetchNextPage()}
                  disabled={assetsQuery.isFetchingNextPage}
                >
                  <ChevronsDown size={16} aria-hidden="true" />
                  {assetsQuery.isFetchingNextPage ? "Loading" : "Load more"}
                </button>
              </div>
            ) : null}
          </>
        ) : null}
      </Section>

      {downloadMutation.isError ? (
        <Section title="Download">
          <ErrorState
            error={downloadMutation.error}
            title="Download failed"
          />
        </Section>
      ) : null}

      {selected ? (
        <AssetDetail
          selected={selected}
          detail={detail}
          isPending={metadataQuery.isPending}
          error={metadataQuery.error}
          protocol={metadataQuery.data}
          retry={() => void metadataQuery.refetch()}
          close={() => setSelected(null)}
          download={() => downloadMutation.mutate(detail ?? selected)}
          downloading={
            downloadMutation.isPending &&
            downloadMutation.variables?.asset_ref === selected.asset_ref
          }
        />
      ) : null}
    </Page>
  );
}

function AssetDetail({
  selected,
  detail,
  isPending,
  error,
  protocol,
  retry,
  close,
  download,
  downloading,
}: {
  selected: AssetSummary;
  detail?: AssetRecord;
  isPending: boolean;
  error: Error | null;
  protocol?: {
    status?: string;
    gaps?: unknown[];
    conflicts?: unknown[];
  };
  retry: () => void;
  close: () => void;
  download: () => void;
  downloading: boolean;
}) {
  const asset: AssetRecord = detail
    ? { ...selected, ...detail }
    : selected;
  const description = descriptionFor(asset);
  const access = accessFor(asset);

  return (
    <Section
      title="Asset metadata"
      meta={assetName(asset)}
      actions={
        <>
          <button
            className="icon-button"
            type="button"
            onClick={download}
            disabled={downloading}
            aria-label={`Download ${assetName(asset)}`}
            title="Download"
          >
            <Download size={17} aria-hidden="true" />
          </button>
          <button
            className="icon-button"
            type="button"
            onClick={close}
            aria-label="Close asset metadata"
            title="Close"
          >
            <X size={17} aria-hidden="true" />
          </button>
        </>
      }
    >
      {isPending ? <LoadingState label="Loading asset metadata" /> : null}
      {error ? (
        <ErrorState
          error={error}
          retry={retry}
          title="Unable to load asset metadata"
        />
      ) : null}
      {detail ? (
        <>
          <ProtocolNotice
            status={protocol?.status}
            gaps={protocol?.gaps?.length}
            conflicts={protocol?.conflicts?.length}
          />
          <DefinitionList
            items={[
              { label: "Path", value: detail.path ?? detail.name ?? "Not returned" },
              { label: "Asset", value: <code>{detail.asset_ref}</code> },
              { label: "Media type", value: detail.media_type },
              { label: "Size", value: formatBytes(detail.size_bytes) },
              { label: "Version", value: detail.version },
              {
                label: "Hash",
                value: <code>{detail.content_hash}</code>,
              },
              {
                label: "Revision",
                value: <code>{detail.corpus_revision ?? "Not returned"}</code>,
              },
              { label: "Stored", value: formatDate(detail.stored_at) },
              {
                label: "Description",
                value: <StatusBadge status={description.status} />,
              },
              {
                label: "Description method",
                value: description.method
                  ? humanize(description.method)
                  : "Not returned",
              },
              {
                label: "Access count",
                value: access.access_count ?? "Not returned",
              },
              {
                label: "Last access",
                value: access.last_accessed_at
                  ? formatDate(access.last_accessed_at)
                  : "Not returned",
              },
              {
                label: "Most-used rank",
                value: formatRank(access.most_used_rank),
              },
              {
                label: "Least-used rank",
                value: formatRank(access.least_used_rank),
              },
              {
                label: "Least-recent rank",
                value: formatRank(access.least_recently_used_rank),
              },
            ]}
          />
          {detail.sources?.length ? (
            <div className="asset-source-list">
              {detail.sources.map((source) => (
                <div key={`${source.role}:${source.source_ref}`}>
                  <StatusBadge status={source.role} />
                  <div>
                    <Link
                      to="/sources/$sourceId"
                      params={{ sourceId: stripRef(source.source_ref) }}
                    >
                      {source.title ?? source.path ?? source.source_ref}
                    </Link>
                    <span>
                      {source.source_kind
                        ? humanize(source.source_kind)
                        : source.source_ref}
                    </span>
                  </div>
                </div>
              ))}
            </div>
          ) : null}
          {detail.metadata ? (
            <JsonView value={detail.metadata} label="Stored asset metadata" />
          ) : null}
        </>
      ) : null}
    </Section>
  );
}

function AssetAccess({ value }: { value: AssetAccessTelemetry }) {
  if (!hasAccessTelemetry(value)) return <span className="muted">Not returned</span>;
  return (
    <div className="asset-access">
      {value.access_count !== undefined ? (
        <strong>{value.access_count} access{value.access_count === 1 ? "" : "es"}</strong>
      ) : null}
      {value.last_accessed_at ? (
        <span>Last {formatRelative(value.last_accessed_at)}</span>
      ) : null}
      {value.most_used_rank != null ? (
        <span>Most used #{value.most_used_rank}</span>
      ) : null}
      {value.least_used_rank != null ? (
        <span>Least used #{value.least_used_rank}</span>
      ) : null}
      {value.least_recently_used_rank != null ? (
        <span>Least recent #{value.least_recently_used_rank}</span>
      ) : null}
    </div>
  );
}

function assetName(asset: Pick<AssetSummary, "asset_ref" | "name" | "path">) {
  if (asset.name) return asset.name;
  const path = asset.path?.replaceAll("\\", "/");
  return path?.split("/").filter(Boolean).at(-1) ?? asset.asset_ref;
}

function descriptionFor(asset: AssetSummary) {
  const available =
    asset.description?.available ??
    asset.description_available ??
    Boolean(asset.description_status || asset.description_method);
  return {
    available,
    status:
      asset.description?.status ??
      asset.description_status ??
      (available ? "available" : "unavailable"),
    method: asset.description?.method ?? asset.description_method,
  };
}

function accessFor(asset: AssetSummary): AssetAccessTelemetry {
  const nested = asset.telemetry ?? asset.usage ?? asset.access ?? {};
  return {
    access_count: asset.access_count ?? nested.access_count,
    metadata_access_count:
      asset.metadata_access_count ?? nested.metadata_access_count,
    download_count: asset.download_count ?? nested.download_count,
    first_accessed_at:
      asset.first_accessed_at ??
      asset.first_used_at ??
      nested.first_accessed_at ??
      nested.first_used_at,
    last_accessed_at:
      asset.last_accessed_at ??
      asset.last_used_at ??
      nested.last_accessed_at ??
      nested.last_used_at,
    first_used_at: asset.first_used_at ?? nested.first_used_at,
    last_used_at: asset.last_used_at ?? nested.last_used_at,
    session_count: asset.session_count ?? nested.session_count,
    never_used: asset.never_used ?? nested.never_used,
    most_used_rank: asset.most_used_rank ?? nested.most_used_rank,
    least_used_rank: asset.least_used_rank ?? nested.least_used_rank,
    least_recently_used_rank:
      asset.least_recently_used_rank ?? nested.least_recently_used_rank,
  };
}

function hasAccessTelemetry(value: AssetAccessTelemetry) {
  return Object.values(value).some(
    (item) => item !== undefined && item !== null,
  );
}

function humanizeMediaType(mediaType: string) {
  const [family, subtype] = mediaType.split("/", 2);
  if (!subtype) return humanize(mediaType);
  return `${family}/${subtype.replaceAll("-", " ")}`;
}

function formatRank(rank?: number | null) {
  return rank == null ? "Not returned" : `#${rank}`;
}

function stripRef(value: string) {
  return value.includes(":") ? value.slice(value.indexOf(":") + 1) : value;
}
