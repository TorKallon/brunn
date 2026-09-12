import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowLeft, Check, ChevronLeft, ChevronRight, CircleHelp, Clock3, FileDiff, History, MessageSquareText, RefreshCw, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { MarkdownView } from "../components/MarkdownView";
import { Metric, Page, PageHeader, Section } from "../components/Page";
import { EmptyState, ErrorState, LoadingState, ReadOnlyNotice, StatusBadge } from "../components/StateViews";
import { ApiError } from "../lib/api";
import { useApi } from "../lib/auth";
import { useCapability, useReadOnly } from "../lib/current";
import { approvalBlock, decisionStateLine, isLegacyReviewItem, legacyReportTitle, reviewIdentity, type DreamerDecisionAction, type DreamerDecisionInput, type DreamerReviewData, type DreamerReviewItem, type DreamerReviewSource } from "../lib/dreamerReview";
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

function SupportingEvidence({ sources }: { sources: DreamerReviewSource[] }) {
  return <details className="review-detail-section review-evidence">
    <summary aria-label={`Supporting evidence, ${sources.length} ${sources.length === 1 ? "source" : "sources"}`}><span>Supporting evidence</span><span className="review-count">{sources.length}</span><ChevronRight size={18} aria-hidden="true" /></summary>
    {sources.length ? <ul className="review-sources">{sources.map((source, index) => <li key={`${source.entry_ref}:${source.version}:${index}`}>
      <EvidenceSource source={source} />{source.excerpt ? <blockquote>{source.excerpt}</blockquote> : null}
    </li>)}</ul> : <p className="review-muted">No exact source references are attached yet.</p>}
  </details>;
}

function additionalReviewText(text: string | null | undefined, displayedText: string): string | undefined {
  const content = text?.trim();
  return content && !displayedText.includes(content) ? content : undefined;
}

function CandidateDiff({ candidate }: { candidate: NonNullable<DreamerReviewItem["candidate"]> }) {
  return <div className="review-diff">
    <div><h4>Before</h4><pre>{candidate.before_md || "(New entry)"}</pre></div>
    <div><h4>After</h4><pre>{candidate.after_md || "(Empty content)"}</pre></div>
  </div>;
}

function RunStatus({ data }: { data: DreamerReviewData }) {
  const attemptAt = data.last_attempt?.finished_at ?? data.last_attempt?.started_at;
  return <div className="review-run-status">
    <div className="review-run-fact">
      <span className="review-eyebrow">Last attempt</span>
      {data.last_attempt ? <>
        <strong>{attemptAt ? <time dateTime={attemptAt}>{formatDate(attemptAt)}</time> : data.last_attempt.date ?? "Not available"}</strong>
        <StatusBadge status={data.last_attempt.outcome} />
        {data.last_attempt.detail ? <p>{data.last_attempt.detail}</p> : null}
      </> : <strong>No attempt recorded</strong>}
    </div>
    <div className="review-run-fact">
      <span className="review-eyebrow">Last fully completed run</span>
      {data.last_successful_run ? <>
        <strong><EntryLink entryRef={data.last_successful_run.entry_ref} version={data.last_successful_run.version}>{data.last_successful_run.run_id}</EntryLink></strong>
        <span>Report v{data.last_successful_run.version}</span>
      </> : <><strong>No fully completed run yet</strong><p>Runs can produce review items while leaving unfinished work for later.</p></>}
    </div>
    <div className="review-run-fact">
      <span className="review-eyebrow">Application mode</span>
      <strong>{humanize(data.mode)}</strong>
      {data.paused ? <StatusBadge status="paused" /> : null}
      <p>{modeBanner(data)}</p>
    </div>
  </div>;
}

function modeBanner(data: DreamerReviewData): string {
  if (data.paused) return "Dreaming is paused. Decisions are saved, but no run will act on them until it resumes.";
  return data.mode === "report-only"
    ? "Report-only: your approvals are saved and held. Nothing is written until Dreaming is switched to full mode."
    : "Full mode: approvals are written by the next run after their sources are re-checked.";
}

