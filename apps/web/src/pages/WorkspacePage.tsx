import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import type { ColumnDef } from "@tanstack/react-table";
import {
  Check,
  FileArchive,
  GitCompareArrows,
  RefreshCw,
  Save,
  ShieldQuestion,
} from "lucide-react";
import { type FormEvent, useEffect, useMemo, useState } from "react";
import { DataTable } from "../components/DataTable";
import { JsonView } from "../components/JsonView";
import { DefinitionList, Metric, Page, PageHeader, Section } from "../components/Page";
import {
  EmptyState,
  ErrorState,
  LoadingState,
  ProtocolNotice,
  StatusBadge,
} from "../components/StateViews";
import { useApi } from "../lib/auth";
import { useReadOnly } from "../lib/current";
import { formatDate, shortId } from "../lib/format";
import type {
  JsonValue,
  OperationSummary,
  SessionDetail,
  VerificationResult,
} from "../lib/types";

export function WorkspacePage() {
  const { sessionId } = useParams({ from: "/sessions/$sessionId" });
  const api = useApi();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const readOnly = useReadOnly();
  const [checkpointTitle, setCheckpointTitle] = useState("");
  const [checkpointObjective, setCheckpointObjective] = useState("");
  const [checkpointCurrentState, setCheckpointCurrentState] = useState("");
  const [checkpointDecisions, setCheckpointDecisions] = useState("");
  const [checkpointQuestions, setCheckpointQuestions] = useState("");
  const [checkpointActions, setCheckpointActions] = useState("");
  const [checkpointArtifacts, setCheckpointArtifacts] = useState("");
  const [verificationClaim, setVerificationClaim] = useState("");
  const sessionQuery = useQuery({
    queryKey: ["session", sessionId],
    queryFn: () => api.session(sessionId),
  });
  const refreshMutation = useMutation({
    mutationFn: () => api.refreshSession(sessionId),
    onSuccess: (result) => {
      queryClient.setQueryData(["session", result.data.id], result);
      void navigate({
        to: "/sessions/$sessionId",
        params: { sessionId: result.data.id },
      });
    },
  });
  const checkpointMutation = useMutation({
    mutationFn: () => {
      const currentSession = sessionQuery.data?.data;
      const currentCheckpoint = currentSession?.checkpoint;
      return api.checkpoint({
        session_id: sessionId,
        parent_checkpoint_id:
          currentCheckpoint?.id ?? currentSession?.resumed_checkpoint_id ?? null,
        state: {
          ...(currentCheckpoint?.state ?? {}),
          title: checkpointTitle,
          objective: checkpointObjective || checkpointTitle,
          current_state: preserveOrParseLines(
            checkpointCurrentState,
            currentCheckpoint?.state?.current_state,
          ),
          decisions: preserveOrParseLines(
            checkpointDecisions,
            currentCheckpoint?.state?.decisions,
          ),
          open_questions: preserveOrParseLines(
            checkpointQuestions,
            currentCheckpoint?.state?.open_questions
              ?? currentCheckpoint?.state?.unresolved_gaps,
          ),
          next_actions: preserveOrParseLines(
            checkpointActions,
            currentCheckpoint?.state?.next_actions,
          ),
          artifacts: preserveOrParseLines(
            checkpointArtifacts,
            currentCheckpoint?.state?.artifacts,
          ),
        },
        source_refs: checkpointSourceRefs(currentSession),
        idempotency_key: `ui-checkpoint:${sessionId}:${checkpointTitle}:${checkpointObjective}`.slice(0, 240),
      });
    },
    onSuccess: () => {
      setCheckpointTitle("");
      setCheckpointObjective("");
      void queryClient.invalidateQueries({ queryKey: ["session", sessionId] });
      void queryClient.invalidateQueries({ queryKey: ["sessions"] });
    },
  });

  useEffect(() => {
    const checkpoint = sessionQuery.data?.data.checkpoint;
    if (!checkpoint) return;
    setCheckpointTitle(checkpoint.title ?? "");
    setCheckpointObjective(checkpoint.objective ?? "");
    setCheckpointCurrentState(formatLines(checkpoint.state?.current_state));
    setCheckpointDecisions(formatLines(checkpoint.state?.decisions));
    setCheckpointQuestions(
      formatLines(checkpoint.state?.open_questions ?? checkpoint.state?.unresolved_gaps),
    );
    setCheckpointActions(formatLines(checkpoint.state?.next_actions));
    setCheckpointArtifacts(formatLines(checkpoint.state?.artifacts));
  }, [sessionQuery.data?.data.checkpoint?.id]);
  const verifyMutation = useMutation({
    mutationFn: () =>
      api.verify({
        session_id: sessionId,
        claims: [{ id: "workspace-claim", claim: verificationClaim }],
        check_for: ["contradictions", "superseded_sources", "unsupported_claims", "temporal_ambiguity"],
      }),
  });

  const operationColumns = useMemo<ColumnDef<OperationSummary, unknown>[]>(
    () => [
      { accessorKey: "operation", header: "Operation" },
      { accessorKey: "status", header: "Status", cell: ({ getValue }) => <StatusBadge status={getValue<string>()} /> },
      { accessorKey: "result_count", header: "Results" },
      { accessorKey: "duration_ms", header: "Duration", cell: ({ getValue }) => getValue<number>() === undefined ? "-" : `${getValue<number>()} ms` },
      { accessorKey: "created_at", header: "Time", cell: ({ getValue }) => formatDate(getValue<string>()) },
    ],
    [],
  );
  const verificationColumns = useMemo<ColumnDef<VerificationResult, unknown>[]>(
    () => [
      { accessorKey: "claim", header: "Claim" },
      { accessorFn: (row) => row.classification ?? row.status, id: "classification", header: "Finding", cell: ({ getValue }) => <StatusBadge status={getValue<string>()} /> },
      { accessorKey: "explanation", header: "Explanation" },
    ],
    [],
  );

  if (sessionQuery.isPending) return <Page><LoadingState label="Opening workspace" /></Page>;
  if (sessionQuery.isError) return <Page><ErrorState error={sessionQuery.error} retry={() => void sessionQuery.refetch()} /></Page>;
  const session = sessionQuery.data.data;
  const checkpoint = session.checkpoint;
  const coverage = session.coverage ?? sessionQuery.data.coverage;
  const learnedContext = session.learned_context;
  const learnedItems = learnedContext?.items ?? [];
  const verificationResults = verifyMutation.data?.data.results ?? session.verification_results ?? [];

  function submitCheckpoint(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!readOnly && checkpointTitle.trim()) checkpointMutation.mutate();
  }

  function submitVerification(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (verificationClaim.trim()) verifyMutation.mutate();
  }

  return (
    <Page>
      <PageHeader
        title={session.title ?? "Workspace"}
        description={`Session ${session.id}`}
        actions={
          <>
            <Link
              className="button secondary"
              to="/assets/$sessionId"
              params={{ sessionId }}
            >
              <FileArchive size={17} aria-hidden="true" />
              Assets
            </Link>
            <button className="button secondary" type="button" onClick={() => refreshMutation.mutate()} disabled={refreshMutation.isPending}>
              <RefreshCw size={17} className={refreshMutation.isPending ? "spin" : undefined} />
              Refresh revision
            </button>
          </>
        }
      />
      <ProtocolNotice status={sessionQuery.data.status} gaps={sessionQuery.data.gaps?.length} conflicts={sessionQuery.data.conflicts?.length} />
      <div className="metric-grid">
        <Metric label="Pinned revision" value={<code>{shortId(session.corpus_revision)}</code>} detail={session.corpus_revision} />
        <Metric label="Checkpoint" value={checkpoint ? shortId(checkpoint.id) : "None"} detail={checkpoint?.created_at ? formatDate(checkpoint.created_at) : "Not committed"} />
        <Metric label="Evidence" value={session.initial_evidence?.length ?? 0} detail="Initially materialized" />
        <Metric label="Coverage" value={coverage?.absence_safe ? "Complete" : "Bounded"} detail={`${coverage?.searched?.length ?? 0} partitions searched`} />
      </div>

      {session.revision_delta ? (
        <Section title="Revision delta" actions={<GitCompareArrows size={18} aria-hidden="true" />}>
          <JsonView value={session.revision_delta} label="Revision delta" />
        </Section>
      ) : null}

      <div className="two-column-layout">
        <Section title="Checkpoint" meta={checkpoint?.parent_checkpoint_id ? `Parent ${shortId(checkpoint.parent_checkpoint_id)}` : undefined}>
          {checkpoint ? (
            <>
              <DefinitionList
                items={[
                  { label: "Objective", value: checkpoint.objective ?? "Not recorded" },
                  { label: "Revision", value: <code>{checkpoint.corpus_revision ?? session.corpus_revision}</code> },
                  { label: "Created", value: formatDate(checkpoint.created_at) },
                  { label: "Sources", value: checkpoint.source_refs?.length ?? 0 },
                ]}
              />
              <div className="checkpoint-groups">
                <div><h3>Goals</h3>{checkpoint.goals?.map((goal) => <div className="compact-row" key={goal.id ?? goal.title}><span>{goal.title}</span><StatusBadge status={goal.status} /></div>) ?? <span className="muted">None</span>}</div>
                <div><h3>Gates</h3>{checkpoint.gates?.map((gate) => <div className="compact-row" key={gate.id ?? gate.title}><span>{gate.title}</span><StatusBadge status={gate.status} /></div>) ?? <span className="muted">None</span>}</div>
                <div><h3>Next actions</h3>{checkpoint.next_actions?.map((action) => <div className="compact-row" key={action.id ?? action.title}><span>{action.title}</span><StatusBadge status={action.status} /></div>) ?? <span className="muted">None</span>}</div>
              </div>
            </>
          ) : <EmptyState title="No checkpoint" />}
        </Section>

        <Section title="Commit checkpoint">
          <form className="stacked-form" onSubmit={submitCheckpoint}>
            <label className="field"><span>Title</span><input value={checkpointTitle} onChange={(event) => setCheckpointTitle(event.target.value)} disabled={readOnly} required /></label>
            <label className="field"><span>Objective</span><textarea rows={4} value={checkpointObjective} onChange={(event) => setCheckpointObjective(event.target.value)} disabled={readOnly} /></label>
            <label className="field"><span>Current state</span><textarea rows={5} value={checkpointCurrentState} onChange={(event) => setCheckpointCurrentState(event.target.value)} disabled={readOnly} /></label>
            <label className="field"><span>Decisions</span><textarea rows={4} value={checkpointDecisions} onChange={(event) => setCheckpointDecisions(event.target.value)} disabled={readOnly} /></label>
            <label className="field"><span>Open questions</span><textarea rows={4} value={checkpointQuestions} onChange={(event) => setCheckpointQuestions(event.target.value)} disabled={readOnly} /></label>
            <label className="field"><span>Next actions</span><textarea rows={4} value={checkpointActions} onChange={(event) => setCheckpointActions(event.target.value)} disabled={readOnly} /></label>
            <label className="field"><span>Artifacts</span><textarea rows={3} value={checkpointArtifacts} onChange={(event) => setCheckpointArtifacts(event.target.value)} disabled={readOnly} /></label>
            <label className="field"><span>Expected revision</span><input value={session.corpus_revision ?? ""} readOnly /></label>
            <button className="button primary" type="submit" disabled={readOnly || !checkpointTitle.trim() || checkpointMutation.isPending} title={readOnly ? "Requires a read/write credential" : undefined}>
              <Save size={17} />
              {checkpointMutation.isPending ? "Committing" : "Commit"}
            </button>
            {checkpointMutation.isSuccess ? <span className="success-message"><Check size={16} /> Checkpoint committed</span> : null}
            {checkpointMutation.isError ? <span className="field-error">{checkpointMutation.error.message}</span> : null}
          </form>
        </Section>
      </div>

      <Section title="Initial evidence" meta={`${session.initial_evidence?.length ?? 0} items`}>
        {session.initial_evidence?.length ? (
          <div className="result-list">
            {session.initial_evidence.map((result) => (
              <article className="result-card" key={result.id}>
                <div><StatusBadge status={result.kind ?? result.profile} /><strong>{result.title}</strong></div>
                {result.excerpt ? <p>{result.excerpt}</p> : null}
                <div className="result-meta">
                  {result.source_id ? <Link to="/sources/$sourceId" params={{ sourceId: result.source_id }}>Source {shortId(result.source_id)}</Link> : null}
                  {result.object_id ? <Link to="/objects/$objectId" params={{ objectId: result.object_id }}>Object {shortId(result.object_id)}</Link> : null}
                </div>
              </article>
            ))}
          </div>
        ) : <EmptyState title="No initial evidence" />}
      </Section>

      {learnedContext ? (
        <Section
          title="Learned context"
          meta={learnedContext.authority === "derived_non_authoritative" ? "Derived, non-authoritative" : learnedContext.authority}
          actions={<StatusBadge status={learnedContext.status} />}
        >
          <DefinitionList
            items={[
              { label: "Included", value: learnedContext.included_by_default ? "Yes" : "No" },
              { label: "Hard gates", value: learnedContext.hard_gates_passed === true ? <StatusBadge status="passed" /> : "Not passed" },
              { label: "Candidate", value: <code>{learnedContext.candidate_ref ?? "None"}</code> },
              { label: "Generated", value: formatDate(learnedContext.generated_at) },
              { label: "Context tokens", value: learnedContext.estimated_tokens ?? 0 },
              { label: "Omitted", value: learnedContext.omitted_items ?? 0 },
            ]}
          />
          {learnedItems.length ? (
            <div className="result-list">
              {learnedItems.map((item, index) => (
                <article className="result-card" key={`${item.target_ref ?? "derived"}-${item.disposition ?? "view"}-${index}`}>
                  <header>
                    <div>
                      <StatusBadge status={item.disposition} />
                      <h3>{item.target_ref ?? "Derived view"}</h3>
                    </div>
                  </header>
                  {item.proposal !== undefined ? <JsonView value={item.proposal} label={`${item.target_ref ?? "Derived view"} proposal`} /> : null}
                  <footer>
                    {(item.source_refs ?? []).map((sourceRef) => <code key={sourceRef}>{sourceRef}</code>)}
                  </footer>
                </article>
              ))}
            </div>
          ) : <EmptyState title={learnedContext.reason ?? "No learned context included"} />}
          <JsonView value={learnedContext} label="Learned context audit record" />
        </Section>
      ) : null}

      <Section title="Verify claim" actions={<ShieldQuestion size={18} aria-hidden="true" />}>
        <form className="inline-form" onSubmit={submitVerification}>
          <input value={verificationClaim} onChange={(event) => setVerificationClaim(event.target.value)} placeholder="Claim to verify" aria-label="Claim to verify" />
          <button className="button secondary" type="submit" disabled={!verificationClaim.trim() || verifyMutation.isPending}>Verify</button>
        </form>
        {verifyMutation.isError ? <ErrorState error={verifyMutation.error} /> : null}
        {verificationResults.length ? <DataTable data={verificationResults} columns={verificationColumns} /> : null}
      </Section>

      <Section title="Operations" meta={`${session.operations?.length ?? 0} recorded`}>
        <DataTable data={session.operations ?? []} columns={operationColumns} emptyTitle="No operations" />
      </Section>
    </Page>
  );
}

function parseLines(value: string): string[] {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}

function formatLines(value: JsonValue | undefined): string {
  if (!Array.isArray(value)) return "";
  return value
    .map((item) => {
      if (typeof item === "string") return item;
      if (item && typeof item === "object" && !Array.isArray(item)) {
        for (const key of ["text", "title", "description", "action"]) {
          const candidate = item[key];
          if (typeof candidate === "string") return candidate;
        }
      }
      return JSON.stringify(item);
    })
    .join("\n");
}

function preserveOrParseLines(value: string, current: JsonValue | undefined): JsonValue {
  return current !== undefined && formatLines(current) === value
    ? current
    : parseLines(value);
}

function checkpointSourceRefs(session: SessionDetail | undefined): string[] {
  const refs = new Set<string>(session?.checkpoint?.source_refs ?? []);
  for (const evidence of session?.initial_evidence ?? []) {
    for (const candidate of [
      evidence.source_id,
      evidence.source_ref,
      evidence.reference,
    ]) {
      if (candidate && /^(source|source_episode|document|chunk|object|claim|artifact):/.test(candidate)) {
        refs.add(candidate);
      }
    }
  }
  return [...refs];
}
