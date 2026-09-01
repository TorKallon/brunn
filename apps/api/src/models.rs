use std::{collections::BTreeMap, fmt, str::FromStr};

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

macro_rules! id_type {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub fn ref_string(self) -> String {
                format!(concat!($prefix, ":{}"), self.0.simple())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let value = value.strip_prefix(concat!($prefix, ":")).unwrap_or(value);
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

id_type!(UserId, "user");
id_type!(CredentialId, "credential");
id_type!(ScopeId, "scope");
id_type!(ObjectId, "object");
id_type!(ObjectRevisionId, "object-version");
id_type!(ClaimId, "claim");
id_type!(EvidenceId, "evidence");
id_type!(RelationId, "relation");
id_type!(RelationRevisionId, "relation-version");
id_type!(PolicyId, "policy");
id_type!(PolicyRevisionId, "policy-version");
id_type!(CheckpointId, "checkpoint");
id_type!(CorpusRevisionId, "revision");
id_type!(SessionId, "session");
id_type!(SourceId, "source");
id_type!(AssetId, "asset");
id_type!(StageId, "stage");
id_type!(DreamId, "dream");

static NAMESPACED_IDENTIFIER: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^[a-z][A-Za-z0-9_.-]*(?::[A-Za-z0-9][A-Za-z0-9_.@/-]*)?$")
        .expect("identifier regex")
});

pub fn validate_namespaced_identifier(name: &str, value: &str) -> ApiResult<()> {
    if value.len() > 240 || !NAMESPACED_IDENTIFIER.is_match(value) {
        return Err(ApiError::invalid(format!(
            "{name} is not a valid namespaced identifier"
        )));
    }
    Ok(())
}

