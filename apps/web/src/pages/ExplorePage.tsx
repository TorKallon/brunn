import { useMutation } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  Braces,
  CalendarClock,
  CircleCheck,
  Network,
  Search,
  SlidersHorizontal,
  Sparkles,
  TextSearch,
} from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";
import { JsonView } from "../components/JsonView";
import { Page, PageHeader, Section } from "../components/Page";
import { EmptyState, ErrorState, ProtocolNotice, StatusBadge } from "../components/StateViews";
import { Tabs, TabPanel } from "../components/Tabs";
import { useApi } from "../lib/auth";
import { useCurrent } from "../lib/current";
import { humanize, shortId } from "../lib/format";
import type { JsonObject, SearchResult } from "../lib/types";

const retrievalModes = [
  { id: "exact", label: "Exact", icon: CircleCheck },
  { id: "structured", label: "Structured", icon: Braces },
  { id: "lexical", label: "Lexical", icon: TextSearch },
  { id: "semantic", label: "Semantic", icon: Sparkles },
  { id: "temporal", label: "Temporal", icon: CalendarClock },
  { id: "relations", label: "Relations", icon: Network },
] as const;

const computeOperators = [
  "catalog",
  "timeline",
  "history",
  "diff",
  "group",
  "aggregate",
  "traverse",
  "resolveIdentity",
  "expandRecurrence",
  "stateHistory",
  "impact",
  "gateRollup",
];

