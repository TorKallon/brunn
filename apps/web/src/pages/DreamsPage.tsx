import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Check, CircleHelp, Clock3, FileDiff, History, MessageSquareText, RefreshCw, X } from "lucide-react";
import { useRef, useState } from "react";
import { MarkdownView } from "../components/MarkdownView";
import { Metric, Page, PageHeader, Section } from "../components/Page";
import { EmptyState, ErrorState, LoadingState, ReadOnlyNotice, StatusBadge } from "../components/StateViews";
import { ApiError } from "../lib/api";
import { useApi } from "../lib/auth";
import { useCapability, useReadOnly } from "../lib/current";
import { approvalBlock, reviewIdentity, type DreamerDecisionAction, type DreamerDecisionInput, type DreamerReviewData, type DreamerReviewItem, type DreamerReviewSource } from "../lib/dreamerReview";
import { formatDate, humanize } from "../lib/format";
import { newOperationId } from "../lib/workspace";
import "../review.css";

type Filter = "all" | "proposal" | "question";
type ReviewDraft = { comment: string; correction: string };

function EntryLink({ entryRef, version, children }: { entryRef: string; version: number; children: React.ReactNode }) {
  return <Link to="/explore" search={{ entryRef, version }}>{children}</Link>;
}

function EvidenceSource({ source }: { source: DreamerReviewSource }) {
  const label = source.label || source.path || source.entry_ref;
  const version = typeof source.version === "number" && Number.isSafeInteger(source.version) && source.version > 0 ? source.version : undefined;
  return <div>{source.entry_ref.startsWith("entry:") && version !== undefined
    ? <EntryLink entryRef={source.entry_ref} version={version}>{label}</EntryLink>
    : <strong>{label}</strong>}{version !== undefined ? <span>v{version}</span> : null}</div>;
}

function RunStatus({ data }: { data: DreamerReviewData }) {
  return <div className="review-run-status">
    <div className="review-run-fact">
      <span className="review-eyebrow">Last attempt</span>
      {data.last_attempt ? <>
        <strong>{data.last_attempt.date ?? formatDate(data.last_attempt.started_at)}</strong>
        <StatusBadge status={data.last_attempt.outcome} />
        {data.last_attempt.detail ? <p>{data.last_attempt.detail}</p> : null}
      </> : <strong>No attempt recorded</strong>}
    </div>
    <div className="review-run-fact">
      <span className="review-eyebrow">Last successful run</span>
      {data.last_successful_run ? <>
        <strong><EntryLink entryRef={data.last_successful_run.entry_ref} version={data.last_successful_run.version}>{data.last_successful_run.run_id}</EntryLink></strong>
        <span>Report v{data.last_successful_run.version}</span>
      </> : <><strong>No successful run recorded</strong><p>Pending work may still be available below.</p></>}
    </div>
    <div className="review-run-fact">
      <span className="review-eyebrow">Application mode</span>
      <strong>{humanize(data.mode)}</strong>
      {data.paused ? <StatusBadge status="paused" /> : null}
      <p>{data.mode === "report-only" ? "Approvals are held. No candidate is applied in report-only mode." : "Approval is recorded first. Application requires source and policy checks."}</p>
    </div>
  </div>;
}

