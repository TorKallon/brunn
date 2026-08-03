import { useMutation } from "@tanstack/react-query";
import {
  CircleCheck,
  FileText,
  Search,
  Sparkles,
  TextSearch,
} from "lucide-react";
import { type FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { useSearch } from "@tanstack/react-router";
import {
  WorkspaceEntryView,
  type WorkspaceEntryNavigationTarget,
} from "../components/WorkspaceEntryView";
import { Page, PageHeader, Section } from "../components/Page";
import {
  EmptyState,
  ErrorState,
  ProtocolNotice,
  StatusBadge,
} from "../components/StateViews";
import { TabPanel, Tabs } from "../components/Tabs";
import { useApi } from "../lib/auth";
import { ApiError } from "../lib/api";
import { formatDate } from "../lib/format";
import type {
  WorkspaceSearchCandidate,
  WorkspaceSearchSort,
} from "../lib/types";
import { workspaceEntryRef } from "../lib/workspace";

const retrievalModes = [
  { id: "exact", label: "Exact", icon: CircleCheck },
  { id: "lexical", label: "Lexical", icon: TextSearch },
  { id: "semantic", label: "Semantic", icon: Sparkles },
] as const;

type ReadTarget = {
  ref?: string;
  path?: string;
  link_target?: string;
  version?: number;
};
type SearchRequest = {
  query: string;
  modes: string[];
  limit: number;
  sort: WorkspaceSearchSort;
};
type ReadRequest = {
  target: ReadTarget;
  alternatePaths?: string[];
  view: string;
  start?: number;
  end?: number;
  fallbackQuery?: string;
};

const sortOptions: Array<{ id: WorkspaceSearchSort; label: string }> = [
  { id: "best_match", label: "Best match" },
  { id: "last_modified", label: "Last modified" },
  { id: "title", label: "Title" },
];

function candidateModifiedTime(candidate: WorkspaceSearchCandidate): number {
  if (!candidate.updated_at) return Number.NEGATIVE_INFINITY;
  const value = Date.parse(candidate.updated_at);
  return Number.isNaN(value) ? Number.NEGATIVE_INFINITY : value;
}

function sortCandidates(
  candidates: WorkspaceSearchCandidate[],
  sort: WorkspaceSearchSort,
): WorkspaceSearchCandidate[] {
  if (
    sort === "best_match" ||
    (candidates.length > 0 && candidates.every((candidate) => candidate.updated_at))
  ) {
    return candidates;
  }
  return candidates
    .map((candidate, index) => ({ candidate, index }))
    .sort((left, right) => {
      const title = left.candidate.title.localeCompare(right.candidate.title, undefined, {
        sensitivity: "base",
      });
      if (sort === "title") return title || left.index - right.index;
      const leftModified = candidateModifiedTime(left.candidate);
      const rightModified = candidateModifiedTime(right.candidate);
      if (leftModified !== rightModified) return rightModified - leftModified;
      return left.index - right.index;
    })
    .map(({ candidate }) => candidate);
}

function linkedEntryTargetFromSearch(
  search: {
    entryRef?: string;
    entryPath?: string;
    alternatePaths?: string;
    linkTarget?: string;
    fallbackQuery?: string;
  },
): WorkspaceEntryNavigationTarget | undefined {
  const ref = search.entryRef;
  const path = search.entryPath;
  const alternatePaths = search.alternatePaths?.split("\n").filter(Boolean) ?? [];
  const linkTarget = search.linkTarget;
  const fallbackQuery = search.fallbackQuery;
  if (!ref && !path && !linkTarget) return undefined;
  return {
    ref,
    path,
    alternatePaths: alternatePaths.length ? alternatePaths : undefined,
    linkTarget,
    fallbackQuery,
  };
}

export function ExplorePage() {
  const api = useApi();
  const entryLinkSearch = useSearch({ from: "/authenticated/explore" });
  const handledEntryLink = useRef("");
  const [tab, setTab] = useState("search");
  const [query, setQuery] = useState("");
  const [modes, setModes] = useState<string[]>([
    "exact",
    "lexical",
    "semantic",
  ]);
  const [limit, setLimit] = useState(20);
  const [sort, setSort] = useState<WorkspaceSearchSort>("best_match");
  const [readTarget, setReadTarget] = useState("");
  const [readView, setReadView] = useState("full");
  const [rangeStart, setRangeStart] = useState(1);
  const [rangeEnd, setRangeEnd] = useState(200);

  const searchMutation = useMutation({
    mutationFn: (request: SearchRequest) =>
      api.workspaceSearch({
        queries: [
          {
            id: "workspace-search",
            goal: "Find current source material",
            ...request,
          },
        ],
      }),
  });

  const readMutation = useMutation({
    mutationFn: async (request: ReadRequest) => {
      const read = (target: ReadTarget) =>
        api.workspaceRead({
          requests: [
            {
              ...target,
              view: request.view,
              ...(request.view === "range"
                ? { start: request.start, end: request.end }
                : {}),
            },
          ],
        });

      const exactTargets: ReadTarget[] = [
        ...(request.target.ref
          ? [{ ref: request.target.ref, version: request.target.version }]
          : []),
        ...(request.target.path
          ? [{ path: request.target.path, version: request.target.version }]
          : []),
        ...(request.alternatePaths ?? []).map((path) => ({
          path,
          version: request.target.version,
        })),
      ];
      for (const target of exactTargets) {
        try {
          const response = await read(target);
          const item = response.data.items[0];
          if (item && item.status !== "not_found" && typeof item.text === "string") {
            return response;
          }
        } catch (error) {
          const mayResolve =
            error instanceof ApiError &&
            (error.status === 400 || error.status === 404);
          if (!mayResolve) throw error;
        }
      }

      if (request.target.link_target) {
        const response = await read({ link_target: request.target.link_target });
        const item = response.data.items[0];
        if (item && item.status !== "not_found" && typeof item.text === "string") {
          return response;
        }
      }

      const label = request.fallbackQuery ?? request.target.path ?? request.target.ref
        ?? request.target.link_target
        ?? "the linked entry";
      throw new Error(
        `No exact entry matches ${label}. Use the entry's full path to avoid an ambiguous link.`,
      );
    },
  });

  const resultSets = searchMutation.isPending
    ? []
    : searchMutation.data?.data.results ?? [];
  const displayedSort = searchMutation.data
    ? searchMutation.variables?.sort ?? "best_match"
    : sort;
  const serverCandidates = useMemo(
    () => resultSets.flatMap((result) => result.candidates),
    [resultSets],
  );
  const candidates = useMemo(
    () => sortCandidates(serverCandidates, displayedSort),
    [serverCandidates, displayedSort],
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
    runSearch(sort);
  }

  function runSearch(nextSort: WorkspaceSearchSort) {
    const value = query.trim();
    if (!value || !modes.length) return;
    searchMutation.mutate({ query: value, modes, limit, sort: nextSort });
  }

  function changeSort(nextSort: WorkspaceSearchSort) {
    if (searchMutation.isPending) return;
    setSort(nextSort);
    if (searchMutation.data) runSearch(nextSort);
  }

  function submitRead(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const target = readTarget.trim();
    if (!target) return;
    readMutation.mutate({
      target: target.startsWith("entry:") ? { ref: target } : { path: target },
      view: readView,
      ...(readView === "range" ? { start: rangeStart, end: rangeEnd } : {}),
    });
  }

  function readCandidate(candidate: WorkspaceSearchCandidate) {
    const ref = workspaceEntryRef(candidate);
    setReadTarget(ref || candidate.path);
    setReadView("full");
    setTab("read");
    readMutation.mutate({
      target: ref
        ? { ref, version: candidate.version }
        : { path: candidate.path, version: candidate.version },
      alternatePaths: ref ? [candidate.path] : undefined,
      view: "full",
      fallbackQuery: candidate.path,
    });
  }

  function readLinkedEntry(target: WorkspaceEntryNavigationTarget) {
    setReadTarget(
      target.ref ?? target.path ?? target.linkTarget ?? target.fallbackQuery ?? "",
    );
    setReadView("full");
    setTab("read");
    readMutation.mutate({
      target: {
        ref: target.ref,
        path: target.path,
        link_target: target.linkTarget,
      },
      alternatePaths: target.alternatePaths,
      view: "full",
      fallbackQuery: target.fallbackQuery,
    });
  }

  const entryLinkKey = JSON.stringify(entryLinkSearch);
  useEffect(() => {
    if (handledEntryLink.current === entryLinkKey) return;
    const target = linkedEntryTargetFromSearch(entryLinkSearch);
    if (!target) return;
    handledEntryLink.current = entryLinkKey;
    readLinkedEntry(target);
  }, [entryLinkKey]);

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
              <div className="search-result-controls">
                <label className="compact-field">
                  <span>Sort</span>
                  <select
                    aria-label="Sort results"
                    value={sort}
                    disabled={searchMutation.isPending}
                    onChange={(event) =>
                      changeSort(event.target.value as WorkspaceSearchSort)
                    }
                  >
                    {sortOptions.map((option) => (
                      <option value={option.id} key={option.id}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
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
            retry={() => runSearch(sort)}
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
            title="Results"
            meta={`${candidates.length} entries · ${sortOptions.find((option) => option.id === displayedSort)?.label ?? "Best match"}`}
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
                      <h3>
                        <button
                          className="result-entry-title"
                          type="button"
                          onClick={() => readCandidate(candidate)}
                        >
                          {candidate.title}
                        </button>
                      </h3>
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
                    {candidate.updated_at ? (
                      <span>Modified {formatDate(candidate.updated_at)}</span>
                    ) : null}
                    {(candidate.lanes ?? []).map((lane) => (
                      <StatusBadge status={lane} key={lane} />
                    ))}
                    <button
                      className="button secondary"
                      type="button"
                      onClick={() => readCandidate(candidate)}
                    >
                      <FileText size={16} aria-hidden="true" />
                      Open entry
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
          <WorkspaceEntryView
            entry={selectedEntry}
            onEntryLink={readLinkedEntry}
          />
        </Section>
      ) : null}
    </Page>
  );
}
