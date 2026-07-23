import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Ban,
  Check,
  GitCompare,
  History,
  RotateCcw,
  ShieldAlert,
  ThumbsDown,
  ThumbsUp,
} from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";
import { JsonView } from "../components/JsonView";
import { DefinitionList, Metric, Page, PageHeader, Section } from "../components/Page";
import { EmptyState, ErrorState, LoadingState, ReadOnlyNotice, StatusBadge } from "../components/StateViews";
import { TabPanel, Tabs } from "../components/Tabs";
import { useApi } from "../lib/auth";
import { useReadOnly } from "../lib/current";
import { formatDate, humanize, shortId } from "../lib/format";
import { asItems, type DreamSummary, type JsonObject } from "../lib/types";

export function DreamsPage({ dreamId }: { dreamId?: string }) {
  const api = useApi();
  const queryClient = useQueryClient();
  const readOnly = useReadOnly();
  const [selectedId, setSelectedId] = useState<string | undefined>(dreamId);
  const [activeTab, setActiveTab] = useState("diff");
  const [reviewNote, setReviewNote] = useState("");
  const [rollbackReason, setRollbackReason] = useState("");
  const [rollbackOpen, setRollbackOpen] = useState(false);
  const dreamsQuery = useQuery({ queryKey: ["dreams"], queryFn: () => api.dreams() });
  const dreams = asItems(dreamsQuery.data?.data);
  const activeId = dreamId ?? selectedId ?? dreams[0]?.id;
  const dreamQuery = useQuery({
    queryKey: ["dream", activeId],
    queryFn: () => api.dream(activeId as string),
    enabled: Boolean(activeId),
  });
  const reviewMutation = useMutation({
    mutationFn: (decision: "approve" | "reject") =>
      api.reviewDream(activeId as string, {
        decision,
        note: reviewNote,
        expected_base_revision: dreamQuery.data?.data.base_revision ?? "",
        expected_candidate_revision: dreamQuery.data?.data.candidate_revision ?? "",
        expected_status: dreamQuery.data?.data.status ?? "",
      }),
    onSuccess: (result) => {
      queryClient.setQueryData(["dream", activeId], result);
      void queryClient.invalidateQueries({ queryKey: ["dreams"] });
    },
  });
  const rollbackMutation = useMutation({
    mutationFn: () => api.rollbackDream(activeId as string, {
      reason: rollbackReason,
      expected_candidate_revision: dreamQuery.data?.data.candidate_revision ?? "",
      expected_status: dreamQuery.data?.data.status ?? "",
    }),
    onSuccess: (result) => {
      queryClient.setQueryData(["dream", activeId], result);
      void queryClient.invalidateQueries({ queryKey: ["dreams"] });
      setRollbackOpen(false);
    },
  });

  const sortedDreams = useMemo(
    () => [...dreams].sort((left, right) => (right.updated_at ?? right.created_at ?? "").localeCompare(left.updated_at ?? left.created_at ?? "")),
    [dreams],
  );

  function chooseDream(dream: DreamSummary) {
    setSelectedId(dream.id);
    setActiveTab("diff");
    setReviewNote("");
    setRollbackOpen(false);
  }

  function submitRollback(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (rollbackReason.trim() && !readOnly) rollbackMutation.mutate();
  }

  return (
    <Page>
      <PageHeader title="Dreams" description="Candidate revisions, gates, review, promotion, and rollback" />
      {readOnly ? <ReadOnlyNotice /> : null}
      <div className="dream-layout">
        <Section title="Queue" meta={dreams.length ? `${dreams.length} jobs` : undefined} className="dream-queue">
          {dreamsQuery.isPending ? <LoadingState label="Loading dream jobs" /> : null}
          {dreamsQuery.isError ? <ErrorState error={dreamsQuery.error} retry={() => void dreamsQuery.refetch()} /> : null}
          {dreamsQuery.isSuccess && !dreams.length ? <EmptyState title="No dream jobs" /> : null}
          <div className="dream-list">
            {sortedDreams.map((dream) => (
              <button key={dream.id} type="button" className={dream.id === activeId ? "active" : undefined} onClick={() => chooseDream(dream)}>
                <div><strong>{dream.region ?? shortId(dream.id)}</strong><StatusBadge status={dream.status} /></div>
                <span>{dream.trigger ? humanize(dream.trigger) : "Unspecified trigger"}</span>
                <time>{formatDate(dream.updated_at ?? dream.created_at)}</time>
              </button>
            ))}
          </div>
        </Section>

        <div className="dream-detail">
          {!activeId && !dreamsQuery.isPending ? <EmptyState title="Select a dream job" /> : null}
          {dreamQuery.isPending ? <LoadingState label="Loading candidate" /> : null}
          {dreamQuery.isError ? <ErrorState error={dreamQuery.error} retry={() => void dreamQuery.refetch()} /> : null}
          {dreamQuery.data ? (() => {
            const dream = dreamQuery.data.data;
            const hardFailure = dream.gates?.some((gate) => gate.status === "failed");
            const canReview = ["awaiting_review", "review", "gated"].includes(dream.status);
            const canRollback = ["promoted", "monitoring"].includes(dream.status);
            const tabs = [
              { id: "diff", label: "Candidate diff", count: dream.diff?.length ?? 0 },
              { id: "gates", label: "Gates", count: dream.gates?.length ?? 0 },
              { id: "lineage", label: "Lineage", count: dream.lineage?.length ?? 0 },
              { id: "evaluation", label: "Evaluation" },
              { id: "audit", label: "Audit", count: dream.audit?.length ?? 0 },
              { id: "receipts", label: "Receipts" },
            ];
            return (
              <>
                <Section title={dream.region ?? "Candidate region"} actions={<StatusBadge status={dream.status} />}>
                  <DefinitionList items={[
                    { label: "Job", value: <code>{dream.id}</code> },
                    { label: "Base revision", value: <code>{dream.base_revision ?? "Unknown"}</code> },
                    { label: "Candidate", value: <code>{dream.candidate_revision ?? "Not built"}</code> },
                    { label: "Risk", value: <StatusBadge status={dream.risk_level} /> },
                    { label: "Trigger", value: humanize(dream.trigger) },
                    { label: "Updated", value: formatDate(dream.updated_at) },
                  ]} />
                </Section>
                <div className="metric-grid compact-metrics">
                  <Metric label="Changes" value={dream.diff?.length ?? 0} />
                  <Metric label="Passed gates" value={dream.gates?.filter((gate) => gate.status === "passed").length ?? 0} />
                  <Metric label="Failed gates" value={dream.gates?.filter((gate) => gate.status === "failed").length ?? 0} />
                  <Metric label="Evidence links" value={dream.lineage?.length ?? 0} />
                </div>
                <Tabs tabs={tabs} active={activeTab} onChange={setActiveTab} />

                <TabPanel id="diff" active={activeTab}>
                  <Section title="Candidate dispositions" actions={<GitCompare size={18} aria-hidden="true" />}>
                    {dream.diff?.length ? <div className="diff-list">{dream.diff.map((change, index) => <article key={change.id ?? `${change.kind}-${index}`}><header><strong>{change.target ?? change.kind}</strong><StatusBadge status={change.disposition ?? change.kind} /></header><div className="diff-values"><div><span>Before</span>{change.before !== undefined ? <JsonView value={change.before} label="Before" /> : <em>None</em>}</div><div><span>After</span>{change.after !== undefined ? <JsonView value={change.after} label="After" /> : <em>None</em>}</div></div></article>)}</div> : <EmptyState title="No candidate changes" />}
                  </Section>
                </TabPanel>

                <TabPanel id="gates" active={activeTab}>
                  <Section title="Deterministic gates" actions={hardFailure ? <ShieldAlert size={18} className="danger-icon" /> : <Check size={18} className="success-icon" />}>
                    {dream.gates?.length ? <div className="gate-list">{dream.gates.map((gate) => <article key={gate.id}><div><StatusBadge status={gate.status} /><strong>{gate.name}</strong></div>{gate.summary ? <p>{gate.summary}</p> : null}{gate.details !== undefined ? <JsonView value={gate.details} label={`${gate.name} details`} /> : null}</article>)}</div> : <EmptyState title="No gate results" />}
                  </Section>
                </TabPanel>

                <TabPanel id="lineage" active={activeTab}>
                  <Section title="Evidence lineage">{dream.lineage?.length ? <div className="evidence-list">{dream.lineage.map((ref, index) => <div key={`${ref.source_id}-${ref.locator}-${index}`}><div><strong>{ref.source_title ?? ref.source_id ?? "Evidence"}</strong><span>{ref.locator ?? "No locator"}</span></div><code>{shortId(ref.hash, 16)}</code></div>)}</div> : <EmptyState title="No lineage records" />}</Section>
                </TabPanel>

                <TabPanel id="evaluation" active={activeTab}>
                  <Section title="Active versus candidate">
                    {dream.evaluation ? <><div className="metric-grid compact-metrics"><Metric label="Active score" value={dream.evaluation.baseline_score?.toFixed(3) ?? "Not run"} /><Metric label="Candidate score" value={dream.evaluation.candidate_score?.toFixed(3) ?? "Not run"} /><Metric label="Delta" value={dream.evaluation.baseline_score !== undefined && dream.evaluation.candidate_score !== undefined ? (dream.evaluation.candidate_score - dream.evaluation.baseline_score).toFixed(3) : "Unknown"} /><Metric label="Regressions" value={dream.evaluation.regressions?.length ?? 0} /></div>{dream.evaluation.regressions?.length ? <ul className="plain-list">{dream.evaluation.regressions.map((regression) => <li key={regression}>{regression}</li>)}</ul> : null}</> : <EmptyState title="No evaluation result" />}
                  </Section>
                </TabPanel>

                <TabPanel id="audit" active={activeTab}>
                  <Section title="Dream audit trail" actions={<History size={18} aria-hidden="true" />}>
                    {dream.audit?.length ? <div className="audit-list dream-audit">{dream.audit.map((event) => <div key={event.id}><time>{formatDate(event.created_at)}</time><strong>{event.action}</strong><span>{event.actor ?? "System"}</span><StatusBadge status={event.status} />{event.details !== undefined ? <JsonView value={event.details} label={`${event.action} details`} /> : null}</div>)}</div> : <EmptyState title="No audit events" />}
                  </Section>
                </TabPanel>

                <TabPanel id="receipts" active={activeTab}>
                  <Section title="Promotion receipt">{dream.promotion_receipt !== undefined ? <JsonView value={dream.promotion_receipt} label="Promotion receipt" /> : <EmptyState title="Not promoted" />}</Section>
                  <Section title="Rollback receipt">{dream.rollback_receipt !== undefined ? <JsonView value={dream.rollback_receipt} label="Rollback receipt" /> : <EmptyState title="No rollback" />}</Section>
                </TabPanel>

                {(canReview || dream.review) ? (
                  <Section title="Review decision">
                    {dream.review?.decision ? <DefinitionList items={[{ label: "Decision", value: <StatusBadge status={dream.review.decision} /> }, { label: "Reviewer", value: dream.review.reviewer ?? "Unknown" }, { label: "Decided", value: formatDate(dream.review.decided_at) }, { label: "Note", value: dream.review.note ?? "No note" }]} /> : null}
                    {canReview ? <div className="review-form"><label className="field"><span>Review note</span><textarea rows={3} value={reviewNote} onChange={(event) => setReviewNote(event.target.value)} disabled={readOnly} /></label><div className="form-actions"><button className="button primary" type="button" onClick={() => reviewMutation.mutate("approve")} disabled={readOnly || hardFailure || reviewMutation.isPending} title={readOnly ? "Requires a read/write credential" : hardFailure ? "A hard gate failed" : undefined}><ThumbsUp size={17} />Approve</button><button className="button secondary" type="button" onClick={() => reviewMutation.mutate("reject")} disabled={readOnly || reviewMutation.isPending} title={readOnly ? "Requires a read/write credential" : undefined}><ThumbsDown size={17} />Reject</button></div>{reviewMutation.isError ? <ErrorState error={reviewMutation.error} title="Review failed" /> : null}</div> : null}
                  </Section>
                ) : null}

                {canRollback ? (
                  <Section title="Rollback">
                    {!rollbackOpen ? <button className="button danger" type="button" onClick={() => setRollbackOpen(true)} disabled={readOnly} title={readOnly ? "Requires a read/write credential" : undefined}><RotateCcw size={17} />Prepare rollback</button> : <form className="stacked-form" onSubmit={submitRollback}><label className="field"><span>Reason</span><textarea rows={3} value={rollbackReason} onChange={(event) => setRollbackReason(event.target.value)} required /></label><div className="form-actions"><button className="button danger" type="submit" disabled={!rollbackReason.trim() || rollbackMutation.isPending}><RotateCcw size={17} />{rollbackMutation.isPending ? "Rolling back" : "Confirm rollback"}</button><button className="button secondary" type="button" onClick={() => setRollbackOpen(false)}><Ban size={17} />Cancel</button></div></form>}
                    {rollbackMutation.isError ? <ErrorState error={rollbackMutation.error} title="Rollback failed" /> : null}
                  </Section>
                ) : null}
              </>
            );
          })() : null}
        </div>
      </div>
    </Page>
  );
}