pub fn reject_global_lifecycle(value: &Value) -> ApiResult<()> {
    match value {
        Value::Object(object) => {
            if object.contains_key("lifecycle_state") || object.contains_key("global_status") {
                return Err(ApiError::invalid(
                    "global lifecycle fields are prohibited; use a named state assignment",
                ));
            }
            for nested in object.values() {
                reject_global_lifecycle(nested)?;
            }
        }
        Value::Array(items) => {
            for nested in items {
                reject_global_lifecycle(nested)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Open,
    Query,
    Read,
    Compute,
    Verify,
    Status,
    Checkpoint,
    Save,
    Stage,
    Correct,
    Delete,
    Dream,
    CredentialManage,
    NotificationPublish,
    NotificationManage,
    SecretRead,
    SecretWrite,
    #[serde(rename = "task.read")]
    TaskRead,
    #[serde(rename = "task.write")]
    TaskWrite,
    #[serde(rename = "integration.manage")]
    IntegrationManage,
    #[serde(rename = "message.read")]
    MessageRead,
    #[serde(rename = "message.write")]
    MessageWrite,
    Admin,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Query => "query",
            Self::Read => "read",
            Self::Compute => "compute",
            Self::Verify => "verify",
            Self::Status => "status",
            Self::Checkpoint => "checkpoint",
            Self::Save => "save",
            Self::Stage => "stage",
            Self::Correct => "correct",
            Self::Delete => "delete",
            Self::Dream => "dream",
            Self::CredentialManage => "credential:manage",
            Self::NotificationPublish => "notification:publish",
            Self::NotificationManage => "notification:manage",
            Self::SecretRead => "secret:read",
            Self::SecretWrite => "secret:write",
            Self::TaskRead => "task.read",
            Self::TaskWrite => "task.write",
            Self::IntegrationManage => "integration.manage",
            Self::MessageRead => "message.read",
            Self::MessageWrite => "message.write",
            Self::Admin => "admin",
        }
    }
}

impl FromStr for Capability {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "open" => Ok(Self::Open),
            "query" => Ok(Self::Query),
            "read" => Ok(Self::Read),
            "compute" => Ok(Self::Compute),
            "verify" => Ok(Self::Verify),
            "status" => Ok(Self::Status),
            "checkpoint" => Ok(Self::Checkpoint),
            "save" => Ok(Self::Save),
            "stage" => Ok(Self::Stage),
            "correct" => Ok(Self::Correct),
            "delete" => Ok(Self::Delete),
            "dream" => Ok(Self::Dream),
            "credential:manage" => Ok(Self::CredentialManage),
            "notification:publish" => Ok(Self::NotificationPublish),
            "notification:manage" => Ok(Self::NotificationManage),
            "secret:read" => Ok(Self::SecretRead),
            "secret:write" => Ok(Self::SecretWrite),
            "task.read" => Ok(Self::TaskRead),
            "task.write" => Ok(Self::TaskWrite),
            "integration.manage" => Ok(Self::IntegrationManage),
            "message.read" => Ok(Self::MessageRead),
            "message.write" => Ok(Self::MessageWrite),
            "admin" => Ok(Self::Admin),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormationMethod {
    Asserted,
    Extracted,
    Observed,
    Inferred,
    Synthesized,
    ActionResult,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimMode {
    Descriptive,
    Preference,
    Instruction,
    Assessment,
    Hypothesis,
    Proposal,
    StateAssignment,
    StateTransition,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportState {
    Reported,
    Supported,
    Verified,
    Disputed,
    Refuted,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusLens {
    Domain,
    Situation,
    Activity,
    Artifact,
    Evidence,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TemporalSpecV1 {
    Date {
        date: NaiveDate,
    },
    DateInterval {
        start: NaiveDate,
        end_exclusive: Option<NaiveDate>,
    },
    LocalDatetime {
        local: NaiveDateTime,
        time_zone: String,
    },
    FloatingDatetime {
        local: NaiveDateTime,
    },
    Instant {
        at: DateTime<Utc>,
    },
    InstantInterval {
        start: DateTime<Utc>,
        end: Option<DateTime<Utc>>,
    },
    LocalInterval {
        start_local: NaiveDateTime,
        end_local: NaiveDateTime,
        time_zone: String,
    },
    Due {
        local: NaiveDateTime,
        time_zone: String,
    },
}

impl TemporalSpecV1 {
    pub fn validate(&self) -> ApiResult<()> {
        match self {
            Self::Date { .. } | Self::FloatingDatetime { .. } | Self::Instant { .. } => {}
            Self::DateInterval {
                start,
                end_exclusive,
            } => {
                if end_exclusive.is_some_and(|end| end <= *start) {
                    return Err(ApiError::invalid("date interval end must be after start"));
                }
            }
            Self::InstantInterval { start, end } => {
                if end.is_some_and(|end| end <= *start) {
                    return Err(ApiError::invalid(
                        "instant interval end must be after start",
                    ));
                }
            }
            Self::LocalInterval {
                start_local,
                end_local,
                time_zone,
            } => {
                validate_time_zone(time_zone)?;
                if end_local <= start_local {
                    return Err(ApiError::invalid("local interval end must be after start"));
                }
            }
            Self::LocalDatetime { time_zone, .. } | Self::Due { time_zone, .. } => {
                validate_time_zone(time_zone)?;
            }
        }
        Ok(())
    }
}

fn validate_time_zone(value: &str) -> ApiResult<()> {
    value
        .parse::<chrono_tz::Tz>()
        .map(|_| ())
        .map_err(|_| ApiError::invalid(format!("unknown IANA time zone: {value}")))
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct RecurrenceSpecV1 {
    pub schema: String,
    pub start: TemporalSpecV1,
    pub duration: Option<String>,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub excluded_recurrence_ids: Vec<TemporalSpecV1>,
    #[serde(default)]
    pub added_occurrence_refs: Vec<String>,
    pub split_from_series_ref: Option<String>,
}

impl RecurrenceSpecV1 {
    pub fn validate(&self) -> ApiResult<()> {
        if self.schema != "recurrence-spec@v1" {
            return Err(ApiError::invalid("unsupported recurrence schema"));
        }
        self.start.validate()?;
        for excluded in &self.excluded_recurrence_ids {
            excluded.validate()?;
        }
        if self.rules.iter().any(|rule| !rule.starts_with("FREQ=")) {
            return Err(ApiError::invalid(
                "recurrence rules must use an RFC 5545 FREQ rule",
            ));
        }
        Ok(())
    }
}

pub const RESERVED_PROFILES: &[&str] = &[
    "core.person",
    "core.organization",
    "core.group",
    "core.place",
    "core.event",
    "core.arrangement",
    "core.resource",
    "core.work_item",
    "core.artifact",
    "core.domain_object",
];

pub const STATE_REGISTRY: &[(&str, &[&str])] = &[
    (
        "state:event.schedule@v1",
        &["tentative", "confirmed", "postponed", "cancelled"],
    ),
    (
        "state:event.execution@v1",
        &["not_started", "in_progress", "completed", "aborted"],
    ),
    (
        "state:participation.response@v1",
        &[
            "needs_action",
            "tentative",
            "accepted",
            "declined",
            "delegated",
        ],
    ),
    (
        "state:participation.attendance@v1",
        &["unknown", "attended", "absent", "partial"],
    ),
    (
        "state:notification.delivery@v1",
        &["not_sent", "sent", "delivered", "failed", "acknowledged"],
    ),
    (
        "state:arrangement.booking@v1",
        &["proposed", "held", "confirmed", "cancelled", "expired"],
    ),
    (
        "state:arrangement.payment@v1",
        &[
            "not_due",
            "unpaid",
            "deposit_paid",
            "paid",
            "waived",
            "refunded",
        ],
    ),
    (
        "state:arrangement.allocation@v1",
        &[
            "unallocated",
            "pending",
            "allocated",
            "waitlisted",
            "released",
        ],
    ),
    (
        "state:arrangement.use@v1",
        &["not_started", "in_use", "used", "no_show"],
    ),
    (
        "state:resource.availability@v1",
        &["unknown", "available", "unavailable"],
    ),
    (
        "state:work.execution@v1",
        &[
            "not_started",
            "in_progress",
            "blocked",
            "completed",
            "abandoned",
        ],
    ),
    (
        "state:gate.validation@v1",
        &[
            "not_started",
            "attempted",
            "passed",
            "failed",
            "blocked",
            "not_applicable",
        ],
    ),
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Complete,
    Partial,
    Degraded,
    Ambiguous,
    Stale,
    Inconsistent,
    Committed,
    NoOp,
    AcceptedProcessing,
    NeedsReview,
    Conflict,
    RejectedByPolicy,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
pub struct Freshness {
    pub source_updated_at: Option<DateTime<Utc>>,
    pub normalized_at: Option<DateTime<Utc>>,
    pub lexical_index_updated_at: Option<DateTime<Utc>>,
    pub semantic_index_updated_at: Option<DateTime<Utc>>,
    pub replica_applied_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
pub struct CoveragePartition {
    pub partition: String,
    pub lane: String,
    pub completeness: String,
    pub index_revision: Option<String>,
    pub searched_count: usize,
    pub candidate_count: usize,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
pub struct Coverage {
    #[serde(default)]
    pub searched: Vec<CoveragePartition>,
    #[serde(default)]
    pub unsearched: Vec<CoveragePartition>,
    pub absence_safe: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct Truncation {
    pub truncated: bool,
    pub returned_tokens: usize,
    pub limit_tokens: usize,
    pub continuation_token: Option<String>,
}

impl Default for Truncation {
    fn default() -> Self {
        Self {
            truncated: false,
            returned_tokens: 0,
            limit_tokens: 24_000,
            continuation_token: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct ApiEnvelope<T> {
    pub request_id: String,
    pub session_id: Option<String>,
    pub corpus_revision: Option<String>,
    pub status: ResponseStatus,
    pub freshness: Freshness,
    pub coverage: Coverage,
    pub data: T,
    #[serde(default)]
    pub conflicts: Vec<Value>,
    #[serde(default)]
    pub gaps: Vec<Value>,
    #[serde(default)]
    pub ambiguities: Vec<Value>,
    pub truncation: Truncation,
}

impl<T> ApiEnvelope<T> {
    pub fn complete(data: T) -> Self {
        Self {
            request_id: crate::request_context::current_request_id(),
            session_id: None,
            corpus_revision: None,
            status: ResponseStatus::Complete,
            freshness: Freshness::default(),
            coverage: Coverage::default(),
            data,
            conflicts: vec![],
            gaps: vec![],
            ambiguities: vec![],
            truncation: Truncation::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
pub struct OpenHints {
    pub authorization_scope: Option<String>,
    #[serde(default)]
    pub root_refs: Vec<String>,
    #[serde(default)]
    pub open_object_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct OpenRequest {
    pub task: String,
    #[serde(default)]
    pub hints: OpenHints,
    #[serde(default = "latest")]
    pub as_of: String,
    #[serde(default = "default_open_mode")]
    pub mode: String,
    #[serde(alias = "checkpoint_id")]
    pub resume_checkpoint_ref: Option<String>,
    pub token_budget: Option<usize>,
}

fn latest() -> String {
    "latest".to_owned()
}

fn default_open_mode() -> String {
    "continuation".to_owned()
}

impl OpenRequest {
    pub fn validate(&self) -> ApiResult<()> {
        if self.task.trim().is_empty() {
            return Err(ApiError::invalid("task is required"));
        }
        if self.task.len() > 64_000 {
            return Err(ApiError::invalid("task exceeds 64,000 characters"));
        }
        if let Some(scope) = &self.hints.authorization_scope {
            validate_namespaced_identifier("authorization_scope", scope)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct QueryRequest {
    pub session_id: String,
    #[serde(default)]
    pub queries: Vec<QuerySpec>,
    pub goal: Option<String>,
    pub query: Option<String>,
    pub scope: Option<QueryScope>,
    #[serde(default)]
    pub modes: Vec<RetrievalMode>,
    #[serde(default)]
    pub r#where: Map<String, Value>,
    pub state_filter: Option<StateFilter>,
    pub expand: Option<QueryExpansion>,
    pub limit: Option<usize>,
}

impl QueryRequest {
    pub fn normalized_queries(&self) -> ApiResult<Vec<QuerySpec>> {
        let queries = if self.queries.is_empty() {
            vec![QuerySpec {
                id: Some("q0".to_owned()),
                goal: self.goal.clone(),
                query: self.query.clone(),
                scope: self.scope.clone(),
                modes: self.modes.clone(),
                r#where: self.r#where.clone(),
                state_filter: self.state_filter.clone(),
                expand: self.expand.clone(),
                limit: self.limit.unwrap_or(12),
            }]
        } else {
            self.queries.clone()
        };
        if queries.len() > 32 {
            return Err(ApiError::invalid("query batches are limited to 32 items"));
        }
        for query in &queries {
            query.validate()?;
        }
        Ok(queries)
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct QuerySpec {
    pub id: Option<String>,
    pub goal: Option<String>,
    pub query: Option<String>,
    pub scope: Option<QueryScope>,
    #[serde(default)]
    pub modes: Vec<RetrievalMode>,
    #[serde(default)]
    pub r#where: Map<String, Value>,
    pub state_filter: Option<StateFilter>,
    pub expand: Option<QueryExpansion>,
    #[serde(default = "default_query_limit")]
    pub limit: usize,
}

impl QuerySpec {
    fn validate(&self) -> ApiResult<()> {
        if self.query.as_deref().unwrap_or("").trim().is_empty()
            && self.r#where.is_empty()
            && self.state_filter.is_none()
        {
            return Err(ApiError::invalid(
                "each query requires natural language, a structured filter, or a state filter",
            ));
        }
        if self.limit == 0 || self.limit > 100 {
            return Err(ApiError::invalid("query limit must be between 1 and 100"));
        }
        const SUPPORTED_FILTERS: &[&str] = &[
            "scope_root",
            "type_profile",
            "predicate",
            "record_kind",
            "authority",
            "canonicality",
        ];
        for (key, value) in &self.r#where {
            if !SUPPORTED_FILTERS.contains(&key.as_str()) {
                return Err(ApiError::invalid(format!(
                    "unsupported structured filter: {key}"
                )));
            }
            if value.as_str().is_none_or(|value| value.trim().is_empty()) {
                return Err(ApiError::invalid(format!(
                    "structured filter {key} must be a nonempty string"
                )));
            }
        }
        if let Some(filter) = &self.state_filter {
            validate_namespaced_identifier("state_filter.machine_ref", &filter.machine_ref)?;
            if filter.states.iter().any(|state| state.trim().is_empty()) {
                return Err(ApiError::invalid(
                    "state_filter.states cannot contain empty values",
                ));
            }
            if filter
                .valid_at
                .as_deref()
                .is_some_and(|value| value != "latest")
            {
                return Err(ApiError::invalid(
                    "state_filter.valid_at currently supports only latest",
                ));
            }
        }
        Ok(())
    }
}

fn default_query_limit() -> usize {
    12
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(untagged)]
pub enum QueryScope {
    Ref(String),
    Structured {
        authorization_scope: Option<String>,
        #[serde(default)]
        root_refs: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalMode {
    Exact,
    Structured,
    Lexical,
    Semantic,
    Temporal,
    Relations,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct StateFilter {
    pub machine_ref: String,
    #[serde(default)]
    pub states: Vec<String>,
    pub valid_at: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
pub struct QueryExpansion {
    #[serde(default)]
    pub parents: bool,
    #[serde(default)]
    pub neighbors: usize,
    #[serde(default)]
    pub relations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct ReadRequest {
    pub session_id: String,
    pub requests: Vec<ReadSpec>,
}

impl ReadRequest {
    pub fn validate(&self) -> ApiResult<()> {
        if self.requests.is_empty() || self.requests.len() > 64 {
            return Err(ApiError::invalid(
                "read batches must contain between 1 and 64 requests",
            ));
        }
        for request in &self.requests {
            if request.reference.trim().is_empty() {
                return Err(ApiError::invalid("read ref is required"));
            }
            if request.max_chars.unwrap_or(20_000) > 100_000 {
                return Err(ApiError::invalid("max_chars cannot exceed 100,000"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct ReadSpec {
    #[serde(rename = "ref", alias = "path")]
    pub reference: String,
    #[serde(default)]
    pub view: ReadView,
    pub version: Option<String>,
    pub anchor: Option<String>,
    pub before: Option<usize>,
    pub after: Option<usize>,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub from_version: Option<String>,
    pub to_version: Option<String>,
    pub max_chars: Option<usize>,
    pub continuation_token: Option<String>,
    pub audience: Option<String>,
    pub purpose: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadView {
    CurrentState,
    Structured,
    Outline,
    #[default]
    Full,
    Range,
    Neighbors,
    Relationships,
    History,
    Diff,
    LastKnownGood,
    MaterializeScope,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct ComputeRequest {
    pub session_id: String,
    pub steps: Vec<ComputeStep>,
    pub max_rows: Option<usize>,
    pub token_budget: Option<usize>,
}

impl ComputeRequest {
    pub fn validate(&self) -> ApiResult<()> {
        if self.steps.is_empty() || self.steps.len() > 64 {
            return Err(ApiError::invalid(
                "compute plans must contain between 1 and 64 steps",
            ));
        }
        if self.max_rows.unwrap_or(10_000) > 10_000 {
            return Err(ApiError::invalid("compute max_rows cannot exceed 10,000"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct ComputeStep {
    pub id: String,
    pub op: ComputeOperator,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ComputeOperator {
    Catalog,
    Query,
    Search,
    Read,
    BatchRead,
    Neighbors,
    Timeline,
    History,
    Diff,
    Group,
    Aggregate,
    Traverse,
    ResolveIdentity,
    ExpandRecurrence,
    StateHistory,
    Impact,
    CompareApplicability,
    Proximity,
    GateRollup,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct VerifyRequest {
    pub session_id: String,
    pub claims: Vec<VerifyClaim>,
    #[serde(default)]
    pub check_for: Vec<String>,
}

impl VerifyRequest {
    pub fn validate(&self) -> ApiResult<()> {
        if self.claims.is_empty() || self.claims.len() > 32 {
            return Err(ApiError::invalid(
                "verify batches must contain between 1 and 32 claims",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct VerifyClaim {
    pub id: Option<String>,
    pub claim: String,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    pub about_ref: Option<String>,
    pub predicate: Option<String>,
    pub value: Option<Value>,
    pub coverage_ref: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationClassification {
    Supported,
    Contradicted,
    InsufficientEvidence,
    Superseded,
    TemporallyAmbiguous,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct SaveRequest {
    pub intent: String,
    pub scope: String,
    #[serde(default)]
    pub root_refs: Vec<String>,
    #[serde(default)]
    pub source_refs: Vec<SourceReference>,
    pub items: Vec<SaveItem>,
    pub idempotency_key: String,
    pub operation_id: Option<String>,
    pub base_corpus_revision: Option<String>,
    pub confirmation_token: Option<String>,
}

impl SaveRequest {
    pub fn validate(&self) -> ApiResult<()> {
        if self.idempotency_key.trim().is_empty() || self.idempotency_key.len() > 240 {
            return Err(ApiError::invalid(
                "idempotency_key is required and limited to 240 characters",
            ));
        }
        validate_namespaced_identifier("scope", &self.scope)?;
        if self.items.is_empty() || self.items.len() > 512 {
            return Err(ApiError::invalid(
                "save batches must contain between 1 and 512 items",
            ));
        }
        for item in &self.items {
            item.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    #[default]
    Auto,
    Draft,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSourceOrigin {
    #[default]
    User,
    External,
    Agent,
    Tool,
    System,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
pub struct CaptureSource {
    /// Existing Brunn source reference. When omitted, capture creates the
    /// source episode in the same atomic save as the extracted records.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// Stable identity in the source system, such as a conversation turn or
    /// message ID. This is provenance, not a Brunn record reference.
    pub external_ref: Option<String>,
    pub title: Option<String>,
    #[serde(default = "default_capture_source_kind")]
    pub kind: String,
    #[serde(default)]
    pub origin: CaptureSourceOrigin,
    pub media_type: Option<String>,
    pub locator: Option<Value>,
    pub metadata: Option<Value>,
    pub content_hash: Option<String>,
}

fn default_capture_source_kind() -> String {
    "user_input".to_owned()
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct CaptureRequest {
    pub content: String,
    pub source: CaptureSource,
    pub scope: String,
    #[serde(default)]
    pub root_refs: Vec<String>,
    pub intent: Option<String>,
    pub idempotency_key: Option<String>,
    pub base_corpus_revision: Option<String>,
    #[serde(default)]
    pub mode: CaptureMode,
}

impl CaptureRequest {
    pub fn validate(&self) -> ApiResult<()> {
        if self.content.trim().is_empty() {
            return Err(ApiError::invalid("capture content is required"));
        }
        if self.content.len() > 256_000 {
            return Err(ApiError::invalid(
                "capture content exceeds 256,000 characters; use memory.stage",
            ));
        }
        validate_namespaced_identifier("scope", &self.scope)?;
        if self.root_refs.len() > 64 {
            return Err(ApiError::invalid(
                "capture root_refs are limited to 64 references",
            ));
        }
        if self
            .source
            .reference
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
            && self
                .source
                .title
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(ApiError::invalid(
                "capture source requires either an existing ref or a title",
            ));
        }
        if self.source.kind.trim().is_empty() || self.source.kind.len() > 120 {
            return Err(ApiError::invalid(
                "capture source kind must contain between 1 and 120 characters",
            ));
        }
        if self
            .source
            .title
            .as_ref()
            .is_some_and(|value| value.len() > 500)
        {
            return Err(ApiError::invalid(
                "capture source title is limited to 500 characters",
            ));
        }
        if self
            .source
            .external_ref
            .as_ref()
            .is_some_and(|value| value.len() > 2_000)
        {
            return Err(ApiError::invalid(
                "capture source external_ref is limited to 2,000 characters",
            ));
        }
        if self
            .idempotency_key
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 240)
        {
            return Err(ApiError::invalid(
                "capture idempotency_key is limited to 240 nonempty characters",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct SourceReference {
    #[serde(rename = "ref")]
    pub reference: String,
    pub span: Option<[usize; 2]>,
    pub content_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveAction {
    Create,
    Revise,
    Supersede,
    Retract,
    Relate,
    Tombstone,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveKind {
    Object,
    Claim,
    Evidence,
    Relation,
    StateAssignment,
    StateTransition,
    TemporalSpec,
    RecurrenceSpec,
    Checkpoint,
    Source,
    Policy,
    Asset,
    IdentityReview,
    ImportReceipt,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct SaveItem {
    pub action: SaveAction,
    pub kind: SaveKind,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub expected_version: Option<i32>,
    #[serde(default)]
    pub payload: Value,
}

impl SaveItem {
    pub fn validate(&self) -> ApiResult<()> {
        reject_global_lifecycle(&self.payload)?;
        if matches!(
            self.action,
            SaveAction::Revise
                | SaveAction::Supersede
                | SaveAction::Retract
                | SaveAction::Tombstone
        ) && self.reference.is_none()
        {
            return Err(ApiError::invalid(
                "revision, supersession, retraction, and tombstone items require ref",
            ));
        }
        match self.kind {
            SaveKind::Object => validate_object_payload(&self.payload),
            SaveKind::Claim | SaveKind::StateAssignment | SaveKind::StateTransition => {
                validate_claim_payload(&self.payload, self.kind)
            }
            SaveKind::Relation => validate_relation_payload(&self.payload),
            SaveKind::TemporalSpec => {
                serde_json::from_value::<TemporalSpecV1>(self.payload.clone())?.validate()
            }
            SaveKind::RecurrenceSpec => {
                serde_json::from_value::<RecurrenceSpecV1>(self.payload.clone())?.validate()
            }
            SaveKind::Checkpoint => validate_checkpoint_payload(&self.payload),
            SaveKind::IdentityReview => validate_identity_review_payload(&self.payload),
            SaveKind::Evidence
            | SaveKind::Source
            | SaveKind::Policy
            | SaveKind::Asset
            | SaveKind::ImportReceipt => Ok(()),
        }
    }
}

fn object_field<'a>(payload: &'a Value, field: &str) -> Option<&'a Value> {
    payload.as_object().and_then(|object| object.get(field))
}

fn validate_object_payload(payload: &Value) -> ApiResult<()> {
    let profiles = object_field(payload, "type_profiles")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::invalid("object payload requires type_profiles"))?;
    if profiles.is_empty() {
        return Err(ApiError::invalid(
            "object requires at least one type profile",
        ));
    }
    for profile in profiles {
        let profile = profile
            .as_str()
            .ok_or_else(|| ApiError::invalid("type_profiles must contain strings"))?;
        if !profile.starts_with("core.") && !profile.contains('.') {
            return Err(ApiError::invalid("custom type profiles must be namespaced"));
        }
    }
    Ok(())
}

fn validate_claim_payload(payload: &Value, kind: SaveKind) -> ApiResult<()> {
    let about = object_field(payload, "about_refs")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::invalid("claim payload requires about_refs"))?;
    if about.is_empty() {
        return Err(ApiError::invalid("a claim requires at least one about ref"));
    }
    let predicate = object_field(payload, "predicate")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("claim payload requires predicate"))?;
    validate_namespaced_identifier("predicate", predicate)?;
    let producer = object_field(payload, "producer_ref")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::invalid("claim payload requires producer_ref"))?;
    validate_namespaced_identifier("producer_ref", producer)?;
    require_enum_field(
        payload,
        "formation_method",
        &[
            "asserted",
            "extracted",
            "observed",
            "inferred",
            "synthesized",
            "action_result",
        ],
    )?;
    let expected_mode = match kind {
        SaveKind::StateAssignment => Some("state_assignment"),
        SaveKind::StateTransition => Some("state_transition"),
        _ => None,
    };
    if let Some(expected) = expected_mode {
        let actual = object_field(payload, "claim_mode")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::invalid("claim payload requires claim_mode"))?;
        if actual != expected {
            return Err(ApiError::invalid(format!(
                "{kind:?} claim_mode must be {expected}"
            )));
        }
    } else {
        require_enum_field(
            payload,
            "claim_mode",
            &[
                "descriptive",
                "preference",
                "instruction",
                "assessment",
                "hypothesis",
                "proposal",
            ],
        )?;
    }
    require_enum_field(
        payload,
        "support_state",
        &[
            "reported",
            "supported",
            "verified",
            "disputed",
            "refuted",
            "unknown",
        ],
    )?;
    for field in ["authority", "canonicality"] {
        if object_field(payload, field)
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(ApiError::invalid(format!("claim payload requires {field}")));
        }
    }
    let evidence = object_field(payload, "evidence_refs").and_then(Value::as_array);
    let lineage = object_field(payload, "lineage").and_then(Value::as_array);
    let formation = object_field(payload, "formation_method").and_then(Value::as_str);
    if evidence.is_none_or(Vec::is_empty)
        && lineage.is_none_or(Vec::is_empty)
        && formation != Some("asserted")
    {
        return Err(ApiError::invalid(
            "non-asserted claims require direct evidence or claim lineage",
        ));
    }
    if matches!(kind, SaveKind::StateAssignment | SaveKind::StateTransition) {
        let states = STATE_REGISTRY
            .iter()
            .find(|(machine, _)| *machine == predicate)
            .map(|(_, states)| *states)
            .ok_or_else(|| {
                ApiError::invalid("state assignment predicate is not a registered state machine")
            })?;
        let state = object_field(payload, "value")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::invalid("state assignment value must be a string"))?;
        if !states.contains(&state) {
            return Err(ApiError::invalid(format!(
                "{state} is not valid for {predicate}"
            )));
        }
    }
    if let Some(confidence) = object_field(payload, "confidence").and_then(Value::as_f64)
        && !(0.0..=1.0).contains(&confidence)
    {
        return Err(ApiError::invalid(
            "claim confidence must be between 0 and 1",
        ));
    }
    Ok(())
}

fn require_enum_field(payload: &Value, field: &str, allowed: &[&str]) -> ApiResult<String> {
    let value = object_field(payload, field)
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid(format!("claim payload requires {field}")))?;
    if !allowed.contains(&value) {
        return Err(ApiError::invalid(format!(
            "claim {field} must be one of {}",
            allowed.join(", ")
        )));
    }
    Ok(value.to_owned())
}

fn validate_identity_review_payload(payload: &Value) -> ApiResult<()> {
    let object = payload
        .as_object()
        .ok_or_else(|| ApiError::invalid("identity review payload must be an object"))?;
    for field in ["relation_ref", "reviewer_ref", "reason"] {
        if object
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(ApiError::invalid(format!(
                "identity review requires {field}"
            )));
        }
    }
    let decision = object
        .get("decision")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("identity review requires decision"))?;
    if !["approved", "rejected"].contains(&decision) {
        return Err(ApiError::invalid(
            "identity review decision must be approved or rejected",
        ));
    }
    if object
        .get("evidence_refs")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        return Err(ApiError::invalid(
            "identity review requires direct evidence_refs",
        ));
    }
    Ok(())
}

fn validate_relation_payload(payload: &Value) -> ApiResult<()> {
    let predicate = object_field(payload, "predicate")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("relation payload requires predicate"))?;
    validate_namespaced_identifier("predicate", predicate)?;
    let endpoints = object_field(payload, "endpoints")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::invalid("relation payload requires endpoints"))?;
    if endpoints.len() < 2 {
        return Err(ApiError::invalid(
            "a qualified relation requires at least two endpoints",
        ));
    }
    if predicate == "core.same_as"
        && object_field(payload, "identity_review_ref")
            .and_then(Value::as_str)
            .is_none()
    {
        return Err(ApiError::with_details(
            http::StatusCode::UNPROCESSABLE_ENTITY,
            "identity_review_required",
            "same_as requires an approved identity review; use possibly_same_as for candidates",
            serde_json::json!({"allowed_candidate_predicate": "core.possibly_same_as"}),
        ));
    }
    Ok(())
}

fn validate_checkpoint_payload(payload: &Value) -> ApiResult<()> {
    let object = payload
        .as_object()
        .ok_or_else(|| ApiError::invalid("checkpoint payload must be an object"))?;
    for field in [
        "ordered_goals",
        "state_refs",
        "acceptance_gates",
        "next_actions",
        "source_refs",
    ] {
        if let Some(value) = object.get(field)
            && !value.is_array()
        {
            return Err(ApiError::invalid(format!(
                "checkpoint {field} must be an array"
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct CheckpointRequest {
    pub session_id: String,
    pub parent_checkpoint_id: Option<String>,
    pub state: Value,
    #[serde(default)]
    pub source_refs: Vec<String>,
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct DreamCreateRequest {
    pub scope: String,
    #[serde(default)]
    pub root_refs: Vec<String>,
    #[serde(default)]
    pub trigger: Vec<String>,
    #[serde(default = "default_dream_kind")]
    pub job_type: String,
    #[serde(default)]
    pub repair_only: bool,
    pub compute_budget: Option<Value>,
    pub expected_query_families: Option<Vec<String>>,
    pub idempotency_key: String,
}

fn default_dream_kind() -> String {
    "deep_consolidation".to_owned()
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct DreamReviewRequest {
    pub decision: DreamReviewDecision,
    #[serde(alias = "note")]
    pub comment: Option<String>,
    pub expected_candidate_revision: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamReviewDecision {
    #[serde(alias = "approve")]
    Accept,
    Reject,
    Quarantine,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
pub struct ListQuery {
    pub cursor: Option<String>,
    pub limit: Option<usize>,
    pub scope: Option<String>,
    pub status: Option<String>,
    pub type_profile: Option<String>,
    pub query: Option<String>,
}

pub fn canonical_json(value: &Value) -> ApiResult<String> {
    fn sort(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut sorted = BTreeMap::new();
                for (key, value) in map {
                    sorted.insert(key, sort(value));
                }
                serde_json::to_value(sorted).expect("BTreeMap serialization")
            }
            Value::Array(items) => Value::Array(items.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    Ok(serde_json::to_string(&sort(value))?)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn temporal_contract_rejects_bad_intervals_and_zones() {
        let backwards = TemporalSpecV1::DateInterval {
            start: NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
            end_exclusive: Some(NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()),
        };
        assert!(backwards.validate().is_err());

        let bad_zone = TemporalSpecV1::Due {
            local: NaiveDate::from_ymd_opt(2026, 12, 12)
                .unwrap()
                .and_hms_opt(8, 30, 0)
                .unwrap(),
            time_zone: "PST".to_owned(),
        };
        assert!(bad_zone.validate().is_err());
    }

    #[test]
    fn same_as_requires_review() {
        let item = SaveItem {
            action: SaveAction::Create,
            kind: SaveKind::Relation,
            reference: None,
            expected_version: None,
            payload: json!({
                "predicate": "core.same_as",
                "endpoints": [
                    {"role": "identity", "ref": "object:a"},
                    {"role": "identity", "ref": "object:b"}
                ]
            }),
        };
        assert!(item.validate().is_err());
    }

    #[test]
    fn independent_state_registry_rejects_cross_machine_values() {
        let item = SaveItem {
            action: SaveAction::Create,
            kind: SaveKind::StateAssignment,
            reference: None,
            expected_version: None,
            payload: json!({
                "about_refs": ["object:event"],
                "predicate": "state:event.schedule@v1",
                "value": "paid",
                "formation_method": "asserted",
                "evidence_refs": []
            }),
        };
        assert!(item.validate().is_err());
    }

    #[test]
    fn claim_requires_explicit_authority_dimensions() {
        let item = SaveItem {
            action: SaveAction::Create,
            kind: SaveKind::Claim,
            reference: None,
            expected_version: None,
            payload: json!({
                "about_refs": ["object:person"],
                "predicate": "profile.favorite_color",
                "value": "green",
                "evidence_refs": ["evidence:note"]
            }),
        };

        assert!(item.validate().is_err());
    }

    #[test]
    fn fully_qualified_claim_is_valid() {
        let item = SaveItem {
            action: SaveAction::Create,
            kind: SaveKind::Claim,
            reference: None,
            expected_version: None,
            payload: json!({
                "about_refs": ["object:person"],
                "predicate": "profile.favorite_color",
                "value": "green",
                "producer_ref": "agent:writer",
                "formation_method": "extracted",
                "claim_mode": "descriptive",
                "support_state": "supported",
                "authority": "source_attributed",
                "canonicality": "candidate",
                "evidence_refs": ["evidence:note"]
            }),
        };

        item.validate().unwrap();
    }

    #[test]
    fn state_claim_requires_explicit_claim_mode() {
        let item = SaveItem {
            action: SaveAction::Create,
            kind: SaveKind::StateAssignment,
            reference: None,
            expected_version: None,
            payload: json!({
                "about_refs": ["object:event"],
                "predicate": "state:event.schedule@v1",
                "value": "scheduled",
                "producer_ref": "agent:writer",
                "formation_method": "asserted",
                "support_state": "reported",
                "authority": "source_attributed",
                "canonicality": "candidate",
                "evidence_refs": []
            }),
        };

        assert!(item.validate().is_err());
    }

    #[test]
    fn identity_review_requires_explicit_evidence_and_reviewer() {
        let item = SaveItem {
            action: SaveAction::Create,
            kind: SaveKind::IdentityReview,
            reference: None,
            expected_version: None,
            payload: json!({
                "relation_ref": "relation:candidate",
                "decision": "approved"
            }),
        };

        assert!(item.validate().is_err());
    }

    #[test]
    fn global_lifecycle_is_rejected_recursively() {
        assert!(reject_global_lifecycle(&json!({"nested": {"lifecycle_state": "done"}})).is_err());
    }

    #[test]
    fn query_rejects_silently_ignored_filters() {
        let spec = QuerySpec {
            id: None,
            goal: None,
            query: None,
            scope: None,
            modes: vec![RetrievalMode::Structured],
            r#where: Map::from_iter([("made_up".to_owned(), json!("value"))]),
            state_filter: None,
            expand: None,
            limit: 12,
        };

        assert!(spec.validate().is_err());
    }

    #[test]
    fn state_filter_rejects_unimplemented_historical_time() {
        let spec = QuerySpec {
            id: None,
            goal: None,
            query: None,
            scope: None,
            modes: vec![RetrievalMode::Structured],
            r#where: Map::new(),
            state_filter: Some(StateFilter {
                machine_ref: "state:event.schedule@v1".to_owned(),
                states: vec!["scheduled".to_owned()],
                valid_at: Some("2026-07-11T12:00:00Z".to_owned()),
            }),
            expand: None,
            limit: 12,
        };

        assert!(spec.validate().is_err());
    }
}
