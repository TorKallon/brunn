export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };
export type JsonObject = { [key: string]: JsonValue };

export type ApiStatus =
  | "complete"
  | "partial"
  | "ambiguous"
  | "stale"
  | "inconsistent"
  | "degraded"
  | "failed"
  | "committed"
  | "no_op"
  | "accepted_processing"
  | "needs_review"
  | "conflict"
  | "rejected_by_policy";

export interface Freshness {
  source_updated_at?: string | null;
  normalized_at?: string | null;
  lexical_index_updated_at?: string | null;
  semantic_index_updated_at?: string | null;
  replica_applied_at?: string | null;
}

export interface CoveragePartition {
  lane?: string;
  partition?: string;
  complete?: boolean;
  searched_count?: number;
  candidate_count?: number;
  index_revision?: string;
  failure_reason?: string | null;
}

export interface Coverage {
  searched?: CoveragePartition[];
  unsearched?: CoveragePartition[];
  absence_safe?: boolean;
}

export interface ApiEnvelope<T> {
  request_id?: string;
  session_id?: string;
  corpus_revision?: string;
  status: ApiStatus;
  freshness?: Freshness;
  coverage?: Coverage;
  data: T;
  conflicts?: JsonValue[];
  gaps?: JsonValue[];
  ambiguities?: JsonValue[];
  truncation?: {
    truncated: boolean;
    returned_tokens?: number;
    limit_tokens?: number;
    continuation_token?: string | null;
  };
}

export interface UserSummary {
  id: string;
  display_name: string;
  email?: string;
}

export interface ScopeSummary {
  id: string;
  name: string;
  description?: string;
  access?: "read_only" | "read_write";
  object_count?: number;
}

export interface MeData {
  user: UserSummary;
  active_scope?: ScopeSummary;
  scopes?: ScopeSummary[];
  corpus_revision?: string;
  capabilities?: string[];
  read_only?: boolean;
  freshness?: Freshness;
}

export interface GoalSummary {
  id?: string;
  title: string;
  status?: string;
  due_at?: string | null;
}

export interface GateSummary {
  id?: string;
  title: string;
  status?: string;
  reason?: string;
}

export interface ActionSummary {
  id?: string;
  title: string;
  status?: string;
  owner?: string;
  due_at?: string | null;
}

export interface CheckpointSummary {
  id: string;
  title?: string;
  objective?: string;
  created_at?: string;
  corpus_revision?: string;
  parent_checkpoint_id?: string | null;
  goals?: GoalSummary[];
  gates?: GateSummary[];
  next_actions?: ActionSummary[];
  source_refs?: string[];
}

export interface SessionSummary {
  id: string;
  session_id?: string;
  title?: string;
  task_hash?: string;
  status?: string;
  scope_id?: string;
  corpus_revision?: string;
  checkpoint_id?: string | null;
  resumed_checkpoint_id?: string | null;
  created_at?: string;
  updated_at?: string;
  expires_at?: string;
  goals?: GoalSummary[];
  gates?: GateSummary[];
  next_actions?: ActionSummary[];
}

export interface SessionDetail extends SessionSummary {
  task?: string;
  coverage?: Coverage;
  checkpoint?: CheckpointSummary | null;
  initial_evidence?: SearchResult[];
  operations?: OperationSummary[];
  gaps?: JsonValue[];
  verification_results?: VerificationResult[];
  revision_delta?: JsonValue;
  learned_context?: LearnedContext;
}

export type LearnedContextItem = JsonObject & {
  target_ref?: string;
  disposition?: string;
  source_refs?: string[];
  proposal?: JsonValue;
};

export type LearnedContext = JsonObject & {
  status: string;
  included_by_default?: boolean;
  authority?: string;
  truth_boundary?: string;
  pinned_corpus_revision?: string;
  dream_ref?: string;
  candidate_ref?: string;
  candidate_corpus_revision?: string;
  candidate_manifest_hash?: string;
  generated_at?: string;
  hard_gates_passed?: boolean;
  retrieval_evaluation?: JsonValue;
  model?: JsonValue;
  items?: LearnedContextItem[];
  estimated_tokens?: number;
  omitted_items?: number;
  truncated?: boolean;
  scheduler?: JsonValue;
  latest_candidate?: JsonValue;
  reason?: string;
};

export interface OperationSummary {
  id: string;
  operation: string;
  status: string;
  created_at?: string;
  duration_ms?: number;
  result_count?: number;
}

export interface EvidenceRef {
  source_id?: string;
  source_title?: string;
  artifact_id?: string;
  locator?: string;
  version?: string | number;
  hash?: string;
}

export interface SearchResult {
  id?: string;
  reference?: string;
  object_id?: string;
  source_id?: string;
  source_ref?: string;
  title?: string;
  heading?: string;
  path?: string;
  excerpt?: string;
  content?: string;
  kind?: string;
  profile?: string;
  score?: number;
  state?: string;
  authority?: string;
  why_selected?: string[] | string;
  lane_scores?: Record<string, number>;
  evidence_refs?: EvidenceRef[];
  updated_at?: string;
  recorded_at?: string;
}

export interface QueryResultData {
  results: SearchResult[];
  query_results?: Array<{
    id: string;
    status?: string;
    results: SearchResult[];
  }>;
}

