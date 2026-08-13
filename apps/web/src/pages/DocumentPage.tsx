import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { MarkdownView, type MarkdownEntryLink } from "../components/MarkdownView";
import { Page, PageHeader } from "../components/Page";
import { ErrorState, LoadingState } from "../components/StateViews";
import {
  resolveWorkspaceEntryLink,
  workspaceEntryNavigationSearch,
} from "../components/WorkspaceEntryView";
import { useApi } from "../lib/auth";
import { formatDate } from "../lib/format";
import type { PublishedDocumentSource } from "../lib/types";

export function DocumentPage() {
  const { slug } = useParams({ from: "/authenticated/documents/$slug" });
  const { version } = useSearch({ from: "/authenticated/documents/$slug" });
  const api = useApi();
  const navigate = useNavigate();
  const documentQuery = useQuery({
    queryKey: ["document", slug, version ?? "current"],
    queryFn: () => api.documentGet(slug, version),
  });
  const document = documentQuery.data?.data;
  const isHistorical = Boolean(
    document && document.version !== document.current_version,
  );
  const freshness = document
    ? document.version > 1
      ? `Published ${formatDate(document.published_at)} · Updated ${formatDate(document.updated_at)}`
      : `Published ${formatDate(document.published_at)}`
    : undefined;

  function openEntryLink(link: MarkdownEntryLink) {
    const target = resolveWorkspaceEntryLink(`Documents/${slug}.md`, link);
    if (!target) return;
    void navigate({
      to: "/explore",
      search: workspaceEntryNavigationSearch(target),
    });
  }

  return (
    <Page>
      <div className="document-reader">
        {document ? (
          <PageHeader title={document.title} description={freshness} />
        ) : null}

        {documentQuery.isPending ? (
          <LoadingState label="Loading document" />
        ) : null}
        {documentQuery.isError ? (
          <ErrorState
            error={documentQuery.error}
            retry={() => void documentQuery.refetch()}
            title="Unable to load document"
          />
        ) : null}

        {document ? (
          <>
            {isHistorical ? (
              <aside className="document-version-notice" aria-label="Historical version">
                <span>
                  Viewing version {document.version} of {document.current_version}
                </span>
                <Link
                  to="/documents/$slug"
                  params={{ slug }}
                  search={{ version: undefined }}
                >
                  Open latest
                </Link>
              </aside>
            ) : null}
            {document.summary ? (
              <p className="document-summary">{document.summary}</p>
            ) : null}
            <article className="document-surface" aria-label={`${document.title} document`}>
              <MarkdownView
                markdown={document.body_md}
                className="document-prose"
                onEntryLink={openEntryLink}
              />
              {document.sources.length ? (
                <DocumentSources sources={document.sources} />
              ) : null}
            </article>
          </>
        ) : null}
      </div>
    </Page>
  );
}

function DocumentSources({ sources }: { sources: PublishedDocumentSource[] }) {
  return (
    <section className="document-sources" aria-labelledby="document-sources-title">
      <h2 id="document-sources-title">Sources</h2>
      <ul>
        {sources.map((source, index) => (
          <li key={`${source.label}:${index}`}>
            <DocumentSourceLink source={source} />
          </li>
        ))}
      </ul>
    </section>
  );
}

function DocumentSourceLink({ source }: { source: PublishedDocumentSource }) {
  const externalUrl = safeHttpUrl(source.url);
  if (externalUrl) {
    return (
      <a href={externalUrl} target="_blank" rel="noreferrer noopener">
        {source.label}
      </a>
    );
  }
  if (source.entry_ref?.startsWith("entry:")) {
    return (
      <Link
        to="/explore"
        search={workspaceEntryNavigationSearch({ ref: source.entry_ref })}
      >
        {source.label}
      </Link>
    );
  }
  return <span>{source.label}</span>;
}

function safeHttpUrl(value?: string | null): string | undefined {
  if (!value) return undefined;
  try {
    const url = new URL(value);
    if (url.username || url.password) return undefined;
    return url.protocol === "http:" || url.protocol === "https:"
      ? url.toString()
      : undefined;
  } catch {
    return undefined;
  }
}
