import { useInfiniteQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ChevronsDown, Sunrise } from "lucide-react";
import { MarkdownView } from "../components/MarkdownView";
import { Page, PageHeader, Section } from "../components/Page";
import {
  EmptyState,
  ErrorState,
  LoadingState,
} from "../components/StateViews";
import { useApi } from "../lib/auth";
import { formatRelative, humanize } from "../lib/format";
import type { BriefingListRow } from "../lib/types";

export const BRIEFINGS_PAGE_SIZE = 14;

export function editionTitle(edition: string, date: string): string {
  return `${humanize(edition)} briefing - ${date}`;
}

/**
 * Keyset-paged briefing index, shared with the edition page so day-to-day
 * navigation reuses the same cache entry under the `['briefings']` key.
 */
export function useBriefingsIndex() {
  const api = useApi();
  return useInfiniteQuery({
    queryKey: ["briefings"],
    queryFn: ({ pageParam }: { pageParam: string | undefined }) =>
      api.briefingsList(BRIEFINGS_PAGE_SIZE, pageParam),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (lastPage) => lastPage.data.next?.after_path,
  });
}

export function BriefingsPage() {
  const indexQuery = useBriefingsIndex();
  const editions =
    indexQuery.data?.pages.flatMap((page) => page.data.editions) ?? [];

  return (
    <Page>
      <PageHeader
        title="Briefings"
        description="Daily thread of published editions"
      />
      {indexQuery.isPending ? (
        <LoadingState label="Loading briefings" />
      ) : null}
      {indexQuery.isError ? (
        <ErrorState
          error={indexQuery.error}
          retry={() => void indexQuery.refetch()}
          title="Unable to load briefings"
        />
      ) : null}
      {indexQuery.isSuccess && !editions.length ? (
        <EmptyState
          title="No briefings published"
          detail="Editions appear here once the briefing agent publishes one."
        />
      ) : null}
      {editions.length ? (
        <Section
          title="Editions"
          meta={`${editions.length} loaded`}
          actions={<Sunrise size={18} aria-hidden="true" />}
        >
          <div className="result-list">
            {editions.map((row) => (
              <BriefingListCard key={row.path} row={row} />
            ))}
          </div>
          {indexQuery.hasNextPage ? (
            <div className="section-footer">
              <button
                className="button secondary"
                type="button"
                onClick={() => void indexQuery.fetchNextPage()}
                disabled={indexQuery.isFetchingNextPage}
              >
                <ChevronsDown size={16} aria-hidden="true" />
                {indexQuery.isFetchingNextPage ? "Loading" : "Load more"}
              </button>
            </div>
          ) : null}
        </Section>
      ) : null}
    </Page>
  );
}

function BriefingListCard({ row }: { row: BriefingListRow }) {
  return (
    <Link
      to="/briefings/$date"
      params={{ date: row.date }}
      search={{ edition: row.edition }}
      className="result-card briefing-card"
    >
      <header>
        <div>
          <h3>{editionTitle(row.edition, row.date)}</h3>
        </div>
        <span>{formatRelative(row.generated_at)}</span>
      </header>
      {row.summary_md.length ? (
        <MarkdownView
          className="briefing-card-summary"
          markdown={row.summary_md[0]}
        />
      ) : null}
      <footer>
        {row.section_titles.map((title) => (
          <span className="briefing-chip" key={title}>
            {title}
          </span>
        ))}
        <span>
          {row.item_count} item{row.item_count === 1 ? "" : "s"}
        </span>
      </footer>
    </Link>
  );
}
