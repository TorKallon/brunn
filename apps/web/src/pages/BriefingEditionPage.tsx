import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useState } from "react";
import { BriefingItemRow } from "../components/BriefingItemRow";
import { MarkdownView } from "../components/MarkdownView";
import { Page, PageHeader, Section } from "../components/Page";
import {
  ErrorState,
  LoadingState,
  ProtocolNotice,
  ReadOnlyNotice,
} from "../components/StateViews";
import { useApi } from "../lib/auth";
import { groupBriefingSections } from "../lib/briefingSections";
import { useReadOnly } from "../lib/current";
import { formatDate, humanize } from "../lib/format";
import { editionTitle, useBriefingsIndex } from "./BriefingsPage";

const SUMMARY_PREVIEW_COUNT = 3;

export function BriefingEditionPage() {
  const { date } = useParams({ from: "/authenticated/briefings/$date" });
  const { edition, item: targetItem } = useSearch({ from: "/authenticated/briefings/$date" });
  const api = useApi();
  const readOnly = useReadOnly();
  const navigate = useNavigate();
  const [summaryOpen, setSummaryOpen] = useState(false);
  useEffect(() => setSummaryOpen(false), [date, edition]);

  const editionQuery = useQuery({
    queryKey: ["briefings", date, edition],
    queryFn: () => api.briefingGet(date, edition),
  });
  const indexQuery = useBriefingsIndex();
  const indexRows =
    indexQuery.data?.pages.flatMap((page) => page.data.editions) ?? [];

  const data = editionQuery.data?.data;
  const briefing = data?.briefing ?? null;
  const summary = briefing?.summary_md ?? [];
  const visibleSummary = summaryOpen
    ? summary
    : summary.slice(0, SUMMARY_PREVIEW_COUNT);
  const hiddenCount = summary.length - SUMMARY_PREVIEW_COUNT;

  // The index is newest-first; the nearest older row is the previous day and
  // the last newer row is the next day.
  const older = indexRows.find((row) => row.date < date);
  const newer = [...indexRows].reverse().find((row) => row.date > date);
  const editionsForDate = indexRows.filter((row) => row.date === date);

  const generatedAt = briefing?.generated_at ?? data?.created_at;
  const lastVersion = data?.versions[data.versions.length - 1];
  const updatedAt =
    data && data.current_version > 1 ? lastVersion?.created_at : undefined;
  const headingState = data
    ? `Generated ${formatDate(generatedAt)}${
        updatedAt ? ` · Updated ${formatDate(updatedAt)}` : ""
      }`
    : undefined;

  return (
    <Page>
      <PageHeader
        title={editionTitle(edition, date)}
        description={headingState}
        actions={
          <>
            {editionsForDate.length > 1 ? (
              <div className="mode-control" aria-label="Edition">
                {editionsForDate.map((row) => (
                  <button
                    key={row.edition}
                    type="button"
                    className={row.edition === edition ? "active" : undefined}
                    onClick={() =>
                      void navigate({
                        to: "/briefings/$date",
                        params: { date },
                        search: { edition: row.edition },
                      })
                    }
                  >
                    {humanize(row.edition)}
                  </button>
                ))}
              </div>
            ) : null}
            {older ? (
              <Link
                className="button secondary"
                to="/briefings/$date"
                params={{ date: older.date }}
                search={{ edition: older.edition }}
                aria-label={`Previous briefing ${older.date}`}
              >
                <ChevronLeft size={16} aria-hidden="true" />
                {older.date}
              </Link>
            ) : null}
            {newer ? (
              <Link
                className="button secondary"
                to="/briefings/$date"
                params={{ date: newer.date }}
                search={{ edition: newer.edition }}
                aria-label={`Next briefing ${newer.date}`}
              >
                {newer.date}
                <ChevronRight size={16} aria-hidden="true" />
              </Link>
            ) : null}
          </>
        }
      />
      {readOnly ? <ReadOnlyNotice /> : null}

      {editionQuery.isPending ? (
        <LoadingState label="Loading briefing" />
      ) : null}
      {editionQuery.isError ? (
        <ErrorState
          error={editionQuery.error}
          retry={() => void editionQuery.refetch()}
          title="Unable to load briefing"
        />
      ) : null}

      {data && briefing ? (
        <div className="briefing-thread">
          {summary.length ? (
            <section className="briefing-summary" aria-label="30-second summary">
              <h2>30-second summary</h2>
              <div className="briefing-summary-list">
                {visibleSummary.map((line, index) => (
                  <MarkdownView key={index} markdown={line} />
                ))}
              </div>
              {hiddenCount > 0 ? (
                <button
                  className="button secondary"
                  type="button"
                  aria-expanded={summaryOpen}
                  onClick={() => setSummaryOpen((value) => !value)}
                >
                  {summaryOpen ? "Show less" : `${hiddenCount} more`}
                </button>
              ) : null}
            </section>
          ) : null}
          {groupBriefingSections(briefing.sections ?? []).map((section) => (
            <Section
              key={section.id}
              title={section.title}
              meta={`${section.itemCount} item${
                section.itemCount === 1 ? "" : "s"
              }`}
            >
              <div className="briefing-section-items">
                {section.parts.flatMap((part) =>
                  part.section.items.map((item) => (
                    <BriefingItemRow
                      key={item.id}
                      item={item}
                      sectionTitle={part.itemLabel}
                      topicSlug={part.section.topic}
                      editionRef={data.entry_ref}
                      readOnly={readOnly}
                      targeted={item.id === targetItem}
                    />
                  )),
                )}
              </div>
            </Section>
          ))}
        </div>
      ) : null}

      {data && !briefing ? (
        <>
          <ProtocolNotice status="structured_payload_unavailable" />
          <Section title="Briefing markdown">
            <MarkdownView markdown={data.markdown} />
          </Section>
        </>
      ) : null}
    </Page>
  );
}