export interface VerificationResult {
  id?: string;
  claim: string;
  status?:
    | "supported"
    | "contradicted"
    | "insufficient_evidence"
    | "superseded"
      | "temporally_ambiguous";
  classification?:
    | "supported"
    | "contradicted"
    | "insufficient_evidence"
    | "superseded"
    | "temporally_ambiguous";
  explanation?: string;
  evidence_refs?: EvidenceRef[];
}

export interface ClaimRecord {
  id: string;
  predicate: string;
  value?: JsonValue;
  status?: string;
  authority?: string;
  confidence?: number;
  evidence_refs?: EvidenceRef[];
  created_at?: string;
}

export interface RelationRecord {
  id: string;
  predicate: string;
  endpoints: Array<{ role: string; object_id: string; label?: string }>;
  qualifiers?: JsonObject;
  status?: string;
}

export interface StateRecord {
  machine: string;
  value: string;
  effective_at?: string;
  claim_id?: string;
}

export interface ObjectVersion {
  version: number;
  created_at?: string;
  source_refs?: string[];
  change_summary?: string;
}

export interface ObjectRecord {
  id: string;
  display_name: string;
  profiles: string[];
  version: number;
  updated_at?: string;
  summary?: string;
  properties?: JsonObject;
  claims?: ClaimRecord[];
  relations?: RelationRecord[];
  states?: StateRecord[];
  timeline?: Array<{ id?: string; at: string; label: string; detail?: string }>;
  evidence?: EvidenceRef[];
  versions?: ObjectVersion[];
  policy?: PolicySummary;
  audit?: AuditEvent[];
}

export interface SourceRecord {
  id: string;
  title: string;
  kind?: string;
  version?: string | number;
  hash?: string;
  content_type?: string;
  size_bytes?: number;
  created_at?: string;
  updated_at?: string;
  content?: string;
  outline?: Array<{ locator: string; title: string; level?: number }>;
  derived_claims?: ClaimRecord[];
  versions?: ObjectVersion[];
  policy?: PolicySummary;
  audit?: AuditEvent[];
}

export interface CommitReceipt {
  id: string;
  operation_id?: string;
  status: string;
  base_corpus_revision?: string;
  resulting_corpus_revision?: string;
  created?: Array<{ kind: string; id: string }>;
  revised?: Array<{ kind: string; id: string; version?: number }>;
  deduplicated?: Array<{ kind: string; id: string }>;
  rejected?: Array<{ kind?: string; reason: string }>;
  review_required?: Array<{ kind?: string; reason: string }>;
  indexing?: Record<string, string>;
  created_at?: string;
}

export interface CaptureReceipt extends Partial<CommitReceipt> {
  status: string;
  summary?: string;
  capture_summary?: string;
  source_ref?: string;
  issues?: JsonValue[];
  unresolved?: string[];
  draft?: { save_request?: JsonObject };
  model?: JsonObject;
}

export interface StageReceipt {
  id: string;
  status: string;
  input_hash?: string;
  inventory_hash?: string;
  entries?: Array<{
    path: string;
    size_bytes?: number;
    content_type?: string;
    disposition?: string;
    status?: string;
  }>;
}

export interface DreamSummary {
  id: string;
  region?: string;
  status: string;
  base_revision?: string;
  candidate_revision?: string;
  trigger?: string;
  created_at?: string;
  updated_at?: string;
  risk_level?: string;
}

export interface DreamGate {
  id: string;
  name: string;
  status: "passed" | "failed" | "warning" | "pending";
  summary?: string;
  details?: JsonValue;
}

export interface DreamDetail extends DreamSummary {
  diff?: Array<{
    id?: string;
    kind: string;
    target?: string;
    disposition?: string;
    before?: JsonValue;
    after?: JsonValue;
  }>;
  gates?: DreamGate[];
  lineage?: EvidenceRef[];
  evaluation?: {
    baseline_score?: number;
    candidate_score?: number;
    regressions?: string[];
  };
  audit?: AuditEvent[];
  review?: {
    decision?: string;
    reviewer?: string;
    note?: string;
    decided_at?: string;
  };
  promotion_receipt?: JsonValue;
  rollback_receipt?: JsonValue;
}

export interface CredentialSummary {
  id: string;
  name: string;
  access: "read_only" | "read_write";
  scope_ids: string[];
  created_at?: string;
  last_used_at?: string | null;
  expires_at?: string | null;
  revoked_at?: string | null;
  token?: string;
}

export interface PolicySummary {
  id: string;
  name: string;
  version?: number;
  audience?: string[];
  purpose?: string[];
  status?: string;
  updated_at?: string;
  rules?: JsonValue;
}

export interface AuditEvent {
  id: string;
  action: string;
  actor?: string;
  target?: string;
  status?: string;
  created_at: string;
  request_id?: string;
  details?: JsonValue;
}

export interface ServiceStatus {
  status: "healthy" | "degraded" | "unavailable" | string;
  version?: string;
  corpus_revision?: string;
  dependencies?: Record<
    string,
    { status: string; latency_ms?: number; detail?: string }
  >;
  indexes?: Record<
    string,
    { status: string; revision?: string; updated_at?: string; lag_seconds?: number }
  >;
}

export interface ListData<T> {
  items: T[];
  continuation_token?: string | null;
  total?: number;
}

export function asItems<T>(data: ListData<T> | T[] | undefined): T[] {
  if (!data) return [];
  return Array.isArray(data) ? data : data.items ?? [];
}