function OlderReportDetail({ item }: { item: DreamerReviewItem }) {
  return <article className="review-detail review-older-detail" aria-label={`Older report ${legacyReportTitle(item)}`}>
    <header className="review-detail-header">
      <span className="review-eyebrow">Older report · For reference</span>
      <h2>{legacyReportTitle(item)}</h2>
      <div className="review-origin"><span>From <EntryLink entryRef={item.run_entry_ref} version={item.run_version}>{item.run_id} · v{item.run_version}</EntryLink></span></div>
      <p className="review-mode-note">This note was saved from an older run. No decision is needed, and it is not scheduled to apply.</p>
    </header>
    <div className="review-detail-section"><h3>Original report text</h3><p className="review-muted">The original wording is preserved below, including any past scheduling language.</p><pre className="review-original-text">{item.body_md || "The original report text is no longer available."}</pre></div>
    {item.sources.length ? <SupportingEvidence sources={item.sources} /> : null}
  </article>;
}

function ReviewDetail({ item, historical, data, decisionVersion, changed, unavailable, draft, onDraftChange, onDecisionBusy, onReviewUpdated }: {
  item: DreamerReviewItem;
  historical: boolean;
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
  const disabled = historical || !canDecide || changed || unavailable || decisionMutation.isPending || decisionMutation.isSuccess || conflict;
  const uncertain = decisionMutation.isError && !(decisionMutation.error instanceof ApiError && decisionMutation.error.status >= 400 && decisionMutation.error.status < 500);
  const managedSummary = Boolean(item.candidate?.target_path?.startsWith("derived/location/") || item.candidate?.target_path?.startsWith("derived/entities/"));
  const hasDiff = item.candidate?.before_md != null && item.candidate?.after_md != null;
  const candidateText = hasDiff && !managedSummary
    ? `${item.candidate!.before_md}\n\n${item.candidate!.after_md}`
    : managedSummary ? item.candidate?.after_md ?? item.candidate?.body_md ?? ""
    : item.candidate?.body_md?.trim() ? item.candidate.body_md : item.candidate?.after_md ?? "";
  const primaryText = `${item.body_md}\n\n${candidateText}`;
  const why = additionalReviewText(item.why_md, primaryText);
  const uncertainty = additionalReviewText(item.uncertainty_md, `${primaryText}\n\n${why ?? ""}`);

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

  // Classification may change while a request is in flight. Keep its exact
  // retry available without presenting any new decisions for a historical note.
  if (historical) return <><OlderReportDetail item={item} />
    {decisionMutation.isPending ? <p role="status">Confirming the earlier decision request…</p> : null}
    {decisionMutation.isError ? <ErrorState error={decisionMutation.error} title="Earlier request result" retry={uncertain && pendingRequest.current ? () => decisionMutation.mutate(pendingRequest.current!) : undefined} /> : null}
    {decisionMutation.isSuccess ? <p role="status">Earlier request confirmed. {decisionMutation.data.data.message ?? humanize(decisionMutation.data.data.application_status)}</p> : null}
  </>;

  return <article className="review-detail" aria-label={`Review ${item.title}`}>
    <header className="review-detail-header">
      <div className="review-detail-meta"><span className="review-eyebrow">{item.kind === "question" ? "Needs your call" : "Proposed"}</span><StatusBadge status={item.status} /></div>
      <h2>{item.title}</h2>
      <div className="review-origin"><code>{item.id}</code><span>From <EntryLink entryRef={item.run_entry_ref} version={item.run_version}>{item.run_id} · v{item.run_version}</EntryLink></span></div>
    </header>
    {(changed || conflict) && !decisionMutation.isSuccess ? <div className="review-notice warning" role="status"><strong>This review has changed.</strong><p>{onReviewUpdated ? "The latest item is shown below. Read it and its decisions before another action. Your note is kept." : "This item is no longer in the pending inbox. Your note is kept."}</p>{onReviewUpdated ? <button className="button secondary" disabled={decisionMutation.isPending || uncertain} onClick={onReviewUpdated}>Review updated item</button> : null}</div> : null}
    {blocked && (item.kind !== "question" || item.stale || item.status === "needs_changes") ? <div className="review-notice warning"><strong>{item.stale || item.status === "needs_changes" ? "Needs another review" : item.status === "approved_held" ? "Approved and held" : "Candidate not ready"}</strong><p>{blocked}</p></div> : null}
    <div className="review-detail-section"><h3>{item.kind === "question" ? "The question" : "Proposal"}</h3><MarkdownView markdown={item.body_md} stripAnchors /></div>
    {item.candidate && (item.candidate.body_md?.trim() || item.candidate.before_md?.trim() || item.candidate.after_md?.trim()) ? <div className="review-detail-section">
      <h3>Candidate</h3>
      {item.candidate.target_path ? <p className="review-target"><code>{item.candidate.target_path}</code></p> : null}
      {hasDiff && !managedSummary ? <CandidateDiff candidate={item.candidate} /> : candidateText.trim() ? <MarkdownView markdown={candidateText} stripAnchors /> : null}
      {hasDiff && managedSummary ? <details className="review-exact-changes"><summary>View exact changes</summary><CandidateDiff candidate={item.candidate} /></details> : null}
    </div> : null}
    {why ? <div className="review-detail-section"><h3>Why it is proposed</h3><MarkdownView markdown={why} stripAnchors /></div> : null}
    {uncertainty ? <div className="review-detail-section"><h3>Uncertainty</h3><MarkdownView markdown={uncertainty} stripAnchors /></div> : null}
    <SupportingEvidence sources={item.sources} />
    <div className="review-decision" aria-label="Your decision">
      <h3>Your decision</h3>
      {!canDecide ? <p className="review-muted">Owner access is required to record decisions.</p> : null}
      <label className="field"><span>Comment <span className="review-muted">(optional)</span></span><textarea rows={2} maxLength={4000} value={comment} onChange={(event) => onDraftChange({ ...draft, comment: event.target.value })} disabled={disabled || uncertain} placeholder="Add context for this decision" /></label>
      <div className="review-actions">
        {item.kind !== "question" ? <button className="button primary" type="button" disabled={disabled || uncertain || Boolean(blocked)} onClick={() => decide("approve")} title={blocked}><Check size={16} aria-hidden="true" />Approve</button> : null}
        <button className="button secondary" type="button" disabled={disabled || uncertain} onClick={() => decide("reject")}><X size={16} aria-hidden="true" />Reject</button>
        <button className="button secondary" type="button" disabled={disabled || uncertain} onClick={() => decide("defer")}><Clock3 size={16} aria-hidden="true" />Defer</button>
      </div>
      <ul className="review-consequences">
        {item.kind !== "question" ? <li><strong>Approve</strong>{data.mode === "report-only"
          ? "Saves your approval and holds it. It is written at the first run after full mode is switched on, after its sources are re-checked. If they change first, it comes back here."
          : "Written now if its sources still match; otherwise it comes back here."}</li> : null}
        <li><strong>Reject</strong>Final. Nothing is written and this proposal does not come back.</li>
        <li><strong>Defer</strong>Stays in this inbox. Nothing is written.</li>
      </ul>
      <details className="review-correction" open={item.kind === "question" ? true : undefined}><summary><MessageSquareText size={16} aria-hidden="true" />{item.kind === "question" ? "Answer or correct this item" : "Suggest a correction"}</summary>
        <label className="field"><span>{item.kind === "question" ? "Answer or correction" : "Correction"}</span><textarea rows={3} maxLength={4000} value={correction} onChange={(event) => onDraftChange({ ...draft, correction: event.target.value })} disabled={disabled || uncertain} /></label>
        <p className="review-muted">Goes to the next run, which drafts a new version for you to review. Nothing is written until you approve that version.</p>
        <button className="button secondary" type="button" disabled={disabled || uncertain || !correction.trim()} onClick={() => decide("correct")}>Save correction</button>
      </details>
      {decisionMutation.isPending ? <p role="status">Recording your decision…</p> : null}
      {decisionMutation.isError ? <ErrorState error={decisionMutation.error} title={conflict ? "Decision needs another review" : "Unable to confirm your decision"} retry={uncertain && pendingRequest.current ? () => decisionMutation.mutate(pendingRequest.current!) : undefined} /> : null}
      {decisionMutation.isSuccess ? <div className="review-notice" role="status"><strong>{decisionStateLine(decisionMutation.data.data.application_status, item.candidate?.target_path)}</strong>{decisionMutation.data.data.message ? <p>{decisionMutation.data.data.message}</p> : null}</div> : null}
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
  const [detailOpen, setDetailOpen] = useState(false);
  const [olderReportsOpen, setOlderReportsOpen] = useState(false);
  const detailRef = useRef<HTMLDivElement>(null);
  const itemButtons = useRef(new Map<string, HTMLButtonElement>());
  const [drafts, setDrafts] = useState<Record<string, ReviewDraft>>({});
  const reviewQuery = useQuery({ queryKey: ["dreamer-review"], queryFn: () => api.dreamerReview(), refetchInterval: 60_000 });
  const data = reviewQuery.data?.data;
  const legacyItemMap = new Map([...(data?.items.filter(isLegacyReviewItem) ?? []), ...(data?.legacy_items ?? [])].map((item) => [item.id, item]));
  const legacyItems = [...legacyItemMap.values()];
  const legacyCount = data?.counts.legacy ?? legacyItems.length;
  const items = data?.items.filter((item) => !legacyItemMap.has(item.id)) ?? [];
  const legacyFallback = items.length !== data?.items.length;
  const counts = data ? legacyFallback ? { ...data.counts, pending: items.length, proposals: items.filter((item) => item.kind === "proposal").length, questions: items.filter((item) => item.kind === "question").length } : data.counts : undefined;
  const visibleItems = filter === "all" ? items : items.filter((item) => item.kind === filter);
  const currentLegacyItem = selected ? legacyItems.find((item) => item.id === selected.id) : undefined;
  const historical = Boolean(selected && (currentLegacyItem || isLegacyReviewItem(selected)));
  const currentItem = selected ? currentLegacyItem ?? items.find((item) => item.id === selected.id) : undefined;
  // Show refreshed content, including server redaction, immediately. The
  // selected snapshot still pins decision identity until acknowledgment.
  const displayedItem = currentItem ?? selected;
  const changed = Boolean(selected && (!currentItem || reviewIdentity(currentItem) !== reviewIdentity(selected) || currentItem.status !== selected.status || currentItem.stale !== selected.stale || selectedDecisionVersion !== data?.decision_version));
  const navigationItems = historical ? legacyItems : visibleItems;
  const selectedIndex = selected ? navigationItems.findIndex((item) => item.id === selected.id) : -1;

  useEffect(() => {
    if (!detailOpen) return;
    detailRef.current?.focus({ preventScroll: true });
    detailRef.current?.scrollIntoView?.({ block: "start" });
  }, [detailOpen, selectionRevision]);

  useEffect(() => {
    if (!detailOpen && selected) itemButtons.current.get(selected.id)?.focus();
  }, [detailOpen, selected]);

  function selectItem(item: DreamerReviewItem) {
    if (decisionBusy) return;
    setSelected(item);
    setDetailOpen(true);
    setSelectedDecisionVersion(data?.decision_version ?? 0);
    setSelectionRevision((value) => value + 1);
  }

  return <Page className={`review-page${detailOpen ? " review-reading" : ""}`}>
    <PageHeader title="Review" description="Concrete proposals and questions that need your decision" actions={<button className="button secondary" type="button" onClick={() => void reviewQuery.refetch()} disabled={reviewQuery.isFetching}><RefreshCw size={16} aria-hidden="true" />{reviewQuery.isFetching ? "Refreshing…" : "Refresh"}</button>} />
    {readOnly ? <ReadOnlyNotice /> : null}
    {reviewQuery.isPending ? <LoadingState label="Loading review inbox" /> : null}
    {reviewQuery.isError ? <ErrorState error={reviewQuery.error} retry={() => void reviewQuery.refetch()} title="Review inbox unavailable" /> : null}
    {data ? <>
      <RunStatus data={data} />
      {!data.available ? <div className="review-notice warning" role="status"><strong>Review is not available yet</strong><p>{data.unavailable_reason ?? "A complete review snapshot is not available. Pending work cannot be confirmed."}</p></div> : null}
      <div className="metric-grid review-metrics"><Metric label="Proposed" value={data.available ? counts!.proposals : "—"} detail="Pending proposals" /><Metric label="Needs your call" value={data.available ? counts!.questions : "—"} detail="Questions to resolve" /><Metric label="Approved & held" value={data.available ? counts!.approved_held : "—"} detail="Awaiting authorized application" /><Metric label="Applied" value={data.available ? counts!.applied : "—"} detail="Confirmed changes" /></div>
      <div className="review-workspace" data-detail-open={detailOpen}>
        <Section title="Pending review" meta={data.available ? `${counts!.pending} items` : "Unavailable"} className="review-inbox">
          <div className="review-filters" role="group" aria-label="Filter review items">{([
            ["all", "All", items.length], ["proposal", "Proposed", counts!.proposals], ["question", "Needs your call", counts!.questions],
          ] as const).map(([value, label, count]) => <button type="button" key={value} aria-pressed={filter === value} onClick={() => setFilter(value)}>{label}{" "}<span>{count}</span></button>)}</div>
          {visibleItems.length ? <ul className="review-item-list">{visibleItems.map((item) => <li key={item.id}>
            <button ref={(node) => { if (node) itemButtons.current.set(item.id, node); else itemButtons.current.delete(item.id); }} type="button" className={`review-item ${selected?.id === item.id ? "selected" : ""}`} disabled={decisionBusy} onClick={() => selectItem(item)} aria-pressed={selected?.id === item.id} aria-label={`Review ${item.title}`}>
              <span className="review-item-kind">{item.kind === "question" ? <CircleHelp size={16} aria-hidden="true" /> : <FileDiff size={16} aria-hidden="true" />}{item.kind === "question" ? "Needs your call" : "Proposed"}<span>{item.run_id}</span></span>
              <strong className="review-item-title">{item.title}</strong><span className="review-item-status"><StatusBadge status={item.stale ? "stale" : item.status} />{!item.reviewable && item.kind !== "question" ? <span>Candidate not ready</span> : null}</span>
              <span className="review-item-open">Read full {item.kind === "question" ? "question" : "proposal"}<ChevronRight size={16} aria-hidden="true" /></span>
            </button>
          </li>)}</ul> : <EmptyState title={data.available ? (filter === "all" ? "Nothing waiting for review" : `No ${filter === "proposal" ? "proposals" : "questions"} waiting`) : "Pending items unavailable"} detail={data.available ? "New proposals and questions will appear here after a run. Past decisions remain below." : "Refresh when the review snapshot is available."} />}
        </Section>
        <div className="review-detail-wrap" ref={detailRef} role="region" tabIndex={-1} aria-label="Selected review">{selected ? <>
          <nav className="review-navigation" aria-label="Review navigation">
            <button type="button" className="button secondary review-back" disabled={decisionBusy} onClick={() => { if (historical) setOlderReportsOpen(true); setDetailOpen(false); }}><ArrowLeft size={16} aria-hidden="true" />{historical ? "Back to older reports" : "Back to inbox"}</button>
            <span className="review-position">{selectedIndex >= 0 ? `${selectedIndex + 1} of ${navigationItems.length}` : "Reviewed item"}</span>
            <div className="review-step-buttons">
              <button type="button" className="button secondary" aria-label={historical ? "Previous older report" : "Previous review item"} disabled={decisionBusy || selectedIndex <= 0} onClick={() => selectItem(navigationItems[selectedIndex - 1])}><ChevronLeft size={16} aria-hidden="true" /><span>Previous</span></button>
              <button type="button" className="button secondary" aria-label={historical ? "Next older report" : "Next review item"} disabled={decisionBusy || selectedIndex >= navigationItems.length - 1} onClick={() => selectItem(navigationItems[selectedIndex + 1])}><span>Next</span><ChevronRight size={16} aria-hidden="true" /></button>
            </div>
          </nav>
          <ReviewDetail key={`${reviewIdentity(selected)}:${selectionRevision}`} item={displayedItem ?? selected} historical={historical} data={data} decisionVersion={selectedDecisionVersion} changed={changed} unavailable={!data.available || reviewQuery.isError} draft={drafts[selected.id] ?? { comment: "", correction: "" }} onDraftChange={(draft) => setDrafts((current) => ({ ...current, [selected.id]: draft }))} onDecisionBusy={setDecisionBusy} onReviewUpdated={currentItem ? () => selectItem(currentItem) : undefined} />
        </> : <div className="review-unselected"><FileDiff size={28} aria-hidden="true" /><h2>Choose an item to review</h2><p>Read the candidate, its evidence, and any uncertainty before recording a decision.</p></div>}</div>
      </div>
      {legacyItems.length ? <details className="review-older-reports" open={olderReportsOpen} onToggle={(event) => setOlderReportsOpen(event.currentTarget.open)}>
        <summary><History size={18} aria-hidden="true" /><strong>Older reports</strong><span>{legacyCount} {legacyCount === 1 ? "note" : "notes"}</span><ChevronRight size={18} aria-hidden="true" /></summary>
        <p className="review-muted">Notes from earlier runs, kept for reference. These are not waiting for a decision.</p>
        <ul className="review-item-list">{legacyItems.map((item) => <li key={item.id}><button ref={(node) => { if (node) itemButtons.current.set(item.id, node); else itemButtons.current.delete(item.id); }} type="button" className={`review-item ${selected?.id === item.id ? "selected" : ""}`} disabled={decisionBusy} onClick={() => selectItem({ ...item, legacy: true })} aria-label={`Read older report ${legacyReportTitle(item)}`}>
          <span className="review-item-kind">Report note<span>{item.run_id}</span></span><strong className="review-item-title">{legacyReportTitle(item)}</strong><span className="review-item-open">Read original report<ChevronRight size={16} aria-hidden="true" /></span>
        </button></li>)}</ul>
      </details> : null}
      <Section title="Decision history" meta="Recorded decisions and application state" className="review-history-section" actions={<History size={18} aria-hidden="true" />}>
        {data.history.length ? <ol className="review-history">{data.history.map((decision) => <li key={decision.id}><div className="review-history-heading"><strong>{humanize(decision.decision)}</strong><StatusBadge status={decision.application_status} /><time dateTime={decision.at}>{formatDate(decision.at)}</time></div><code>{decision.item_id}</code>{decision.comment ? <p>{decision.comment}</p> : null}{decision.correction ? <p><strong>Correction:</strong> {decision.correction}</p> : null}</li>)}</ol> : <EmptyState title="No decisions recorded" detail="Approvals, rejections, deferrals, and corrections will appear here." />}
      </Section>
    </> : null}
  </Page>;
}
