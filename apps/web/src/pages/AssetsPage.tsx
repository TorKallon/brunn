import { useInfiniteQuery, useMutation, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type { ColumnDef } from "@tanstack/react-table";
import {
  Binary,
  ChevronsDown,
  Download,
  FileUp,
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
  StatusBadge,
} from "../components/StateViews";
import { useApi } from "../lib/auth";
import {
  formatBytes,
  formatDate,
  formatRelative,
  shortId,
} from "../lib/format";
import type { WorkspaceBinary } from "../lib/types";
import { metadataObject } from "../lib/workspace";

const BINARY_PAGE_SIZE = 100;

export function AssetsPage() {
  const api = useApi();
  const [selected, setSelected] = useState<WorkspaceBinary | null>(null);
  const binariesQuery = useInfiniteQuery({
    queryKey: ["workspace-binaries"],
    queryFn: ({ pageParam }) =>
      api.workspaceBinaries(pageParam, BINARY_PAGE_SIZE),
    initialPageParam: 0,
    getNextPageParam: (lastPage, pages) =>
      lastPage.data.binaries.length === BINARY_PAGE_SIZE
        ? pages.reduce(
            (total, page) => total + page.data.binaries.length,
            0,
          )
        : undefined,
  });
  const metadataQuery = useQuery({
    queryKey: ["workspace-binary", selected?.entry_ref],
    queryFn: () => api.workspaceBinary(selected!.entry_ref),
    enabled: Boolean(selected),
  });
  const downloadMutation = useMutation({
    mutationFn: (binary: WorkspaceBinary) =>
      api.downloadWorkspaceBinary(
        binary.entry_ref,
        binary.content_hash,
        binary.size_bytes,
      ),
    onSuccess: (download, binary) => {
      const url = URL.createObjectURL(download.blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = download.filename ?? binaryName(binary);
      anchor.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 0);
    },
  });

  const binaries =
    binariesQuery.data?.pages.flatMap((page) => page.data.binaries) ?? [];
  const loadedBytes = binaries.reduce(
    (total, binary) => total + binary.size_bytes,
    0,
  );
  const described = binaries.filter(
    (binary) => descriptionStatus(binary) !== "pending",
  ).length;
  const detail = metadataQuery.data?.data;

  const columns = useMemo<ColumnDef<WorkspaceBinary, unknown>[]>(
    () => [
      {
        id: "binary",
        header: "Binary",
        cell: ({ row }) => (
          <div className="asset-primary">
            <Binary size={17} aria-hidden="true" />
            <div>
              <strong>{binaryName(row.original)}</strong>
              <span title={row.original.path}>{row.original.path}</span>
            </div>
          </div>
        ),
      },
      {
        accessorKey: "media_type",
        header: "Media type",
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
        cell: ({ row }) => (
          <StatusBadge status={descriptionStatus(row.original)} />
        ),
      },
      {
        accessorKey: "updated_at",
        header: "Updated",
        cell: ({ getValue }) => formatRelative(getValue<string>()),
      },
      {
        id: "download",
        header: "Download",
        cell: ({ row }) => (
          <button
            className="icon-button"
            type="button"
            onClick={(event) => {
              event.stopPropagation();
              downloadMutation.mutate(row.original);
            }}
            disabled={
              downloadMutation.isPending &&
              downloadMutation.variables?.entry_ref === row.original.entry_ref
            }
            aria-label={`Download ${binaryName(row.original)}`}
            title="Download"
          >
            <Download size={17} aria-hidden="true" />
          </button>
        ),
      },
    ],
    [downloadMutation],
  );

  return (
    <Page>
      <PageHeader
        title="Binaries"
        description="Exact files and searchable companion state"
        actions={
          <Link className="button primary" to="/capture">
            <FileUp size={17} aria-hidden="true" />
            Upload
          </Link>
        }
      />

      <div className="metric-grid">
        <Metric label="Loaded" value={binaries.length} />
        <Metric label="Loaded bytes" value={formatBytes(loadedBytes)} />
        <Metric label="Descriptions ready" value={described} />
        <Metric
          label="Descriptions pending"
          value={Math.max(0, binaries.length - described)}
        />
      </div>

      <Section
        title="Current binaries"
        meta={binaries.length ? `${binaries.length} loaded` : undefined}
      >
        {binariesQuery.isPending ? (
          <LoadingState label="Loading binaries" />
        ) : null}
        {binariesQuery.isError ? (
          <ErrorState
            error={binariesQuery.error}
            retry={() => void binariesQuery.refetch()}
            title="Unable to load binaries"
          />
        ) : null}
        {binariesQuery.isSuccess && !binaries.length ? (
          <EmptyState title="No binaries" />
        ) : null}
        {binaries.length ? (
          <>
            <DataTable
              data={binaries}
              columns={columns}
              onRowClick={setSelected}
              getRowLabel={(binary) => `Inspect ${binaryName(binary)}`}
            />
            {binariesQuery.hasNextPage ? (
              <div className="section-footer">
                <button
                  className="button secondary"
                  type="button"
                  onClick={() => void binariesQuery.fetchNextPage()}
                  disabled={binariesQuery.isFetchingNextPage}
                >
                  <ChevronsDown size={16} aria-hidden="true" />
                  {binariesQuery.isFetchingNextPage ? "Loading" : "Load more"}
                </button>
              </div>
            ) : null}
          </>
        ) : null}
      </Section>

      {downloadMutation.isError ? (
        <Section title="Download">
          <ErrorState error={downloadMutation.error} title="Download failed" />
        </Section>
      ) : null}

      {selected ? (
        <Section
          title="Binary metadata"
          meta={binaryName(selected)}
          actions={
            <>
              <button
                className="icon-button"
                type="button"
                onClick={() => downloadMutation.mutate(detail ?? selected)}
                disabled={downloadMutation.isPending}
                aria-label={`Download ${binaryName(selected)}`}
                title="Download"
              >
                <Download size={17} aria-hidden="true" />
              </button>
              <button
                className="icon-button"
                type="button"
                onClick={() => setSelected(null)}
                aria-label="Close binary metadata"
                title="Close"
              >
                <X size={17} aria-hidden="true" />
              </button>
            </>
          }
        >
          {metadataQuery.isPending ? (
            <LoadingState label="Loading binary metadata" />
          ) : null}
          {metadataQuery.isError ? (
            <ErrorState
              error={metadataQuery.error}
              retry={() => void metadataQuery.refetch()}
              title="Unable to load binary metadata"
            />
          ) : null}
          {detail ? (
            <>
              <DefinitionList
                items={[
                  { label: "Path", value: <code>{detail.path}</code> },
                  { label: "Reference", value: <code>{detail.entry_ref}</code> },
                  { label: "Media type", value: detail.media_type },
                  { label: "Size", value: formatBytes(detail.size_bytes) },
                  { label: "Version", value: `v${detail.version}` },
                  {
                    label: "Hash",
                    value: <code>{detail.content_hash}</code>,
                  },
                  {
                    label: "Description",
                    value: <StatusBadge status={descriptionStatus(detail)} />,
                  },
                  { label: "Updated", value: formatDate(detail.updated_at) },
                ]}
              />
              <JsonView value={detail.metadata} label="Stored binary metadata" />
            </>
          ) : null}
        </Section>
      ) : null}
    </Page>
  );
}

function binaryName(binary: Pick<WorkspaceBinary, "path" | "title">) {
  return (
    binary.title ||
    binary.path.replaceAll("\\", "/").split("/").filter(Boolean).at(-1) ||
    binary.path
  );
}

function descriptionStatus(binary: Pick<WorkspaceBinary, "metadata">) {
  const metadata = metadataObject(binary.metadata);
  const status = metadata.description_status;
  return typeof status === "string" ? status : "pending";
}