function ReviewDetail({ item, data, decisionVersion, changed, unavailable, draft, onDraftChange, onDecisionBusy, onReviewUpdated }: {
  item: DreamerReviewItem;
  data: DreamerReviewData;
  decisionVersion: number;
  changed: boolean;
  unavailable: boolean;
  draft: ReviewDraft;
  onDraftChange: (draft: ReviewDraft) => void;
  onDecisionBusy: (busy: boolean) => void;
  onReviewUpdated?: () => void;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const isOwner = useCapability("credential:manage");
  const readOnly = useReadOnly();
  const canDecide = isOwner && !readOnly;
  const { comment, correction } = draft;
  // Retain the exact request after an uncertain response so Retry is idempotent.
  const pendingRequest = useRef<DreamerDecisionInput | null>(null);
  const decisionMutation = useMutation({
    mutationFn: (request: DreamerDecisionInput) => api.dreamerReviewDecision(request),
    onMutate: () => onDecisionBusy(true),
    onSuccess: () => {
      pendingRequest.current = null;
      onDecisionBusy(false);
      void queryClient.invalidateQueries({ queryKey: ["dreamer-review"] });
      void queryClient.invalidateQueries({ queryKey: ["dreaming-status"] });
    },
    onError: (error) => {
      // An uncertain request must be retried before replacing this review.
      if (error instanceof ApiError && error.status >= 400 && error.status < 500) onDecisionBusy(false);
      if (error instanceof ApiError && error.status === 409) {
        void queryClient.invalidateQueries({ queryKey: ["dreamer-review"] });
      }
    },
  });
  const blocked = approvalBlock(item);
  const conflict = decisionMutation.error instanceof ApiError && decisionMutation.error.status === 409;
  const disabled = !canDecide || changed || unavailable || decisionMutation.isPending || decisionMutation.isSuccess || conflict;
  const uncertain = decisionMutation.isError && !(decisionMutation.error instanceof ApiError && decisionMutation.error.status >= 400 && decisionMutation.error.status < 500);

  function decide(decision: DreamerDecisionAction) {
    if (disabled || (decision === "approve" && blocked) || (decision === "correct" && !correction.trim())) return;
    const payload: DreamerDecisionInput = {
      item_id: item.id,
      run_entry_ref: item.run_entry_ref,
      run_version: item.run_version,
      candidate_hash: item.candidate_hash,
      expected_decisions_version: decisionVersion,
      decision,
      ...(comment.trim() ? { comment: comment.trim() } : {}),
      ...(decision === "correct" ? { correction: correction.trim() } : {}),
      idempotency_key: newOperationId("web_dreamer_decision"),
    };
    pendingRequest.current = payload;
    decisionMutation.mutate(payload);
  }

  return <article className="review-detail" aria-label={`Review ${item.title}`}>
    <header className="review-detail-header">
      <div className="review-detail-meta"><span className="review-eyebrow">{item.kind === "question" ? "Needs your call" : "Proposed"}</span><StatusBadge status={item.status} /></div>
      <h2>{item.title}</h2>
      <div className="review-origin"><code>{item.id}</code><span>From <EntryLink entryRef={item.run_entry_ref} version={item.run_version}>{item.run_id} · v{item.run_version}</EntryLink></span></div>
    </header>
    {(changed || conflict) && !decisionMutation.isSuccess ? <div className="review-notice warning" role="status"><strong>This review has changed.</strong><p>Review the latest item and decisions before taking another action. Your note is kept when you open the updated item.</p>{onReviewUpdated ? <button className="button secondary" disabled={decisionMutation.isPending || uncertain} onClick={onReviewUpdated}>Review updated item</button> : <p>This item is no longer in the pending inbox.</p>}</div> : null}
    {blocked ? <div className="review-notice warning"><strong>{item.stale || item.status === "needs_changes" ? "Needs another review" : item.status === "approved_held" ? "Approved and held" : "Candidate not ready"}</strong><p>{blocked}</p></div> : null}
    <div className="review-detail-section"><h3>{item.kind === "question" ? "The question" : "Proposal"}</h3><MarkdownView markdown={item.body_md} stripAnchors /></div>
    {item.candidate ? <div className="review-detail-section">
      <h3>Candidate</h3>
      {item.candidate.target_path ? <p className="review-target"><code>{item.candidate.target_path}</code></p> : null}
      {item.candidate.before_md !== undefined && item.candidate.before_md !== null && item.candidate.after_md !== undefined && item.candidate.after_md !== null ? <div className="review-diff">
        <div><h4>Before</h4><pre>{item.candidate.before_md || "(New entry)"}</pre></div>
        <div><h4>After</h4><pre>{item.candidate.after_md || "(Empty content)"}</pre></div>
      </div> : item.candidate.body_md || item.candidate.after_md ? <MarkdownView markdown={item.candidate.body_md || item.candidate.after_md || ""} stripAnchors /> : null}
    </div> : null}
    <div className="review-detail-section"><h3>Why it is proposed</h3>{item.why_md ? <MarkdownView markdown={item.why_md} stripAnchors /> : <p className="review-muted">No separate rationale was supplied.</p>}</div>
    <div className="review-detail-section"><h3>Uncertainty</h3>{item.uncertainty_md ? <MarkdownView markdown={item.uncertainty_md} stripAnchors /> : <p className="review-muted">No uncertainty statement was supplied.</p>}</div>
    <div className="review-detail-section"><h3>Supporting evidence <span className="review-count">{item.sources.length}</span></h3>
      {item.sources.length ? <ul className="review-sources">{item.sources.map((source, index) => <li key={`${source.entry_ref}:${source.version}:${index}`}>
        <EvidenceSource source={source} />
        {source.excerpt ? <blockquote>{source.excerpt}</blockquote> : null}
      </li>)}</ul> : <p className="review-muted">No exact source references are attached yet.</p>}
    </div>
    <div className="review-decision" aria-label="Your decision">
      <h3>Your decision</h3>
      {!canDecide ? <p className="review-muted">Owner access is required to record decisions.</p> : null}
      {data.mode === "report-only" ? <p className="review-muted">Approve records your decision and holds application until an authorized mode permits it.</p> : null}
      <label className="field"><span>Comment <span className="review-muted">(optional)</span></span><textarea rows={2} maxLength={4000} value={comment} onChange={(event) => onDraftChange({ ...draft, comment: event.target.value })} disabled={disabled || uncertain} placeholder="Add context for this decision" /></label>
      <div className="review-actions">
        <button className="button primary" type="button" disabled={disabled || uncertain || Boolean(blocked)} onClick={() => decide("approve")} title={blocked}><Check size={16} aria-hidden="true" />Approve</button>
        <button className="button secondary" type="button" disabled={disabled || uncertain} onClick={() => decide("reject")}><X size={16} aria-hidden="true" />Reject</button>
        <button className="button secondary" type="button" disabled={disabled || uncertain} onClick={() => decide("defer")}><Clock3 size={16} aria-hidden="true" />Defer</button>
      </div>
      <details className="review-correction"><summary><MessageSquareText size={16} aria-hidden="true" />{item.kind === "question" ? "Answer or correct this item" : "Suggest a correction"}</summary>
        <label className="field"><span>{item.kind === "question" ? "Answer or correction" : "Correction"}</span><textarea rows={3} maxLength={4000} value={correction} onChange={(event) => onDraftChange({ ...draft, correction: event.target.value })} disabled={disabled || uncertain} /></label>
        <p className="review-muted">The note is recorded for a fresh candidate and review.</p>
        <button className="button secondary" type="button" disabled={disabled || uncertain || !correction.trim()} onClick={() => decide("correct")}>Save correction</button>
      </details>
      {decisionMutation.isPending ? <p role="status">Recording your decision…</p> : null}
      {decisionMutation.isError ? <ErrorState error={decisionMutation.error} title={conflict ? "Decision needs another review" : "Unable to confirm your decision"} retry={uncertain && pendingRequest.current ? () => decisionMutation.mutate(pendingRequest.current!) : undefined} /> : null}
      {decisionMutation.isSuccess ? <div className="review-notice" role="status"><strong>Decision recorded</strong><p>{decisionMutation.data.data.message ?? humanize(decisionMutation.data.data.application_status)}</p></div> : null}
    </div>
  </article>;
}

export function DreamsPage() {
  const api = useApi();
  const readOnly = useReadOnly();
  const [filter, setFilter] = useState<Filter>("all");
  const [selected, setSelected] = useState<DreamerReviewItem | null>(null);
  const [selectedDecisionVersion, setSelectedDecisionVersion] = useState(0);
  const [selectionRevision, setSelectionRevision] = useState(0);
  const [decisionBusy, setDecisionBusy] = useState(false);
  const [drafts, setDrafts] = useState<Record<string, ReviewDraft>>({});
  const reviewQuery = useQuery({ queryKey: ["dreamer-review"], queryFn: () => api.dreamerReview(), refetchInterval: 60_000 });
  const data = reviewQuery.data?.data;
  const items = data?.items ?? [];
  const visibleItems = filter === "all" ? items : items.filter((item) => item.kind === filter);
  const currentItem = selected ? items.find((item) => item.id === selected.id) : undefined;
  // Preserve decision identity across ordinary revisions, but never keep
  // displaying cached evidence after the server explicitly withholds it.
  const displayedItem = currentItem?.stale && !currentItem.reviewable && currentItem.sources.length === 0
    ? currentItem : selected;
  const changed = Boolean(selected && (!currentItem || reviewIdentity(currentItem) !== reviewIdentity(selected) || currentItem.status !== selected.status || currentItem.stale !== selected.stale || selectedDecisionVersion !== data?.decision_version));

  function selectItem(item: DreamerReviewItem) {
    setSelected(item);
    setSelectedDecisionVersion(data?.decision_version ?? 0);
    setSelectionRevision((value) => value + 1);
  }

  return <Page>
    <PageHeader title="Review" description="Proposals, questions, and decisions from nightly dreaming" actions={<button className="button secondary" type="button" onClick={() => void reviewQuery.refetch()} disabled={reviewQuery.isFetching}><RefreshCw size={16} aria-hidden="true" />{reviewQuery.isFetching ? "Refreshing…" : "Refresh"}</button>} />
    {readOnly ? <ReadOnlyNotice /> : null}
    {reviewQuery.isPending ? <LoadingState label="Loading review inbox" /> : null}
    {reviewQuery.isError ? <ErrorState error={reviewQuery.error} retry={() => void reviewQuery.refetch()} title="Review inbox unavailable" /> : null}
    {data ? <>
      <RunStatus data={data} />
      {!data.available ? <div className="review-notice warning" role="status"><strong>Review is not available yet</strong><p>{data.unavailable_reason ?? "A complete review snapshot is not available. Pending work cannot be confirmed."}</p></div> : null}
      <div className="metric-grid review-metrics"><Metric label="Proposed" value={data.available ? data.counts.proposals : "—"} detail="Pending proposals" /><Metric label="Needs your call" value={data.available ? data.counts.questions : "—"} detail="Questions to resolve" /><Metric label="Approved & held" value={data.available ? data.counts.approved_held : "—"} detail="Awaiting authorized application" /><Metric label="Applied" value={data.available ? data.counts.applied : "—"} detail="Confirmed changes" /></div>
      <div className="review-workspace">
        <Section title="Pending review" meta={data.available ? `${data.counts.pending} items` : "Unavailable"} className="review-inbox">
          <div className="review-filters" role="group" aria-label="Filter review items">{([
            ["all", "All", items.length], ["proposal", "Proposed", data.counts.proposals], ["question", "Needs your call", data.counts.questions],
          ] as const).map(([value, label, count]) => <button type="button" key={value} aria-pressed={filter === value} onClick={() => setFilter(value)}>{label}{" "}<span>{count}</span></button>)}</div>
          {visibleItems.length ? <ul className="review-item-list">{visibleItems.map((item) => <li key={item.id}>
            <button type="button" className={`review-item ${selected?.id === item.id ? "selected" : ""}`} disabled={decisionBusy} onClick={() => selectItem(item)} aria-pressed={selected?.id === item.id} aria-label={`Review ${item.title}`}>
              <span className="review-item-kind">{item.kind === "question" ? <CircleHelp size={16} aria-hidden="true" /> : <FileDiff size={16} aria-hidden="true" />}{item.kind === "question" ? "Needs your call" : "Proposed"}<span>{item.run_id}</span></span>
              <strong>{item.title}</strong><span className="review-item-status"><StatusBadge status={item.stale ? "stale" : item.status} />{!item.reviewable ? <span>Candidate not ready</span> : null}</span>
            </button>
          </li>)}</ul> : <EmptyState title={data.available ? (filter === "all" ? "Nothing waiting for review" : `No ${filter === "proposal" ? "proposals" : "questions"} waiting`) : "Pending items unavailable"} detail={data.available ? "New proposals and questions will appear here after a run. Past decisions remain below." : "Refresh when the review snapshot is available."} />}
        </Section>
        <div className="review-detail-wrap">{selected ? <ReviewDetail key={`${reviewIdentity(selected)}:${selectionRevision}`} item={displayedItem ?? selected} data={data} decisionVersion={selectedDecisionVersion} changed={changed} unavailable={!data.available || reviewQuery.isError} draft={drafts[selected.id] ?? { comment: "", correction: "" }} onDraftChange={(draft) => setDrafts((current) => ({ ...current, [selected.id]: draft }))} onDecisionBusy={setDecisionBusy} onReviewUpdated={currentItem ? () => selectItem(currentItem) : undefined} /> : <div className="review-unselected"><FileDiff size={28} aria-hidden="true" /><h2>Choose an item to review</h2><p>Read the candidate, its evidence, and any uncertainty before recording a decision.</p></div>}</div>
      </div>
      <Section title="Decision history" meta="Recorded decisions and application state" actions={<History size={18} aria-hidden="true" />}>
        {data.history.length ? <ol className="review-history">{data.history.map((decision) => <li key={decision.id}><div className="review-history-heading"><strong>{humanize(decision.decision)}</strong><StatusBadge status={decision.application_status} /><time dateTime={decision.at}>{formatDate(decision.at)}</time></div><code>{decision.item_id}</code>{decision.comment ? <p>{decision.comment}</p> : null}{decision.correction ? <p><strong>Correction:</strong> {decision.correction}</p> : null}</li>)}</ol> : <EmptyState title="No decisions recorded" detail="Approvals, rejections, deferrals, and corrections will appear here." />}
      </Section>
    </> : null}
  </Page>;
}
