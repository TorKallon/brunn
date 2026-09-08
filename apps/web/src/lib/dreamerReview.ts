export type DreamerDecisionAction = "approve" | "reject" | "defer" | "correct";

export interface DreamerReviewSource {
  entry_ref: string;
  path?: string | null;
  version?: number | null;
  label?: string | null;
  excerpt?: string | null;
}

export interface DreamerReviewItem {
  id: string;
  kind: "proposal" | "question";
  title: string;
  body_md: string;
  why_md?: string | null;
  uncertainty_md?: string | null;
  run_id: string;
  run_entry_ref: string;
  run_version: number;
  candidate_hash: string;
  candidate?: {
    body_md?: string | null;
    before_md?: string | null;
    after_md?: string | null;
    target_path?: string | null;
  } | null;
  sources: DreamerReviewSource[];
  status: string;
  reviewable: boolean;
  stale: boolean;
  blocked_reason?: string | null;
}

export interface DreamerReviewDecision {
  id: string;
  item_id: string;
  decision: DreamerDecisionAction;
  comment?: string | null;
  correction?: string | null;
  at: string;
  application_status: string;
}

export interface DreamerReviewData {
  available: boolean;
  mode: string;
  paused: boolean;
  unavailable_reason?: string | null;
  last_attempt?: {
    date?: string | null;
    outcome: string;
    detail?: string | null;
    started_at?: string | null;
    finished_at?: string | null;
  } | null;
  last_successful_run?: {
    run_id: string;
    entry_ref: string;
    version: number;
    at?: string | null;
  } | null;
  counts: {
    pending: number;
    proposals: number;
    questions: number;
    approved_held: number;
    applied: number;
  };
  items: DreamerReviewItem[];
  history: DreamerReviewDecision[];
  decision_version: number;
}

export interface DreamerDecisionInput {
  item_id: string;
  run_entry_ref: string;
  run_version: number;
  candidate_hash: string;
  expected_decisions_version: number;
  decision: DreamerDecisionAction;
  comment?: string;
  correction?: string;
  idempotency_key: string;
}

export interface DreamerDecisionData {
  saved?: boolean;
  decision: DreamerReviewDecision;
  application_status: string;
  message?: string;
  state_version?: number;
}

export function reviewIdentity(item: DreamerReviewItem): string {
  return JSON.stringify([item.id, item.run_entry_ref, item.run_version, item.candidate_hash]);
}

export function approvalBlock(item: DreamerReviewItem): string | undefined {
  if (item.stale) return "The supporting evidence has changed. This item needs a fresh candidate and another review.";
  if (item.status === "approved_held") return "This candidate is already approved and held for authorized application.";
  if (item.status === "needs_changes") return "Your correction is awaiting a fresh candidate and another review.";
  if (!item.reviewable) return item.blocked_reason || "This proposal describes future work. A concrete candidate and its evidence are needed before approval.";
  if (!item.candidate || !(item.candidate.body_md?.trim() || item.candidate.after_md?.trim())) {
    return "There is no concrete candidate to approve yet.";
  }
  if (!item.sources.length) return "Supporting sources are needed before approval.";
  return undefined;
}
