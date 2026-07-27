import { useMutation } from "@tanstack/react-query";
import {
  CircleCheck,
  FileText,
  Search,
  Sparkles,
  TextSearch,
} from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";
import { WorkspaceEntryView } from "../components/WorkspaceEntryView";
import { Page, PageHeader, Section } from "../components/Page";
import {
  EmptyState,
  ErrorState,
  ProtocolNotice,
  StatusBadge,
} from "../components/StateViews";
import { TabPanel, Tabs } from "../components/Tabs";
import { useApi } from "../lib/auth";
import type { WorkspaceSearchCandidate } from "../lib/types";
import { workspaceEntryRef } from "../lib/workspace";

const retrievalModes = [
  { id: "exact", label: "Exact", icon: CircleCheck },
  { id: "lexical", label: "Lexical", icon: TextSearch },
  { id: "semantic", label: "Semantic", icon: Sparkles },
] as const;

type ReadTarget = { ref?: string; path?: string };

export function ExplorePage() {
  const api = useApi();
  const [tab, setTab] = useState("search");
  const [query, setQuery] = useState("");
  const [modes, setModes] = useState<string[]>([
    "exact",
    "lexical",
    "semantic",
  ]);
  const [limit, setLimit] = useState(20);
  const [readTarget, setReadTarget] = useState("");
  const [readView, setReadView] = useState("full");
  const [rangeStart, setRangeStart] = useState(1);
  const [rangeEnd, setRangeEnd] = useState(200);

  const searchMutation = useMutation({
    mutationFn: () =>
      api.workspaceSearch({
        queries: [
          {
            id: "workspace-search",
            goal: "Find current source material",
            query: query.trim(),
            modes,
            limit,
          },
        ],
      }),
  });

  const readMutation = useMutation({
    mutationFn: (target: ReadTarget) =>
      api.workspaceRead({
        requests: [
          {
            ...target,
            view: readView,
            ...(readView === "range"
              ? { start: rangeStart, end: rangeEnd }
              : {}),
          },
        ],
      }),
  });

  const resultSets = searchMutation.data?.data.results ?? [];
  const candidates = useMemo(
    () => resultSets.flatMap((result) => result.candidates),
    [resultSets],
  );
  const selectedEntry = readMutation.data?.data.items[0];

  function toggleMode(mode: string) {
    setModes((active) =>
      active.includes(mode)
        ? active.filter((item) => item !== mode)
        : [...active, mode],
    );
  }

  function submitSearch(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (query.trim() && modes.length) searchMutation.mutate();
  }

  function submitRead(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const target = readTarget.trim();
    if (!target) return;
    readMutation.mutate(
      target.startsWith("entry:") ? { ref: target } : { path: target },
    );
  }

  function readCandidate(candidate: WorkspaceSearchCandidate) {
    const ref = workspaceEntryRef(candidate);
    setReadTarget(ref);
    setReadView("full");
    readMutation.mutate({ ref });
  }

  return (
    <Page>
      <PageHeader title="Search" description="Current Markdown and exact reads" />
      <Tabs
        tabs={[
          { id: "search", label: "Search", count: candidates.length || undefined },
          { id: "read", label: "Exact read" },
        ]}
        active={tab}
        onChange={setTab}
      />

      <TabPanel id="search" active={tab}>
        <Section title="Search workspace">
          <form className="search-form" onSubmit={submitSearch}>
            <div className="search-input-row">
              <Search size={19} aria-hidden="true" />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                aria-label="Search workspace"
              />
              <button
                className="button primary"
                type="submit"
                disabled={!query.trim() || !modes.length || searchMutation.isPending}
              >
                <Search size={17} aria-hidden="true" />
                {searchMutation.isPending ? "Searching" : "Search"}
              </button>
            </div>
            <div className="search-options-row">
              <div className="mode-control" aria-label="Retrieval modes">
                {retrievalModes.map((mode) => {
                  const Icon = mode.icon;
                  const active = modes.includes(mode.id);
                  return (
                    <button
                      key={mode.id}
                      type="button"
                      className={active ? "active" : undefined}
                      aria-pressed={active}
                      onClick={() => toggleMode(mode.id)}
                    >
                      <Icon size={15} aria-hidden="true" />
                      {mode.label}
                    </button>
                  );
                })}
              </div>
              <label className="compact-field">
                <span>Limit</span>
                <input
                  type="number"
                  min={1}
                  max={50}
                  value={limit}
                  onChange={(event) => setLimit(Number(event.target.value))}
                />
              </label>
            </div>
          </form>
        </Section>

        {searchMutation.data ? (
          <ProtocolNotice
            status={searchMutation.data.status}
            gaps={searchMutation.data.gaps?.length}
            conflicts={searchMutation.data.conflicts?.length}
          />
        ) : null}
        {searchMutation.isError ? (
          <ErrorState
            error={searchMutation.error}
            retry={() => searchMutation.mutate()}
            title="Search failed"
          />
        ) : null}
        {!searchMutation.data &&
        !searchMutation.isPending &&
        !searchMutation.isError ? (
          <EmptyState title="No search run" />
        ) : null}
        {searchMutation.isSuccess && !candidates.length ? (
          <EmptyState title="No candidates returned" />
        ) : null}

        {candidates.length ? (
          <Section
            title="Candidates"
            meta={`${candidates.length} current entries`}
          >
            <div className="result-list">
              {candidates.map((candidate) => (
                <article
                  className="result-card"
                  key={`${workspaceEntryRef(candidate)}:${candidate.version}`}
                >
                  <header>
                    <div>
                      <StatusBadge status="markdown" />
                      <h3>{candidate.title}</h3>
                    </div>
                    {candidate.score !== undefined ? (
                      <strong className="score">
                        {candidate.score.toFixed(3)}
                      </strong>
                    ) : null}
                  </header>
                  <code className="result-path">{candidate.path}</code>
                  <p>{candidate.excerpt}</p>
                  <footer>
                    <span>v{candidate.version}</span>
                    {(candidate.lanes ?? []).map((lane) => (
                      <StatusBadge status={lane} key={lane} />
                    ))}
                    <button
                      className="button secondary"
                      type="button"
                      onClick={() => readCandidate(candidate)}
                    >
                      <FileText size={16} aria-hidden="true" />
                      Read exact
                    </button>
                  </footer>
                </article>
              ))}
            </div>
          </Section>
        ) : null}
      </TabPanel>

      <TabPanel id="read" active={tab}>
        <Section title="Read exact entry">
          <form className="form-grid" onSubmit={submitRead}>
            <label className="field field-span-2">
              <span>Path or entry reference</span>
              <input
                value={readTarget}
                onChange={(event) => setReadTarget(event.target.value)}
                spellCheck={false}
                required
              />
            </label>
            <label className="field">
              <span>View</span>
              <select
                value={readView}
                onChange={(event) => setReadView(event.target.value)}
              >
                <option value="full">Full</option>
                <option value="outline">Outline</option>
                <option value="range">Line range</option>
              </select>
            </label>
            {readView === "range" ? (
              <div className="line-range">
                <label className="field">
                  <span>Start</span>
                  <input
                    type="number"
                    min={1}
                    value={rangeStart}
                    onChange={(event) => setRangeStart(Number(event.target.value))}
                  />
                </label>
                <label className="field">
                  <span>End</span>
                  <input
                    type="number"
                    min={rangeStart}
                    value={rangeEnd}
                    onChange={(event) => setRangeEnd(Number(event.target.value))}
                  />
                </label>
              </div>
            ) : (
              <div />
            )}
            <div className="form-actions field-span-2">
              <button
                className="button primary"
                type="submit"
                disabled={!readTarget.trim() || readMutation.isPending}
              >
                <FileText size={17} aria-hidden="true" />
                {readMutation.isPending ? "Reading" : "Read"}
              </button>
            </div>
          </form>
        </Section>
      </TabPanel>

      {readMutation.isError ? (
        <ErrorState error={readMutation.error} title="Read failed" />
      ) : null}
      {selectedEntry ? (
        <Section title="Entry" meta={selectedEntry.path}>
          <WorkspaceEntryView entry={selectedEntry} />
        </Section>
      ) : null}
    </Page>
  );
}