export function ExplorePage() {
  const api = useApi();
  const current = useCurrent();
  const [tab, setTab] = useState("search");
  const [query, setQuery] = useState("");
  const [modes, setModes] = useState<string[]>(["exact", "structured", "lexical", "semantic"]);
  const [profile, setProfile] = useState("");
  const [state, setState] = useState("");
  const [authority, setAuthority] = useState("");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [operator, setOperator] = useState("timeline");
  const [subjectRef, setSubjectRef] = useState("");

  const searchMutation = useMutation({
    mutationFn: async () => {
      const opened = await api.open({
        task: `Explore the corpus for: ${query}`,
        mode: "exploration",
        hints: { authorization_scope: current.data.active_scope?.id ?? "authorized" },
        token_budget: 8_000,
      });
      const sessionId = opened.data.session_id ?? opened.session_id ?? opened.data.id;
      return api.query({
        session_id: sessionId,
        queries: [
          {
            id: "explore",
            goal: "inspect",
            query,
            scope: current.data.active_scope?.id ?? "authorized",
            modes,
            where: {
              ...(profile ? { type_profile: profile } : {}),
              ...(authority ? { authority } : {}),
              ...(from ? { valid_from: from } : {}),
              ...(to ? { valid_to: to } : {}),
            },
            ...(state ? { state_filter: { machine_ref: state, states: [] } } : {}),
            limit: 30,
          },
        ],
      });
    },
  });
  const computeMutation = useMutation({
    mutationFn: async () => {
      const opened = await api.open({
        task: `Run ${operator} for ${subjectRef}`,
        mode: "exploration",
        hints: {
          authorization_scope: current.data.active_scope?.id ?? "authorized",
          root_refs: [subjectRef],
        },
        token_budget: 4_000,
      });
      const sessionId = opened.data.session_id ?? opened.session_id ?? opened.data.id;
      return api.compute({
        session_id: sessionId,
        steps: [{ id: "explore-compute", op: operator, input: { ref: subjectRef } }],
        max_rows: 10_000,
        token_budget: 8_000,
      });
    },
  });

  const results = useMemo<SearchResult[]>(() => {
    const data = searchMutation.data?.data;
    if (!data) return [];
    if (data.results?.length) return data.results;
    return data.query_results?.flatMap((result) => result.results ?? []) ?? [];
  }, [searchMutation.data]);

  function toggleMode(mode: string) {
    setModes((active) =>
      active.includes(mode) ? active.filter((item) => item !== mode) : [...active, mode],
    );
  }

  function submitSearch(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (query.trim() && modes.length) searchMutation.mutate();
  }

  function submitCompute(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (subjectRef.trim()) computeMutation.mutate();
  }

  return (
    <Page>
      <PageHeader title="Explore" description="Search, inspect ranking, and run bounded read-only computations" />
      <Tabs
        tabs={[
          { id: "search", label: "Search", count: results.length || undefined },
          { id: "compute", label: "Compute" },
        ]}
        active={tab}
        onChange={setTab}
      />
      <TabPanel id="search" active={tab}>
        <Section title="Query" actions={<SlidersHorizontal size={18} aria-hidden="true" />}>
          <form className="search-form" onSubmit={submitSearch}>
            <div className="search-input-row">
              <Search size={19} aria-hidden="true" />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="Search authorized context"
                aria-label="Search authorized context"
              />
              <button className="button primary" type="submit" disabled={!query.trim() || !modes.length || searchMutation.isPending}>
                <Search size={17} aria-hidden="true" />
                {searchMutation.isPending ? "Searching" : "Search"}
              </button>
            </div>
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
            <div className="filter-grid">
              <label className="field"><span>Profile</span><input value={profile} onChange={(event) => setProfile(event.target.value)} placeholder="core.person" /></label>
              <label className="field"><span>State</span><input value={state} onChange={(event) => setState(event.target.value)} placeholder="work.execution.active" /></label>
              <label className="field"><span>Authority</span><input value={authority} onChange={(event) => setAuthority(event.target.value)} placeholder="source-backed" /></label>
              <label className="field"><span>From</span><input type="datetime-local" value={from} onChange={(event) => setFrom(event.target.value)} /></label>
              <label className="field"><span>To</span><input type="datetime-local" value={to} onChange={(event) => setTo(event.target.value)} /></label>
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
        {searchMutation.isError ? <ErrorState error={searchMutation.error} retry={() => searchMutation.mutate()} title="Search failed" /> : null}
        {!searchMutation.isPending && !searchMutation.data && !searchMutation.isError ? <EmptyState title="No query run" /> : null}
        {searchMutation.isSuccess && !results.length ? <EmptyState title="No results" detail={searchMutation.data.coverage?.absence_safe ? "The searched scope is complete." : "The available coverage cannot prove absence."} /> : null}
        {results.length ? (
          <Section title="Results" meta={`${results.length} selected`}>
            <div className="result-list">
              {results.map((result) => (
                <article className="result-card" key={result.id ?? result.reference ?? `${result.source_ref}:${result.path}:${result.heading}`}>
                  <header>
                    <div>
                      <StatusBadge status={result.kind ?? result.profile ?? result.reference?.split(":", 1)[0]} />
                      <h3>{result.title ?? (result.heading && result.heading !== "(preamble)" ? result.heading : null) ?? result.path ?? result.source_ref ?? result.reference ?? "Untitled result"}</h3>
                    </div>
                    {result.score !== undefined ? <strong className="score">{result.score.toFixed(3)}</strong> : null}
                  </header>
                  {result.excerpt ?? result.content ? <p>{result.excerpt ?? result.content}</p> : null}
                  {result.why_selected ? (
                    <div className="selection-reason">
                      <span>Selected</span>
                      <strong>{Array.isArray(result.why_selected) ? result.why_selected.join(" · ") : result.why_selected}</strong>
                    </div>
                  ) : null}
                  {result.lane_scores ? (
                    <div className="lane-scores">
                      {Object.entries(result.lane_scores).map(([lane, score]) => (
                        <span key={lane}>{humanize(lane)} <strong>{score.toFixed(3)}</strong></span>
                      ))}
                    </div>
                  ) : null}
                  <footer>
                    {result.object_id ?? (result.reference?.startsWith("object:") ? result.reference : null) ? <Link to="/objects/$objectId" params={{ objectId: result.object_id ?? result.reference! }}>Object {shortId(result.object_id ?? result.reference!)}</Link> : null}
                    {result.source_id ? <Link to="/sources/$sourceId" params={{ sourceId: result.source_id }}>Source {shortId(result.source_id)}</Link> : null}
                    {result.updated_at ?? result.recorded_at ? <span>{result.updated_at ?? result.recorded_at}</span> : null}
                  </footer>
                </article>
              ))}
            </div>
          </Section>
        ) : null}
      </TabPanel>

      <TabPanel id="compute" active={tab}>
        <Section title="Read-only computation">
          <form className="form-grid" onSubmit={submitCompute}>
            <label className="field"><span>Operator</span><select value={operator} onChange={(event) => setOperator(event.target.value)}>{computeOperators.map((value) => <option value={value} key={value}>{humanize(value)}</option>)}</select></label>
            <label className="field"><span>Subject reference</span><input value={subjectRef} onChange={(event) => setSubjectRef(event.target.value)} required /></label>
            <div className="form-actions field-span-2"><button className="button primary" type="submit" disabled={!subjectRef.trim() || computeMutation.isPending}><Braces size={17} />{computeMutation.isPending ? "Computing" : "Run"}</button></div>
          </form>
        </Section>
        {computeMutation.isError ? <ErrorState error={computeMutation.error} retry={() => computeMutation.mutate()} title="Computation failed" /> : null}
        {computeMutation.data ? <Section title="Result"><ProtocolNotice status={computeMutation.data.status} gaps={computeMutation.data.gaps?.length} conflicts={computeMutation.data.conflicts?.length} /><JsonView value={computeMutation.data.data} label="Computation result" /></Section> : null}
      </TabPanel>
    </Page>
  );
}
