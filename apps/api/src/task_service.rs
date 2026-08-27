use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::{
    Extension, Json,
    extract::{Path, RawQuery, State},
};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::{Capability, ResponseStatus, canonical_json},
    simple_core::{self, WorkspaceEnvelope},
    task_engine::{
        self, CandidateRequest as EngineCandidateRequest, CostOfDelay, CostPeriod, EngineSettings,
        ProjectInterest, Sourced, TaskSnapshot, TaskStatus, TaskView,
    },
    todoist_sync::{
        MappedRecurrence, MappedTodoistItem, TodoistCompletedOccurrence, TodoistSyncResponse,
        TodoistTerminal, map_item, next_todoist_occurrence,
    },
};

pub(crate) const TASK_ENTRY_PREFIX: &str = ".straylight/tasks/";
pub(crate) const TASK_SCHEMA: &str = "task.v1";

#[derive(Clone, Debug)]
struct Cell<'a> {
    value: &'a Value,
    source: Option<&'a str>,
    set_at: Option<&'a str>,
}

#[derive(Clone, Debug)]
struct TaskProjection {
    task_id: Uuid,
    title: String,
    status: String,
    ready_at: Option<DateTime<Utc>>,
    soft_due: Option<NaiveDate>,
    hard_due: Option<DateTime<Utc>>,
    hard_due_lead_days: Option<i32>,
    cost_amount_cents: Option<i64>,
    cost_period: Option<String>,
    cost_flag: bool,
    cost_since: Option<NaiveDate>,
    required_contexts: Vec<String>,
    project_slug: Option<String>,
    estimate_minutes: Option<i32>,
    waiting_on: Option<Value>,
    snooze_count: i32,
    parked: bool,
    today_pin: Option<NaiveDate>,
    triaged_at: Option<DateTime<Utc>>,
    done_at: Option<DateTime<Utc>>,
    dropped_at: Option<DateTime<Utc>>,
    recurrence: Option<Value>,
    provenance: Value,
    source_timestamps: Value,
    task: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContextSuggestion {
    pub(crate) slug: String,
    pub(crate) reason: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CorrectionDelta {
    pub(crate) field_name: String,
    pub(crate) previous_value: Value,
    pub(crate) previous_source: Option<String>,
    pub(crate) corrected_value: Value,
    pub(crate) corrected_source: String,
}

pub(crate) fn derive_project_interest(
    explicit: Option<(&str, DateTime<Utc>)>,
    last_activity_at: Option<DateTime<Utc>>,
    as_of: DateTime<Utc>,
) -> ProjectInterest {
    if let Some((interest, set_at)) = explicit
        && set_at <= as_of
        && as_of.signed_duration_since(set_at) < chrono::Duration::days(14)
    {
        return match interest {
            "hot" => ProjectInterest::Hot,
            "parked" => ProjectInterest::Parked,
            _ => ProjectInterest::Normal,
        };
    }
    match last_activity_at
        .filter(|activity| *activity <= as_of)
        .map(|activity| as_of.signed_duration_since(activity))
    {
        Some(age) if age <= chrono::Duration::days(7) => ProjectInterest::Hot,
        Some(age) if age < chrono::Duration::days(60) => ProjectInterest::Normal,
        _ => ProjectInterest::Parked,
    }
}

pub(crate) fn task_id_from_path(path: &str) -> Option<Uuid> {
    let raw = path.strip_prefix(TASK_ENTRY_PREFIX)?.strip_suffix(".md")?;
    let task_id = Uuid::parse_str(raw).ok()?;
    (raw == task_id.to_string() && task_id.get_version_num() == 7).then_some(task_id)
}

pub(crate) fn is_task_metadata(metadata: &Value) -> bool {
    effective_metadata(metadata)
        .get("kind")
        .and_then(Value::as_str)
        == Some("task")
}

pub(crate) fn validate_task_entry(path: &str, metadata: &Value) -> ApiResult<bool> {
    let managed_task_path = task_id_from_path(path);
    let metadata = effective_metadata(metadata);
    if metadata.get("kind").and_then(Value::as_str) != Some("task") {
        if metadata.get("schema").and_then(Value::as_str) == Some(TASK_SCHEMA) {
            return Err(ApiError::invalid("task.v1 metadata must declare kind task"));
        }
        return Ok(false);
    }
    let path_task_id = managed_task_path.ok_or_else(|| {
        ApiError::invalid("task metadata is allowed only at .straylight/tasks/<uuid>.md")
    })?;
    if metadata.get("schema").and_then(Value::as_str) != Some(TASK_SCHEMA) {
        return Err(ApiError::invalid("task metadata schema must be task.v1"));
    }
    let projection = parse_projection(metadata)?;
    if projection.task_id != path_task_id {
        return Err(ApiError::invalid(
            "task metadata id must match the canonical task entry path",
        ));
    }
    Ok(true)
}

#[doc(hidden)]
pub async fn sync_managed_entry_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    entry_id: Uuid,
    entry_version: i64,
    path: &str,
    metadata: &Value,
) -> ApiResult<()> {
    if task_id_from_path(path).is_some() || is_task_metadata(metadata) {
        sync_task_projection_in_tx(tx, user_id, entry_id, entry_version, path, metadata).await?;
    }
    if effective_metadata(metadata)
        .get("kind")
        .and_then(Value::as_str)
        == Some("checkpoint")
    {
        sync_checkpoint_link_in_tx(tx, user_id, entry_id, metadata).await?;
    }
    Ok(())
}

pub(crate) async fn delete_task_projection_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    entry_id: Uuid,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM straylight.task_todoist_occurrences WHERE user_id=$1 AND entry_id=$2")
        .bind(user_id)
        .bind(entry_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM straylight.task_external_refs WHERE user_id=$1 AND entry_id=$2")
        .bind(user_id)
        .bind(entry_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM straylight.task_index WHERE user_id=$1 AND entry_id=$2")
        .bind(user_id)
        .bind(entry_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) fn apply_sourced_field(
    metadata: &mut Value,
    field_name: &str,
    value: Value,
    source: &str,
    set_at: DateTime<Utc>,
    note: Option<&str>,
    explicit_correction: bool,
) -> ApiResult<Option<CorrectionDelta>> {
    const ENRICHABLE_FIELDS: &[&str] = &[
        "notes",
        "project",
        "status",
        "ready_at",
        "soft_due",
        "hard_due",
        "hard_due_lead_days",
        "cost_of_delay",
        "required_contexts",
        "estimate_minutes",
        "waiting_on",
        "snooze_count",
        "parked",
        "triaged_at",
        "today_pin",
        "recurrence",
        "completed_via",
        "dropped_reason",
    ];
    if !ENRICHABLE_FIELDS.contains(&field_name) {
        return Err(ApiError::invalid(format!(
            "{field_name} is not a sourced task field"
        )));
    }
    validate_source(source)?;
    let task = effective_metadata_mut(metadata)
        .get_mut("task")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ApiError::invalid("task.v1 metadata requires a task object"))?;
    let previous = task.get(field_name).cloned().unwrap_or(Value::Null);
    let (previous_value, previous_source) = previous
        .as_object()
        .filter(|cell| cell.contains_key("value"))
        .map(|cell| {
            (
                cell.get("value").cloned().unwrap_or(Value::Null),
                cell.get("source")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            )
        })
        .unwrap_or_else(|| (previous.clone(), None));
    if previous_value == value && previous_source.as_deref() == Some(source) {
        return Ok(None);
    }
    match previous_source.as_deref() {
        Some("owner") if source != "owner" => {
            return Err(ApiError::conflict(
                "task_owner_value_precedence",
                "owner-set task fields cannot be overwritten by agents or integrations",
                json!({"field": field_name}),
            ));
        }
        Some(previous_source)
            if (previous_source == "derived" || previous_source.starts_with("agent:"))
                && source != "owner"
                && !explicit_correction =>
        {
            return Err(ApiError::conflict(
                "task_correction_required",
                "agent-set task fields require an explicit correction before replacement",
                json!({"field": field_name}),
            ));
        }
        Some("todoist")
            if source != "owner" && source != "todoist" && !source.starts_with("agent:") =>
        {
            return Err(ApiError::conflict(
                "task_source_precedence",
                "derived values cannot overwrite Todoist-sourced task fields",
                json!({"field": field_name}),
            ));
        }
        Some(previous_source)
            if source == "todoist"
                && previous_source != "todoist"
                && previous_source != "derived" =>
        {
            return Err(ApiError::conflict(
                "task_source_precedence",
                "Todoist can refresh only fields that remain Todoist-sourced",
                json!({"field": field_name}),
            ));
        }
        _ => {}
    }
    task.insert(
        field_name.to_owned(),
        json!({
            "value": value.clone(),
            "source": source,
            "set_at": set_at,
            "note": note
        }),
    );
    Ok(Some(CorrectionDelta {
        field_name: field_name.to_owned(),
        previous_value,
        previous_source,
        corrected_value: value,
        corrected_source: source.to_owned(),
    }))
}

pub(crate) async fn context_suggestions_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    candidate: &str,
) -> ApiResult<Vec<ContextSuggestion>> {
    let normalized = normalize_slug(candidate)?;
    let rows = sqlx::query(
        r#"
        SELECT context.slug,alias.alias
        FROM straylight.task_contexts AS context
        LEFT JOIN straylight.task_context_aliases AS alias
          ON alias.user_id=context.user_id AND alias.context_slug=context.slug
        WHERE context.user_id=$1 AND context.archived_at IS NULL
        ORDER BY context.slug,alias.alias
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut reasons = BTreeMap::<String, &'static str>::new();
    for row in rows {
        let slug: String = row.get("slug");
        let alias = row.get::<Option<String>, _>("alias");
        let reason = if slug == normalized
            || alias
                .as_deref()
                .and_then(|value| normalize_slug(value).ok())
                .as_deref()
                == Some(normalized.as_str())
        {
            Some("exact_or_alias")
        } else if shared_token(&slug, &normalized) {
            Some("shared_token")
        } else if damerau_levenshtein(&slug, &normalized) <= 2 {
            Some("small_edit")
        } else {
            None
        };
        if let Some(reason) = reason {
            reasons
                .entry(slug)
                .and_modify(|current| {
                    if suggestion_priority(reason) < suggestion_priority(current) {
                        *current = reason;
                    }
                })
                .or_insert(reason);
        }
    }
    Ok(reasons
        .into_iter()
        .map(|(slug, reason)| ContextSuggestion { slug, reason })
        .collect())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_context_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    credential_id: Uuid,
    slug_or_name: &str,
    display_name_override: Option<&str>,
    description: Option<&str>,
    source: &str,
    confirm_new: bool,
) -> ApiResult<String> {
    validate_source(source)?;
    let slug = normalize_slug(slug_or_name)?;
    let suggestions = context_suggestions_in_tx(tx, user_id, &slug).await?;
    if !suggestions.is_empty() && !confirm_new {
        return Err(ApiError::conflict(
            "context_confirmation_required",
            "a similar context already exists; confirm_new is required",
            json!({
                "suggested_existing": suggestions.iter().map(|suggestion| json!({
                    "slug": suggestion.slug,
                    "reason": suggestion.reason
                })).collect::<Vec<_>>()
            }),
        ));
    }
    let display_name = display_name_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| display_name(&slug));
    sqlx::query(
        r#"
        INSERT INTO straylight.task_contexts (
          user_id,slug,display_name,description,created_by
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(user_id)
    .bind(&slug)
    .bind(display_name)
    .bind(description)
    .bind(source)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.task_audit_events (
          user_id,credential_id,action,details
        ) VALUES ($1,$2,'context.create',$3)
        "#,
    )
    .bind(user_id)
    .bind(credential_id)
    .bind(json!({"slug": slug, "source": source}))
    .execute(&mut **tx)
    .await?;
    Ok(slug)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn merge_contexts_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    credential_id: Uuid,
    from: &str,
    into: &str,
    source: &str,
    as_of: DateTime<Utc>,
) -> ApiResult<usize> {
    validate_source(source)?;
    let from = normalize_slug(from)?;
    let into = normalize_slug(into)?;
    if from == into {
        return Err(ApiError::invalid(
            "context merge source and target must differ",
        ));
    }
    let rows = sqlx::query(
        r#"
        SELECT slug,archived_at
        FROM straylight.task_contexts
        WHERE user_id=$1 AND slug=ANY($2)
        ORDER BY slug
        "#,
    )
    .bind(user_id)
    .bind(vec![from.clone(), into.clone()])
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != 2
        || rows
            .iter()
            .any(|row| row.get::<Option<DateTime<Utc>>, _>("archived_at").is_some())
    {
        return Err(ApiError::not_found(
            "context_not_found",
            "context merge source or target",
        ));
    }
    let task_rows = sqlx::query(
        r#"
        SELECT task.task_id,entry.id AS entry_id,entry.path,entry.current_version,
               version.content,version.metadata
        FROM straylight.task_index AS task
        JOIN straylight.entries AS entry
          ON entry.user_id=task.user_id AND entry.id=task.entry_id
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE task.user_id=$1 AND task.required_contexts @> ARRAY[$2]::text[]
        ORDER BY task.task_id
        FOR UPDATE OF entry
        "#,
    )
    .bind(user_id)
    .bind(&from)
    .fetch_all(&mut **tx)
    .await?;
    let mut rewritten = 0_usize;
    for row in task_rows {
        let task_id: Uuid = row.get("task_id");
        let entry_id: Uuid = row.get("entry_id");
        let path: String = row.get("path");
        let current_version: i64 = row.get("current_version");
        let content: String = row.get("content");
        let mut metadata: Value = row.get("metadata");
        let correction =
            rewrite_context_cell_for_merge(&mut metadata, &from, &into, source, as_of)?;
        let prepared = simple_core::prepare_task_markdown_for_update(
            path,
            content,
            metadata,
            current_version,
        )?;
        let result =
            simple_core::upsert_markdown_in_tx(tx, user_id, Some(credential_id), prepared).await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.task_corrections (
              user_id,task_id,entry_id,entry_version,field_name,
              previous_value,previous_source,corrected_value,corrected_source,
              reason,credential_id
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            "#,
        )
        .bind(user_id)
        .bind(task_id)
        .bind(entry_id)
        .bind(result.version)
        .bind(&correction.field_name)
        .bind(&correction.previous_value)
        .bind(&correction.previous_source)
        .bind(&correction.corrected_value)
        .bind(&correction.corrected_source)
        .bind("explicit context merge")
        .bind(credential_id)
        .execute(&mut **tx)
        .await?;
        rewritten += 1;
    }
    sqlx::query(
        r#"
        UPDATE straylight.task_context_aliases
        SET context_slug=$3,reason='merge'
        WHERE user_id=$1 AND context_slug=$2
        "#,
    )
    .bind(user_id)
    .bind(&from)
    .bind(&into)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.task_context_aliases (
          user_id,alias,context_slug,reason
        ) VALUES ($1,$2,$3,'merge')
        ON CONFLICT (user_id,alias) DO UPDATE SET
          context_slug=EXCLUDED.context_slug,
          reason='merge'
        "#,
    )
    .bind(user_id)
    .bind(&from)
    .bind(&into)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.task_contexts
        SET archived_at=$3,updated_at=$3,version=version+1
        WHERE user_id=$1 AND slug=$2
        "#,
    )
    .bind(user_id)
    .bind(&from)
    .bind(as_of)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.task_surface_defaults AS defaults
        SET contexts=(
          SELECT array_agg(value ORDER BY first_ordinal)
          FROM (
            SELECT CASE WHEN item=$2 THEN $3 ELSE item END AS value,
                   min(ordinality) AS first_ordinal
            FROM unnest(defaults.contexts) WITH ORDINALITY AS expanded(item,ordinality)
            GROUP BY CASE WHEN item=$2 THEN $3 ELSE item END
          ) AS deduplicated
        ),updated_at=$4,version=defaults.version+1
        WHERE defaults.user_id=$1 AND defaults.contexts @> ARRAY[$2]::text[]
        "#,
    )
    .bind(user_id)
    .bind(&from)
    .bind(&into)
    .bind(as_of)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.task_audit_events (
          user_id,credential_id,action,details
        ) VALUES ($1,$2,'context.merge',$3)
        "#,
    )
    .bind(user_id)
    .bind(credential_id)
    .bind(json!({"from": from, "into": into, "tasks_rewritten": rewritten}))
    .execute(&mut **tx)
    .await?;
    Ok(rewritten)
}

fn rewrite_context_cell_for_merge(
    metadata: &mut Value,
    from: &str,
    into: &str,
    structural_source: &str,
    as_of: DateTime<Utc>,
) -> ApiResult<CorrectionDelta> {
    let task = effective_metadata_mut(metadata)
        .get_mut("task")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ApiError::invalid("task.v1 metadata requires a task object"))?;
    let cell = task.get_mut("required_contexts").ok_or_else(|| {
        ApiError::invalid("required_contexts must be present for a context merge")
    })?;
    let previous_value = cell.get("value").unwrap_or(cell).clone();
    let previous_source = cell
        .get("source")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let values = previous_value
        .as_array()
        .ok_or_else(|| ApiError::invalid("required_contexts must be an array"))?;
    let mut next = BTreeSet::new();
    for value in values {
        let value = value
            .as_str()
            .ok_or_else(|| ApiError::invalid("required_contexts must contain strings"))?;
        next.insert(if value == from {
            into.to_owned()
        } else {
            value.to_owned()
        });
    }
    if !next.contains(into) || !values.iter().any(|value| value.as_str() == Some(from)) {
        return Err(ApiError::conflict(
            "task_projection_stale",
            "task projection and canonical required contexts disagree",
            json!({"from": from, "into": into}),
        ));
    }
    let corrected_value = json!(next.into_iter().collect::<Vec<_>>());
    let corrected_source = if let Some(object) = cell.as_object_mut() {
        object.insert("value".to_owned(), corrected_value.clone());
        previous_source
            .clone()
            .unwrap_or_else(|| structural_source.to_owned())
    } else {
        *cell = json!({
            "value": corrected_value.clone(),
            "source": structural_source,
            "set_at": as_of,
            "note": "context registry merge"
        });
        structural_source.to_owned()
    };
    Ok(CorrectionDelta {
        field_name: "required_contexts".to_owned(),
        previous_value,
        previous_source,
        corrected_value,
        corrected_source,
    })
}

async fn sync_task_identity_projection_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    task_id: Uuid,
    entry_id: Uuid,
    task: &Value,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM straylight.task_todoist_occurrences WHERE user_id=$1 AND task_id=$2")
        .bind(user_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "DELETE FROM straylight.task_external_refs WHERE user_id=$1 AND system='todoist' AND task_id=$2",
    )
    .bind(user_id)
    .bind(task_id)
    .execute(&mut **tx)
    .await?;
    let refs = task
        .get("external_refs")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| ApiError::invalid("task external_refs must be an array"))
        })
        .transpose()?
        .cloned()
        .unwrap_or_default();
    for external in refs {
        let external = external
            .as_object()
            .ok_or_else(|| ApiError::invalid("task external_refs entries must be objects"))?;
        if external.get("system").and_then(Value::as_str) != Some("todoist") {
            continue;
        }
        let external_id = external
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 512)
            .ok_or_else(|| ApiError::invalid("Todoist external ref id is invalid"))?;
        let series_id = external.get("series_id").and_then(Value::as_str);
        let occurrence_key = external.get("occurrence_key").and_then(Value::as_str);
        if series_id.is_some() != occurrence_key.is_some()
            || series_id.is_some_and(|value| value.is_empty() || value.len() > 512)
            || occurrence_key.is_some_and(|value| value.is_empty() || value.len() > 512)
        {
            return Err(ApiError::invalid(
                "Todoist series identity must contain bounded series_id and occurrence_key",
            ));
        }
        if let (Some(series_id), Some(occurrence_key)) = (series_id, occurrence_key) {
            let inserted = sqlx::query(
                r#"
                INSERT INTO straylight.task_todoist_occurrences(
                  user_id,series_id,occurrence_key,task_id,entry_id
                ) VALUES($1,$2,$3,$4,$5)
                ON CONFLICT(user_id,series_id,occurrence_key) DO UPDATE SET
                  entry_id=EXCLUDED.entry_id
                WHERE task_todoist_occurrences.task_id=EXCLUDED.task_id
                "#,
            )
            .bind(user_id)
            .bind(series_id)
            .bind(occurrence_key)
            .bind(task_id)
            .bind(entry_id)
            .execute(&mut **tx)
            .await?;
            if inserted.rows_affected() != 1 {
                return Err(ApiError::conflict(
                    "todoist_occurrence_conflict",
                    "Todoist occurrence identity belongs to another canonical task",
                    json!({"series_id":series_id,"occurrence_key":occurrence_key}),
                ));
            }
        }
        if external.get("current").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        sqlx::query(
            r#"
            INSERT INTO straylight.task_external_refs(
              user_id,system,external_id,task_id,entry_id,series_id,
              occurrence_key,metadata
            ) VALUES($1,'todoist',$2,$3,$4,$5,$6,$7)
            ON CONFLICT(user_id,system,external_id) DO UPDATE SET
              task_id=EXCLUDED.task_id,entry_id=EXCLUDED.entry_id,
              series_id=EXCLUDED.series_id,occurrence_key=EXCLUDED.occurrence_key,
              metadata=EXCLUDED.metadata,last_seen_at=clock_timestamp()
            "#,
        )
        .bind(user_id)
        .bind(external_id)
        .bind(task_id)
        .bind(entry_id)
        .bind(series_id)
        .bind(occurrence_key)
        .bind(json!({
            "url":external.get("url").cloned().unwrap_or(Value::Null),
            "project_id":external.get("project_id").cloned().unwrap_or(Value::Null),
        }))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn sync_task_projection_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    entry_id: Uuid,
    entry_version: i64,
    path: &str,
    metadata: &Value,
) -> ApiResult<()> {
    if !validate_task_entry(path, metadata)? {
        delete_task_projection_in_tx(tx, user_id, entry_id).await?;
        return Ok(());
    }
    let metadata = effective_metadata(metadata);
    let projection = parse_projection(metadata)?;
    let version_created_at = sqlx::query_scalar::<_, DateTime<Utc>>(
        r#"
        SELECT created_at
        FROM straylight.entry_versions
        WHERE user_id=$1 AND entry_id=$2 AND version=$3
        "#,
    )
    .bind(user_id)
    .bind(entry_id)
    .bind(entry_version)
    .fetch_one(&mut **tx)
    .await?;
    let captured_at = projection
        .task
        .get("provenance")
        .and_then(|value| value.get("created_at"))
        .and_then(Value::as_str)
        .and_then(parse_timestamp)
        .unwrap_or(version_created_at);

    if let Some(project_slug) = projection.project_slug.as_deref() {
        let project_source = cell(
            projection
                .task
                .as_object()
                .expect("validated task is an object"),
            "project",
        )?
        .and_then(|value| value.source)
        .unwrap_or("derived");
        sqlx::query(
            r#"
            INSERT INTO straylight.task_projects (
              user_id,slug,title,created_by,last_activity_at
            ) VALUES ($1,$2,$3,$4,$5)
            ON CONFLICT (user_id,slug) DO UPDATE SET
              last_activity_at=GREATEST(
                task_projects.last_activity_at,
                EXCLUDED.last_activity_at
              ),
              updated_at=GREATEST(task_projects.updated_at,EXCLUDED.last_activity_at)
            "#,
        )
        .bind(user_id)
        .bind(project_slug)
        .bind(display_name(project_slug))
        .bind(project_source)
        .bind(version_created_at)
        .execute(&mut **tx)
        .await?;
    }
    let context_source = cell(
        projection
            .task
            .as_object()
            .expect("validated task is an object"),
        "required_contexts",
    )?
    .and_then(|value| value.source)
    .unwrap_or("derived");
    for context in &projection.required_contexts {
        sqlx::query(
            r#"
            INSERT INTO straylight.task_contexts (
              user_id,slug,display_name,created_by
            ) VALUES ($1,$2,$3,$4)
            ON CONFLICT (user_id,slug) DO NOTHING
            "#,
        )
        .bind(user_id)
        .bind(context)
        .bind(display_name(context))
        .bind(context_source)
        .execute(&mut **tx)
        .await?;
    }

    sqlx::query(
        r#"
        INSERT INTO straylight.task_index (
          user_id,task_id,entry_id,entry_version,title,status,ready_at,
          soft_due,hard_due,hard_due_lead_days,cost_amount_cents,cost_period,
          cost_flag,cost_since,required_contexts,project_slug,estimate_minutes,
          waiting_on,snooze_count,parked,today_pin,triaged_at,done_at,dropped_at,
          recurrence,provenance,source_timestamps,task,created_at,updated_at
        ) VALUES (
          $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,
          $19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30
        )
        ON CONFLICT (user_id,task_id) DO UPDATE SET
          entry_id=EXCLUDED.entry_id,
          entry_version=EXCLUDED.entry_version,
          title=EXCLUDED.title,
          status=EXCLUDED.status,
          ready_at=EXCLUDED.ready_at,
          soft_due=EXCLUDED.soft_due,
          hard_due=EXCLUDED.hard_due,
          hard_due_lead_days=EXCLUDED.hard_due_lead_days,
          cost_amount_cents=EXCLUDED.cost_amount_cents,
          cost_period=EXCLUDED.cost_period,
          cost_flag=EXCLUDED.cost_flag,
          cost_since=EXCLUDED.cost_since,
          required_contexts=EXCLUDED.required_contexts,
          project_slug=EXCLUDED.project_slug,
          estimate_minutes=EXCLUDED.estimate_minutes,
          waiting_on=EXCLUDED.waiting_on,
          snooze_count=EXCLUDED.snooze_count,
          parked=EXCLUDED.parked,
          today_pin=EXCLUDED.today_pin,
          triaged_at=EXCLUDED.triaged_at,
          done_at=EXCLUDED.done_at,
          dropped_at=EXCLUDED.dropped_at,
          recurrence=EXCLUDED.recurrence,
          provenance=EXCLUDED.provenance,
          source_timestamps=EXCLUDED.source_timestamps,
          task=EXCLUDED.task,
          created_at=LEAST(task_index.created_at,EXCLUDED.created_at),
          updated_at=EXCLUDED.updated_at
        "#,
    )
    .bind(user_id)
    .bind(projection.task_id)
    .bind(entry_id)
    .bind(entry_version)
    .bind(&projection.title)
    .bind(&projection.status)
    .bind(projection.ready_at)
    .bind(projection.soft_due)
    .bind(projection.hard_due)
    .bind(projection.hard_due_lead_days)
    .bind(projection.cost_amount_cents)
    .bind(&projection.cost_period)
    .bind(projection.cost_flag)
    .bind(projection.cost_since)
    .bind(&projection.required_contexts)
    .bind(&projection.project_slug)
    .bind(projection.estimate_minutes)
    .bind(&projection.waiting_on)
    .bind(projection.snooze_count)
    .bind(projection.parked)
    .bind(projection.today_pin)
    .bind(projection.triaged_at)
    .bind(projection.done_at)
    .bind(projection.dropped_at)
    .bind(&projection.recurrence)
    .bind(&projection.provenance)
    .bind(&projection.source_timestamps)
    .bind(&projection.task)
    .bind(captured_at)
    .bind(version_created_at)
    .execute(&mut **tx)
    .await?;
    sync_task_identity_projection_in_tx(
        tx,
        user_id,
        projection.task_id,
        entry_id,
        &projection.task,
    )
    .await?;
    Ok(())
}

async fn sync_checkpoint_link_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    checkpoint_entry_id: Uuid,
    metadata: &Value,
) -> ApiResult<()> {
    let metadata = effective_metadata(metadata);
    let state = metadata.get("checkpoint_state").unwrap_or(&Value::Null);
    let explicit = metadata
        .get("project")
        .or_else(|| state.get("project"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let mut references = Vec::new();
    collect_string_values(metadata.get("source_refs"), &mut references);
    collect_string_values(state.get("state_refs"), &mut references);
    if let Some(entries) = metadata.get("source_entries").and_then(Value::as_array) {
        for entry in entries {
            if let Some(path) = entry.get("path").and_then(Value::as_str) {
                references.push(path.to_owned());
            }
        }
    }

    let match_result = if let Some(project_slug) = explicit {
        validate_slug(project_slug, "project")?;
        let exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
              SELECT 1 FROM straylight.task_projects
              WHERE user_id=$1 AND slug=$2 AND archived_at IS NULL
            )
            "#,
        )
        .bind(user_id)
        .bind(project_slug)
        .fetch_one(&mut **tx)
        .await?;
        if !exists {
            return Err(ApiError::invalid(format!(
                "checkpoint project {project_slug} is not registered"
            )));
        }
        Some((project_slug.to_owned(), "explicit", None))
    } else {
        let rows = sqlx::query(
            r#"
            SELECT slug,hub_path,repo_path
            FROM straylight.task_projects
            WHERE user_id=$1 AND archived_at IS NULL
            "#,
        )
        .bind(user_id)
        .fetch_all(&mut **tx)
        .await?;
        let mut best: Option<(usize, String, String)> = None;
        for row in rows {
            let slug: String = row.get("slug");
            let mut prefixes = Vec::new();
            if let Some(hub_path) = row.get::<Option<String>, _>("hub_path") {
                prefixes.push(hub_path.clone());
                if let Some((parent, _)) = hub_path.rsplit_once('/') {
                    prefixes.push(format!("{parent}/"));
                }
            }
            if let Some(repo_path) = row.get::<Option<String>, _>("repo_path") {
                prefixes.push(repo_path);
            }
            for prefix in prefixes {
                if references
                    .iter()
                    .any(|reference| path_prefix_matches(reference, &prefix))
                    && best.as_ref().is_none_or(|current| {
                        prefix.len() > current.0 || (prefix.len() == current.0 && slug < current.1)
                    })
                {
                    best = Some((prefix.len(), slug.clone(), prefix));
                }
            }
        }
        best.map(|(_, slug, matched)| (slug, "path_fallback", Some(matched)))
    };

    if let Some((project_slug, attribution, matched_path)) = match_result {
        sqlx::query(
            r#"
            INSERT INTO straylight.task_checkpoint_links (
              user_id,checkpoint_entry_id,project_slug,attribution,matched_path
            ) VALUES ($1,$2,$3,$4,$5)
            ON CONFLICT (user_id,checkpoint_entry_id) DO UPDATE SET
              project_slug=EXCLUDED.project_slug,
              attribution=EXCLUDED.attribution,
              matched_path=EXCLUDED.matched_path,
              linked_at=clock_timestamp()
            "#,
        )
        .bind(user_id)
        .bind(checkpoint_entry_id)
        .bind(&project_slug)
        .bind(attribution)
        .bind(matched_path)
        .execute(&mut **tx)
        .await?;
        sqlx::query("SELECT straylight.touch_task_project_from_checkpoint($1,$2,$3)")
            .bind(user_id)
            .bind(checkpoint_entry_id)
            .bind(project_slug)
            .execute(&mut **tx)
            .await?;
    } else {
        sqlx::query(
            "DELETE FROM straylight.task_checkpoint_links WHERE user_id=$1 AND checkpoint_entry_id=$2",
        )
        .bind(user_id)
        .bind(checkpoint_entry_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn path_prefix_matches(reference: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    reference == prefix
        || reference
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn parse_projection(metadata: &Value) -> ApiResult<TaskProjection> {
    let task = metadata
        .get("task")
        .and_then(Value::as_object)
        .ok_or_else(|| ApiError::invalid("task.v1 metadata requires a task object"))?;
    let task_id = task
        .get("id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| ApiError::invalid("task id must be a UUID"))?;
    let title = task
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty() && value.len() <= 500 && !has_forbidden_control(value, false)
        })
        .ok_or_else(|| ApiError::invalid("task title must contain 1 to 500 characters"))?
        .to_owned();
    let status = string_value(task, "status")?.unwrap_or_else(|| "open".to_owned());
    if !["open", "waiting", "done", "dropped"].contains(&status.as_str()) {
        return Err(ApiError::invalid(
            "task status must be open, waiting, done, or dropped",
        ));
    }
    if let Some(notes) = string_value(task, "notes")?
        && (notes.len() > 20_000 || has_forbidden_control(&notes, true))
    {
        return Err(ApiError::invalid(
            "task notes must be printable and at most 20000 characters",
        ));
    }
    let ready_at = timestamp_value(task, "ready_at")?;
    let soft_due = date_value(task, "soft_due")?;
    let hard_due = timestamp_value(task, "hard_due")?;
    let hard_due_lead_days = integer_value(task, "hard_due_lead_days")?
        .map(i32::try_from)
        .transpose()
        .map_err(|_| ApiError::invalid("hard_due_lead_days is out of range"))?
        .filter(|value| (0..=3650).contains(value));
    if task.get("hard_due_lead_days").is_some()
        && integer_value(task, "hard_due_lead_days")?.is_some()
        && hard_due_lead_days.is_none()
    {
        return Err(ApiError::invalid(
            "hard_due_lead_days must be null or 0..3650",
        ));
    }
    let required_contexts = string_array_value(task, "required_contexts")?;
    if required_contexts.len() > 20 {
        return Err(ApiError::invalid(
            "required_contexts accepts at most 20 values",
        ));
    }
    for context in &required_contexts {
        validate_slug(context, "context")?;
    }
    let project_slug = string_value(task, "project")?.filter(|value| !value.is_empty());
    if let Some(project) = project_slug.as_deref() {
        validate_slug(project, "project")?;
    }
    let estimate_minutes = integer_value(task, "estimate_minutes")?
        .map(i32::try_from)
        .transpose()
        .map_err(|_| ApiError::invalid("estimate_minutes is out of range"))?
        .filter(|value| (1..=10080).contains(value));
    if task.get("estimate_minutes").is_some()
        && integer_value(task, "estimate_minutes")?.is_some()
        && estimate_minutes.is_none()
    {
        return Err(ApiError::invalid(
            "estimate_minutes must be null or 1..10080",
        ));
    }
    let waiting_on = owned_value(task, "waiting_on")?;
    if waiting_on.as_ref().is_some_and(|value| !value.is_object()) {
        return Err(ApiError::invalid("waiting_on must be an object"));
    }
    if waiting_on
        .as_ref()
        .is_some_and(|value| json_has_forbidden_control(value, true))
    {
        return Err(ApiError::invalid(
            "waiting_on must not contain control characters",
        ));
    }
    let snooze_count = integer_value(task, "snooze_count")?.unwrap_or(0);
    let snooze_count = i32::try_from(snooze_count)
        .ok()
        .filter(|value| *value >= 0)
        .ok_or_else(|| ApiError::invalid("snooze_count must be a nonnegative integer"))?;
    let parked = boolean_value(task, "parked")?.unwrap_or(false);
    let today_pin = date_value(task, "today_pin")?;
    let triaged_at = timestamp_value(task, "triaged_at")?;
    let done_at = direct_timestamp(task, "done_at")?;
    let dropped_at = direct_timestamp(task, "dropped_at")?;
    let recurrence = owned_value(task, "recurrence")?;
    if recurrence
        .as_ref()
        .is_some_and(|value| json_has_forbidden_control(value, false))
    {
        return Err(ApiError::invalid(
            "recurrence must not contain control characters",
        ));
    }
    let (cost_amount_cents, cost_period, cost_flag, cost_since) = parse_cost(task)?;
    let (provenance, source_timestamps) = collect_cell_provenance(task)?;

    Ok(TaskProjection {
        task_id,
        title,
        status,
        ready_at,
        soft_due,
        hard_due,
        hard_due_lead_days,
        cost_amount_cents,
        cost_period,
        cost_flag,
        cost_since,
        required_contexts,
        project_slug,
        estimate_minutes,
        waiting_on,
        snooze_count,
        parked,
        today_pin,
        triaged_at,
        done_at,
        dropped_at,
        recurrence,
        provenance,
        source_timestamps,
        task: Value::Object(task.clone()),
    })
}

fn parse_cost(
    task: &Map<String, Value>,
) -> ApiResult<(Option<i64>, Option<String>, bool, Option<NaiveDate>)> {
    let Some(cell) = cell(task, "cost_of_delay")? else {
        return Ok((None, None, false, None));
    };
    if cell.value.is_null() {
        return Ok((None, None, false, None));
    }
    let cost = cell
        .value
        .as_object()
        .ok_or_else(|| ApiError::invalid("cost_of_delay must be an object"))?;
    let since = cost
        .get("since")
        .and_then(Value::as_str)
        .map(parse_date_required)
        .transpose()?
        .ok_or_else(|| ApiError::invalid("cost_of_delay requires since"))?;
    if cost.get("flag").and_then(Value::as_bool) == Some(true) {
        return Ok((None, None, true, Some(since)));
    }
    let amount = cost
        .get("amount_cents")
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)
        .ok_or_else(|| ApiError::invalid("numeric cost_of_delay requires amount_cents"))?;
    let period = cost
        .get("per")
        .and_then(Value::as_str)
        .filter(|value| ["day", "week", "month"].contains(value))
        .ok_or_else(|| ApiError::invalid("cost_of_delay per must be day, week, or month"))?;
    Ok((Some(amount), Some(period.to_owned()), false, Some(since)))
}

fn cell<'a>(task: &'a Map<String, Value>, field: &str) -> ApiResult<Option<Cell<'a>>> {
    let Some(raw) = task.get(field) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(Some(Cell {
            value: raw,
            source: None,
            set_at: None,
        }));
    }
    let Some(object) = raw.as_object() else {
        return Err(ApiError::invalid(format!(
            "task {field} must be a sourced cell with value, source, and set_at"
        )));
    };
    if !object.contains_key("value") {
        return Err(ApiError::invalid(format!(
            "task {field} must be a sourced cell with value, source, and set_at"
        )));
    }
    let source = object
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid(format!("task {field} cell requires source")))?;
    validate_source(source)?;
    let set_at = object
        .get("set_at")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid(format!("task {field} cell requires set_at")))?;
    parse_timestamp(set_at)
        .ok_or_else(|| ApiError::invalid(format!("task {field} set_at must be RFC3339")))?;
    if let Some(note) = object.get("note").filter(|value| !value.is_null()) {
        let note = note.as_str().ok_or_else(|| {
            ApiError::invalid(format!("task {field} cell note must be a string or null"))
        })?;
        if note.len() > 1000 || has_forbidden_control(note, true) {
            return Err(ApiError::invalid(format!(
                "task {field} cell note must be printable and at most 1000 characters"
            )));
        }
    }
    Ok(Some(Cell {
        value: object
            .get("value")
            .expect("checked task provenance cell value"),
        source: Some(source),
        set_at: Some(set_at),
    }))
}

fn validate_source(source: &str) -> ApiResult<()> {
    if ["owner", "todoist", "derived"].contains(&source)
        || source.strip_prefix("agent:").is_some_and(|agent| {
            !agent.is_empty()
                && agent.len() <= 200
                && agent.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "._:@/-".contains(character)
                })
        })
    {
        Ok(())
    } else {
        Err(ApiError::invalid(
            "task field source must be owner, todoist, derived, or agent:<id>",
        ))
    }
}

fn has_forbidden_control(value: &str, allow_markdown_whitespace: bool) -> bool {
    value.chars().any(|character| {
        character.is_control()
            && !(allow_markdown_whitespace && matches!(character, '\n' | '\r' | '\t'))
    })
}

fn json_has_forbidden_control(value: &Value, allow_markdown_whitespace: bool) -> bool {
    match value {
        Value::String(value) => has_forbidden_control(value, allow_markdown_whitespace),
        Value::Array(values) => values
            .iter()
            .any(|value| json_has_forbidden_control(value, allow_markdown_whitespace)),
        Value::Object(values) => {
            values.keys().any(|key| has_forbidden_control(key, false))
                || values
                    .values()
                    .any(|value| json_has_forbidden_control(value, allow_markdown_whitespace))
        }
        _ => false,
    }
}

fn validate_slug(slug: &str, name: &str) -> ApiResult<()> {
    let valid = !slug.is_empty()
        && slug.len() <= 100
        && slug.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        });
    if valid {
        Ok(())
    } else {
        Err(ApiError::invalid(format!(
            "task {name} slug must be lowercase kebab case"
        )))
    }
}

pub(crate) fn validate_project_slug(slug: &str) -> ApiResult<()> {
    validate_slug(slug, "project")
}

fn string_value(task: &Map<String, Value>, field: &str) -> ApiResult<Option<String>> {
    let Some(cell) = cell(task, field)? else {
        return Ok(None);
    };
    if cell.value.is_null() {
        return Ok(None);
    }
    cell.value
        .as_str()
        .map(|value| Some(value.to_owned()))
        .ok_or_else(|| ApiError::invalid(format!("task {field} must be a string or null")))
}

fn integer_value(task: &Map<String, Value>, field: &str) -> ApiResult<Option<i64>> {
    let Some(cell) = cell(task, field)? else {
        return Ok(None);
    };
    if cell.value.is_null() {
        return Ok(None);
    }
    cell.value
        .as_i64()
        .map(Some)
        .ok_or_else(|| ApiError::invalid(format!("task {field} must be an integer or null")))
}

fn boolean_value(task: &Map<String, Value>, field: &str) -> ApiResult<Option<bool>> {
    let Some(cell) = cell(task, field)? else {
        return Ok(None);
    };
    if cell.value.is_null() {
        return Ok(None);
    }
    cell.value
        .as_bool()
        .map(Some)
        .ok_or_else(|| ApiError::invalid(format!("task {field} must be a boolean or null")))
}

fn owned_value(task: &Map<String, Value>, field: &str) -> ApiResult<Option<Value>> {
    let Some(cell) = cell(task, field)? else {
        return Ok(None);
    };
    Ok((!cell.value.is_null()).then(|| cell.value.clone()))
}

fn string_array_value(task: &Map<String, Value>, field: &str) -> ApiResult<Vec<String>> {
    let Some(cell) = cell(task, field)? else {
        return Ok(Vec::new());
    };
    if cell.value.is_null() {
        return Ok(Vec::new());
    }
    let values = cell
        .value
        .as_array()
        .ok_or_else(|| ApiError::invalid(format!("task {field} must be an array")))?;
    let values = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| ApiError::invalid(format!("task {field} must contain strings")))
        })
        .collect::<ApiResult<BTreeSet<_>>>()?;
    Ok(values.into_iter().collect())
}

fn timestamp_value(task: &Map<String, Value>, field: &str) -> ApiResult<Option<DateTime<Utc>>> {
    let value = string_value(task, field)?;
    value
        .as_deref()
        .map(|value| {
            parse_timestamp(value)
                .ok_or_else(|| ApiError::invalid(format!("task {field} must be RFC3339")))
        })
        .transpose()
}

fn direct_timestamp(task: &Map<String, Value>, field: &str) -> ApiResult<Option<DateTime<Utc>>> {
    let Some(value) = task.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = value
        .as_str()
        .ok_or_else(|| ApiError::invalid(format!("task {field} must be RFC3339 or null")))?;
    parse_timestamp(value)
        .map(Some)
        .ok_or_else(|| ApiError::invalid(format!("task {field} must be RFC3339 or null")))
}

fn date_value(task: &Map<String, Value>, field: &str) -> ApiResult<Option<NaiveDate>> {
    let value = string_value(task, field)?;
    value.as_deref().map(parse_date_required).transpose()
}

fn parse_date_required(value: &str) -> ApiResult<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| ApiError::invalid("task date must be YYYY-MM-DD"))
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn collect_cell_provenance(task: &Map<String, Value>) -> ApiResult<(Value, Value)> {
    let mut sources = BTreeMap::new();
    let mut timestamps = BTreeMap::new();
    for field in [
        "notes",
        "project",
        "status",
        "ready_at",
        "soft_due",
        "hard_due",
        "hard_due_lead_days",
        "cost_of_delay",
        "required_contexts",
        "estimate_minutes",
        "waiting_on",
        "snooze_count",
        "parked",
        "triaged_at",
        "today_pin",
        "recurrence",
        "completed_via",
        "dropped_reason",
    ] {
        let Some(cell) = cell(task, field)? else {
            continue;
        };
        if let Some(source) = cell.source {
            sources.insert(field.to_owned(), json!(source));
        }
        if let Some(set_at) = cell.set_at {
            timestamps.insert(field.to_owned(), json!(set_at));
        }
    }
    Ok((json!(sources), json!(timestamps)))
}

fn effective_metadata(metadata: &Value) -> &Value {
    metadata
        .get("client")
        .filter(|value| value.is_object())
        .unwrap_or(metadata)
}

fn effective_metadata_mut(metadata: &mut Value) -> &mut Value {
    if metadata.get("client").is_some_and(Value::is_object) {
        metadata
            .get_mut("client")
            .expect("checked client metadata object")
    } else {
        metadata
    }
}

pub(crate) fn normalize_slug(value: &str) -> ApiResult<String> {
    if value.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            "context name must not contain control characters",
        ));
    }
    let mut normalized = String::new();
    let mut separator_pending = false;
    for character in value.nfkd().flat_map(char::to_lowercase) {
        if is_combining_mark(character) {
            continue;
        } else if character.is_ascii_alphanumeric() {
            if separator_pending && !normalized.is_empty() {
                normalized.push('-');
            }
            separator_pending = false;
            normalized.push(character);
        } else if !normalized.is_empty() {
            separator_pending = true;
        }
    }
    if normalized.is_empty() || normalized.len() > 80 {
        return Err(ApiError::invalid(
            "context name must normalize to 1 to 80 lowercase kebab characters",
        ));
    }
    Ok(normalized)
}

fn shared_token(left: &str, right: &str) -> bool {
    let left = left
        .split('-')
        .filter(|token| token.len() >= 3)
        .collect::<BTreeSet<_>>();
    right
        .split('-')
        .filter(|token| token.len() >= 3)
        .any(|token| left.contains(token))
}

fn suggestion_priority(reason: &str) -> u8 {
    match reason {
        "exact_or_alias" => 0,
        "shared_token" => 1,
        "small_edit" => 2,
        _ => 3,
    }
}

fn damerau_levenshtein(left: &str, right: &str) -> usize {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    let mut distances = vec![vec![0_usize; right.len() + 1]; left.len() + 1];
    for (index, row) in distances.iter_mut().enumerate() {
        row[0] = index;
    }
    for index in 0..=right.len() {
        distances[0][index] = index;
    }
    for left_index in 1..=left.len() {
        for right_index in 1..=right.len() {
            let substitution = usize::from(left[left_index - 1] != right[right_index - 1]);
            distances[left_index][right_index] = (distances[left_index - 1][right_index] + 1)
                .min(distances[left_index][right_index - 1] + 1)
                .min(distances[left_index - 1][right_index - 1] + substitution);
            if left_index > 1
                && right_index > 1
                && left[left_index - 1] == right[right_index - 2]
                && left[left_index - 2] == right[right_index - 1]
            {
                distances[left_index][right_index] = distances[left_index][right_index]
                    .min(distances[left_index - 2][right_index - 2] + 1);
            }
        }
    }
    distances[left.len()][right.len()]
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourcedInput {
    pub value: Value,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_ref: Option<String>,
    pub raw_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<SourcedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<SourcedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<SourcedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_due: Option<SourcedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hard_due: Option<SourcedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hard_due_lead_days: Option<SourcedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_of_delay: Option<SourcedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_contexts: Option<SourcedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_minutes: Option<SourcedInput>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureRequest {
    pub idempotency_key: String,
    pub items: Vec<CaptureItem>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum UpdateOperation {
    Correct {
        field: String,
        value: Value,
        source: String,
        #[serde(default)]
        note: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    Complete {
        source: String,
        completed_via: String,
    },
    Reopen {
        source: String,
    },
    Snooze {
        source: String,
        #[serde(default)]
        until: Option<DateTime<Utc>>,
        #[serde(default)]
        days: Option<u32>,
    },
    Drop {
        source: String,
        #[serde(default)]
        reason: Option<String>,
    },
    WaitOn {
        source: String,
        who_or_what: String,
        #[serde(default)]
        check_back_at: Option<DateTime<Utc>>,
    },
    Unpark {
        source: String,
    },
    PinToday {
        source: String,
    },
    Unpin {
        source: String,
    },
    ConfirmHard {
        source: String,
    },
    DowngradeToSoft {
        source: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateTaskRequest {
    pub expected_version: i64,
    pub idempotency_key: String,
    pub operation: UpdateOperation,
}

enum ReceiptStart {
    New,
    Replay(Value),
}

fn envelope(status: ResponseStatus, data: Value) -> Json<WorkspaceEnvelope<Value>> {
    let mut response = WorkspaceEnvelope::complete(data);
    response.status = status;
    Json(response)
}

fn validate_idempotency_key(key: &str) -> ApiResult<()> {
    if key.is_empty() || key.len() > 240 || key.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            "idempotency_key must contain 1 to 240 non-control characters",
        ));
    }
    Ok(())
}

fn request_hash<T: Serialize>(request: &T) -> ApiResult<String> {
    let canonical = canonical_json(&serde_json::to_value(request)?)?;
    Ok(hex::encode(Sha256::digest(canonical.as_bytes())))
}

async fn begin_receipt<T: Serialize>(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    operation_kind: &str,
    key: &str,
    request: &T,
) -> ApiResult<ReceiptStart> {
    validate_idempotency_key(key)?;
    let hash = request_hash(request)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "task-operation:{}:{operation_kind}:{key}",
            auth.user_id.0
        ))
        .execute(&mut **tx)
        .await?;
    if let Some(row) = sqlx::query(
        r#"
        SELECT request_hash,status,receipt
        FROM straylight.task_operation_receipts
        WHERE user_id=$1 AND operation_kind=$2 AND idempotency_key=$3
        "#,
    )
    .bind(auth.user_id.0)
    .bind(operation_kind)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await?
    {
        let previous_hash: String = row.get("request_hash");
        if previous_hash != hash {
            return Err(ApiError::conflict(
                "idempotency_conflict",
                "the idempotency key was already used with different input",
                json!({"operation": operation_kind}),
            ));
        }
        let status: String = row.get("status");
        if status != "committed" {
            return Err(ApiError::conflict(
                "idempotency_in_progress",
                "the operation receipt has not been finalized",
                json!({"operation": operation_kind}),
            ));
        }
        let mut receipt: Value = row.get("receipt");
        if let Some(object) = receipt.as_object_mut() {
            object.insert("replayed".to_owned(), Value::Bool(true));
        }
        return Ok(ReceiptStart::Replay(receipt));
    }
    sqlx::query(
        r#"
        INSERT INTO straylight.task_operation_receipts (
          user_id,operation_kind,idempotency_key,request_hash,
          created_by_credential_id
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(auth.user_id.0)
    .bind(operation_kind)
    .bind(key)
    .bind(hash)
    .bind(auth.credential_id.0)
    .execute(&mut **tx)
    .await?;
    Ok(ReceiptStart::New)
}

async fn finalize_receipt(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    operation_kind: &str,
    key: &str,
    task_id: Option<Uuid>,
    receipt: &Value,
) -> ApiResult<()> {
    let updated = sqlx::query(
        r#"
        UPDATE straylight.task_operation_receipts
        SET status='committed',task_id=$4,receipt=$5,committed_at=clock_timestamp()
        WHERE user_id=$1 AND operation_kind=$2 AND idempotency_key=$3
          AND status='pending' AND created_by_credential_id=$6
        "#,
    )
    .bind(auth.user_id.0)
    .bind(operation_kind)
    .bind(key)
    .bind(task_id)
    .bind(receipt)
    .bind(auth.credential_id.0)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(ApiError::Internal(
            "task operation receipt could not be finalized".to_owned(),
        ));
    }
    Ok(())
}

fn actor_source(auth: &AuthContext) -> String {
    if may_assert_owner(auth) {
        "owner".to_owned()
    } else {
        format!("agent:{}", auth.credential_id.0)
    }
}

fn may_assert_owner(auth: &AuthContext) -> bool {
    auth.can(Capability::CredentialManage)
        || auth.can(Capability::Admin)
        || is_owner_device_credential(auth)
}

fn is_owner_device_credential(auth: &AuthContext) -> bool {
    auth.capabilities.len() == 2
        && auth.can(Capability::TaskWrite)
        && auth.can(Capability::NotificationManage)
}

fn may_preserve_agent_identity(auth: &AuthContext) -> bool {
    auth.can(Capability::CredentialManage) || auth.can(Capability::Admin)
}

fn validate_public_source(auth: &AuthContext, source: &str) -> ApiResult<()> {
    canonical_public_source(auth, source).map(|_| ())
}

fn canonical_public_source(auth: &AuthContext, source: &str) -> ApiResult<String> {
    validate_source(source)?;
    match source {
        "owner" if may_assert_owner(auth) => Ok("owner".to_owned()),
        "owner" => Err(ApiError::public(
            axum::http::StatusCode::FORBIDDEN,
            "owner_source_denied",
            "this credential cannot assert owner provenance",
        )),
        value if value.starts_with("agent:") && may_preserve_agent_identity(auth) => {
            Ok(value.to_owned())
        }
        value if value.starts_with("agent:") => Ok(format!("agent:{}", auth.credential_id.0)),
        _ => Err(ApiError::invalid(
            "public task mutations accept only owner or agent:<id> source",
        )),
    }
}

fn validate_capture_item(auth: &AuthContext, item: &CaptureItem) -> ApiResult<()> {
    let raw = item.raw_text.trim();
    if raw.is_empty() || raw.len() > 20_000 || has_forbidden_control(raw, true) {
        return Err(ApiError::invalid(
            "capture raw_text must contain 1 to 20000 printable characters",
        ));
    }
    if item.title.as_deref().is_some_and(|title| {
        title.trim().is_empty() || title.trim().len() > 500 || has_forbidden_control(title, false)
    }) {
        return Err(ApiError::invalid(
            "capture title must contain 1 to 500 printable characters",
        ));
    }
    if item.client_ref.as_deref().is_some_and(|value| {
        value.is_empty() || value.len() > 240 || has_forbidden_control(value, false)
    }) {
        return Err(ApiError::invalid(
            "client_ref must contain 1 to 240 printable characters",
        ));
    }
    if item.captured_from.as_deref().is_some_and(|value| {
        value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control)
    }) {
        return Err(ApiError::invalid(
            "captured_from must be a printable reference of at most 4096 characters",
        ));
    }
    for cell in [
        item.notes.as_ref(),
        item.project.as_ref(),
        item.ready_at.as_ref(),
        item.soft_due.as_ref(),
        item.hard_due.as_ref(),
        item.hard_due_lead_days.as_ref(),
        item.cost_of_delay.as_ref(),
        item.required_contexts.as_ref(),
        item.estimate_minutes.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_public_source(auth, &cell.source)?;
        if cell
            .note
            .as_deref()
            .is_some_and(|note| note.len() > 1000 || has_forbidden_control(note, true))
        {
            return Err(ApiError::invalid(
                "task source notes must be printable and at most 1000 characters",
            ));
        }
    }
    for (field, cell) in [
        ("notes", item.notes.as_ref()),
        ("project", item.project.as_ref()),
        ("ready_at", item.ready_at.as_ref()),
        ("soft_due", item.soft_due.as_ref()),
        ("hard_due", item.hard_due.as_ref()),
        ("hard_due_lead_days", item.hard_due_lead_days.as_ref()),
        ("cost_of_delay", item.cost_of_delay.as_ref()),
        ("required_contexts", item.required_contexts.as_ref()),
        ("estimate_minutes", item.estimate_minutes.as_ref()),
    ]
    .into_iter()
    .filter_map(|(field, cell)| cell.map(|cell| (field, cell)))
    {
        validate_correction_value(field, &cell.value)?;
        if field == "project" && cell.value.is_null() {
            return Err(ApiError::invalid("capture project cannot be null"));
        }
    }
    if item
        .notes
        .as_ref()
        .is_some_and(|cell| !cell.value.is_null() && !cell.value.is_string())
    {
        return Err(ApiError::invalid("capture notes must be a string or null"));
    }
    Ok(())
}

fn sourced_cell(auth: &AuthContext, input: &SourcedInput, now: DateTime<Utc>) -> ApiResult<Value> {
    let mut cell = json!({
        "value": input.value,
        "source": canonical_public_source(auth, &input.source)?,
        "set_at": now,
    });
    if let Some(note) = &input.note {
        cell["note"] = json!(note);
    }
    Ok(cell)
}

fn capture_title(item: &CaptureItem) -> String {
    item.title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            item.raw_text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
        })
        .unwrap_or("Untitled task")
        .chars()
        .take(500)
        .collect()
}

async fn resolve_context_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    requested: &str,
) -> ApiResult<Option<String>> {
    let normalized = normalize_slug(requested)?;
    Ok(sqlx::query_scalar::<_, String>(
        r#"
        SELECT context_slug FROM (
          SELECT slug AS context_slug,0 AS priority
          FROM straylight.task_contexts
          WHERE user_id=$1 AND slug=$2 AND archived_at IS NULL
          UNION ALL
          SELECT aliases.context_slug,1
          FROM straylight.task_context_aliases AS aliases
          JOIN straylight.task_contexts AS context
            ON context.user_id=aliases.user_id AND context.slug=aliases.context_slug
          WHERE aliases.user_id=$1 AND lower(aliases.alias)=lower($3)
            AND context.archived_at IS NULL
        ) AS matches
        ORDER BY priority
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(normalized)
    .bind(requested.trim())
    .fetch_optional(&mut **tx)
    .await?)
}

async fn resolve_project_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    requested: &str,
) -> ApiResult<String> {
    let normalized = normalize_slug(requested)?;
    Ok(sqlx::query_scalar::<_, String>(
        r#"
        SELECT project_slug FROM (
          SELECT slug AS project_slug,0 AS priority
          FROM straylight.task_projects
          WHERE user_id=$1 AND slug=$2 AND archived_at IS NULL
          UNION ALL
          SELECT aliases.project_slug,1
          FROM straylight.task_project_aliases AS aliases
          JOIN straylight.task_projects AS project
            ON project.user_id=aliases.user_id AND project.slug=aliases.project_slug
          WHERE aliases.user_id=$1 AND lower(aliases.alias)=lower($3)
            AND project.archived_at IS NULL
        ) AS matches
        ORDER BY priority
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(&normalized)
    .bind(requested.trim())
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(normalized))
}

pub(crate) async fn capture_tasks(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CaptureRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskWrite)?;
    if request.items.is_empty() || request.items.len() > 25 {
        return Err(ApiError::invalid("task capture accepts 1 to 25 items"));
    }
    for item in &request.items {
        validate_capture_item(&auth, item)?;
    }
    let mut tx = state.begin_write(&auth).await?;
    match begin_receipt(
        &mut tx,
        &auth,
        "task.capture",
        &request.idempotency_key,
        &request,
    )
    .await?
    {
        ReceiptStart::Replay(receipt) => {
            tx.commit().await?;
            return Ok(envelope(ResponseStatus::NoOp, receipt));
        }
        ReceiptStart::New => {}
    }

    let now = Utc::now();
    let mut prepared_contexts = Vec::<Vec<String>>::new();
    let mut unknown_contexts = BTreeMap::<String, String>::new();
    let mut review = Vec::new();
    for item in &request.items {
        let mut resolved = Vec::new();
        if let Some(cell) = &item.required_contexts {
            let values = cell.value.as_array().ok_or_else(|| {
                ApiError::invalid("required_contexts must be an array of context names")
            })?;
            for value in values {
                let requested = value.as_str().ok_or_else(|| {
                    ApiError::invalid("required_contexts must contain only strings")
                })?;
                if let Some(canonical) =
                    resolve_context_in_tx(&mut tx, auth.user_id.0, requested).await?
                {
                    resolved.push(canonical);
                    continue;
                }
                let normalized = normalize_slug(requested)?;
                let suggestions =
                    context_suggestions_in_tx(&mut tx, auth.user_id.0, requested).await?;
                if !suggestions.is_empty() {
                    review.push(json!({
                        "client_ref": item.client_ref,
                        "requested": requested,
                        "suggested_existing": suggestions.iter().map(|suggestion| json!({
                            "slug": suggestion.slug,
                            "reason": suggestion.reason,
                        })).collect::<Vec<_>>(),
                    }));
                } else {
                    unknown_contexts
                        .entry(normalized.clone())
                        .or_insert(canonical_public_source(&auth, &cell.source)?);
                    resolved.push(normalized);
                }
            }
        }
        resolved.sort();
        resolved.dedup();
        prepared_contexts.push(resolved);
    }
    if !review.is_empty() {
        tx.rollback().await?;
        return Ok(envelope(
            ResponseStatus::NeedsReview,
            json!({
                "items": [],
                "suggested_existing": review,
                "replayed": false,
            }),
        ));
    }
    for (slug, source) in &unknown_contexts {
        create_context_in_tx(
            &mut tx,
            auth.user_id.0,
            auth.credential_id.0,
            slug,
            None,
            None,
            source,
            true,
        )
        .await?;
    }

    let mut response_items = Vec::with_capacity(request.items.len());
    for (item, contexts) in request.items.iter().zip(prepared_contexts) {
        let task_id = Uuid::now_v7();
        let title = capture_title(item);
        let actor = [
            item.notes.as_ref(),
            item.project.as_ref(),
            item.ready_at.as_ref(),
            item.soft_due.as_ref(),
            item.hard_due.as_ref(),
            item.hard_due_lead_days.as_ref(),
            item.cost_of_delay.as_ref(),
            item.required_contexts.as_ref(),
            item.estimate_minutes.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(|cell| cell.source.as_str())
        .find(|source| source.starts_with("agent:"))
        .map(|source| canonical_public_source(&auth, source))
        .transpose()?
        .unwrap_or_else(|| actor_source(&auth));
        let mut task = Map::new();
        task.insert("id".to_owned(), json!(task_id));
        task.insert("title".to_owned(), json!(title));
        task.insert(
            "provenance".to_owned(),
            json!({
                "created_at": now,
                "created_by": actor,
                "raw_text": item.raw_text,
                "captured_from": item.captured_from,
                "credential_id": auth.credential_id.0,
                "title_source": actor,
                "title_set_at": now,
            }),
        );
        task.insert(
            "status".to_owned(),
            json!({"value":"open","source":"derived","set_at":now}),
        );
        for (name, cell) in [
            ("notes", item.notes.as_ref()),
            ("ready_at", item.ready_at.as_ref()),
            ("soft_due", item.soft_due.as_ref()),
            ("hard_due", item.hard_due.as_ref()),
            ("hard_due_lead_days", item.hard_due_lead_days.as_ref()),
            ("cost_of_delay", item.cost_of_delay.as_ref()),
            ("estimate_minutes", item.estimate_minutes.as_ref()),
        ] {
            if let Some(cell) = cell {
                task.insert(name.to_owned(), sourced_cell(&auth, cell, now)?);
            }
        }
        if let Some(cell) = &item.project {
            let requested = cell
                .value
                .as_str()
                .ok_or_else(|| ApiError::invalid("capture project must be a string or null"))?;
            let project = resolve_project_in_tx(&mut tx, auth.user_id.0, requested).await?;
            let mut project_cell = cell.clone();
            project_cell.value = json!(project);
            task.insert(
                "project".to_owned(),
                sourced_cell(&auth, &project_cell, now)?,
            );
        }
        if let Some(cell) = &item.required_contexts {
            let mut contexts_cell = cell.clone();
            contexts_cell.value = json!(contexts);
            task.insert(
                "required_contexts".to_owned(),
                sourced_cell(&auth, &contexts_cell, now)?,
            );
        }
        let metadata = json!({
            "kind": "task",
            "schema": TASK_SCHEMA,
            "task": task,
        });
        let notes = item
            .notes
            .as_ref()
            .and_then(|cell| cell.value.as_str())
            .unwrap_or(item.raw_text.trim());
        let content = if notes.is_empty() {
            format!("# {title}\n")
        } else {
            format!("# {title}\n\n{notes}\n")
        };
        let path = format!("{TASK_ENTRY_PREFIX}{task_id}.md");
        let prepared = simple_core::prepare_task_markdown_for_update(path, content, metadata, 0)?;
        let result = simple_core::upsert_markdown_in_tx(
            &mut tx,
            auth.user_id.0,
            Some(auth.credential_id.0),
            prepared,
        )
        .await?;
        let projection = sqlx::query(
            "SELECT title,task FROM straylight.task_index WHERE user_id=$1 AND task_id=$2",
        )
        .bind(auth.user_id.0)
        .bind(task_id)
        .fetch_one(&mut *tx)
        .await?;
        let projected_task: Value = projection.get("task");
        response_items.push(json!({
            "client_ref": item.client_ref,
            "task_ref": task_id,
            "entry_ref": format!("entry:{}", result.entry_id),
            "version": result.version,
            "title": projection.get::<String,_>("title"),
            "enrichment": projected_task,
            "context_suggestions": [],
            "suggested_existing": [],
        }));
    }
    let receipt = json!({"items": response_items, "replayed": false});
    finalize_receipt(
        &mut tx,
        &auth,
        "task.capture",
        &request.idempotency_key,
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    state.workspace_features.invalidate(auth.user_id.0).await;
    Ok(envelope(ResponseStatus::Committed, receipt))
}

fn parse_task_ref(raw: &str) -> ApiResult<Uuid> {
    if raw.starts_with("task:") {
        return Err(ApiError::invalid("task_ref must be a raw UUID"));
    }
    let id =
        Uuid::parse_str(raw).map_err(|_| ApiError::invalid("task_ref must be a raw UUIDv7"))?;
    if raw != id.to_string() || id.get_version_num() != 7 {
        return Err(ApiError::invalid(
            "task_ref must be a lowercase hyphenated raw UUIDv7",
        ));
    }
    Ok(id)
}

fn task_detail_from_row(row: &sqlx::postgres::PgRow) -> Value {
    let task_id: Uuid = row.get("task_id");
    let entry_id: Uuid = row.get("entry_id");
    json!({
        "task_ref": task_id,
        "entry_ref": format!("entry:{entry_id}"),
        "version": row.get::<i64,_>("entry_version"),
        "title": row.get::<String,_>("title"),
        "status": row.get::<String,_>("status"),
        "task": row.get::<Value,_>("task"),
        "provenance": row.get::<Value,_>("provenance"),
        "source_timestamps": row.get::<Value,_>("source_timestamps"),
        "created_at": row.get::<DateTime<Utc>,_>("created_at"),
        "updated_at": row.get::<DateTime<Utc>,_>("updated_at"),
    })
}

async fn fetch_task_row(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    task_id: Uuid,
    lock: bool,
) -> ApiResult<Option<sqlx::postgres::PgRow>> {
    const QUERY: &str = r#"
        SELECT task.task_id,task.entry_id,task.entry_version,task.title,task.status,
               task.task,task.provenance,task.source_timestamps,task.created_at,
               task.updated_at,entry.path,entry.current_version,version.content,
               version.metadata
        FROM straylight.task_index AS task
        JOIN straylight.entries AS entry
          ON entry.user_id=task.user_id AND entry.id=task.entry_id
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE task.user_id=$1 AND task.task_id=$2
        "#;
    const LOCKED_QUERY: &str = r#"
        SELECT task.task_id,task.entry_id,task.entry_version,task.title,task.status,
               task.task,task.provenance,task.source_timestamps,task.created_at,
               task.updated_at,entry.path,entry.current_version,version.content,
               version.metadata
        FROM straylight.task_index AS task
        JOIN straylight.entries AS entry
          ON entry.user_id=task.user_id AND entry.id=task.entry_id
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE task.user_id=$1 AND task.task_id=$2
        FOR UPDATE OF entry
        "#;
    let row = if lock {
        sqlx::query(LOCKED_QUERY)
            .bind(user_id)
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?
    } else {
        sqlx::query(QUERY)
            .bind(user_id)
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?
    };
    Ok(row)
}

pub(crate) async fn get_task(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(task_ref): Path<String>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskRead)?;
    let task_id = parse_task_ref(&task_ref)?;
    let mut tx = state.begin_read(&auth).await?;
    let row = fetch_task_row(&mut tx, auth.user_id.0, task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task_not_found", &task_ref))?;
    let task = task_detail_from_row(&row);
    tx.commit().await?;
    Ok(envelope(ResponseStatus::Complete, json!({"task": task})))
}

fn update_operation_source(operation: &UpdateOperation) -> &str {
    match operation {
        UpdateOperation::Correct { source, .. }
        | UpdateOperation::Complete { source, .. }
        | UpdateOperation::Reopen { source }
        | UpdateOperation::Snooze { source, .. }
        | UpdateOperation::Drop { source, .. }
        | UpdateOperation::WaitOn { source, .. }
        | UpdateOperation::Unpark { source }
        | UpdateOperation::PinToday { source }
        | UpdateOperation::Unpin { source }
        | UpdateOperation::ConfirmHard { source }
        | UpdateOperation::DowngradeToSoft { source } => source,
    }
}

fn canonical_completed_via(auth: &AuthContext, completed_via: &str) -> ApiResult<String> {
    match completed_via {
        "ios" if is_owner_device_credential(auth) => Ok("ios".to_owned()),
        "web" if may_preserve_agent_identity(auth) => Ok("web".to_owned()),
        "ios" | "web" => Err(ApiError::public(
            axum::http::StatusCode::FORBIDDEN,
            "completion_channel_denied",
            "this credential cannot assert the requested completion channel",
        )),
        value if value.starts_with("agent:") => canonical_public_source(auth, value),
        _ => Err(ApiError::invalid(
            "completed_via must be ios, web, or agent:<id>",
        )),
    }
}

fn validate_update_operation(auth: &AuthContext, operation: &UpdateOperation) -> ApiResult<()> {
    validate_public_source(auth, update_operation_source(operation))?;
    match operation {
        UpdateOperation::Correct {
            note,
            reason,
            field,
            ..
        } => {
            if note
                .as_deref()
                .is_some_and(|value| value.len() > 1000 || has_forbidden_control(value, true))
                || reason
                    .as_deref()
                    .is_some_and(|value| value.len() > 1000 || has_forbidden_control(value, true))
            {
                return Err(ApiError::invalid(
                    "task correction notes and reasons must be printable and at most 1000 characters",
                ));
            }
            if field.is_empty() || field.len() > 80 || has_forbidden_control(field, false) {
                return Err(ApiError::invalid("task correction field is invalid"));
            }
            validate_correction_value(field, operation_correct_value(operation)?)?;
        }
        UpdateOperation::Complete { completed_via, .. } => {
            canonical_completed_via(auth, completed_via)?;
        }
        UpdateOperation::Snooze { until, days, .. } => {
            if until.is_some() == days.is_some() {
                return Err(ApiError::invalid(
                    "snooze requires exactly one of until or days",
                ));
            }
            if days.is_some_and(|value| value == 0 || value > 3650) {
                return Err(ApiError::invalid("snooze days must be between 1 and 3650"));
            }
        }
        UpdateOperation::Drop { reason, .. } => {
            if reason
                .as_deref()
                .is_some_and(|value| value.len() > 1000 || has_forbidden_control(value, true))
            {
                return Err(ApiError::invalid(
                    "drop reason must be printable and at most 1000 characters",
                ));
            }
        }
        UpdateOperation::WaitOn { who_or_what, .. } => {
            if who_or_what.trim().is_empty()
                || who_or_what.len() > 1000
                || has_forbidden_control(who_or_what, true)
            {
                return Err(ApiError::invalid(
                    "wait_on who_or_what must contain 1 to 1000 printable characters",
                ));
            }
        }
        UpdateOperation::ConfirmHard { source } if source != "owner" => {
            return Err(ApiError::invalid(
                "confirm_hard requires owner source; agents may instead issue a sourced correction",
            ));
        }
        _ => {}
    }
    Ok(())
}

fn validate_action_state(operation: &UpdateOperation, status: &str) -> ApiResult<()> {
    let terminal = matches!(status, "done" | "dropped");
    let allowed = match operation {
        UpdateOperation::Correct { .. } => true,
        UpdateOperation::Reopen { .. } => status != "open",
        UpdateOperation::Complete { .. } | UpdateOperation::Drop { .. } => !terminal,
        UpdateOperation::Snooze { .. }
        | UpdateOperation::WaitOn { .. }
        | UpdateOperation::Unpark { .. }
        | UpdateOperation::PinToday { .. }
        | UpdateOperation::Unpin { .. }
        | UpdateOperation::ConfirmHard { .. }
        | UpdateOperation::DowngradeToSoft { .. } => !terminal,
    };
    if allowed {
        Ok(())
    } else {
        Err(ApiError::conflict(
            "task_state_conflict",
            "the action is not valid for the current task state",
            json!({"status":status}),
        ))
    }
}

fn operation_correct_value(operation: &UpdateOperation) -> ApiResult<&Value> {
    match operation {
        UpdateOperation::Correct { value, .. } => Ok(value),
        _ => Err(ApiError::Internal(
            "correction validation called for an action".to_owned(),
        )),
    }
}

fn validate_correction_value(field: &str, value: &Value) -> ApiResult<()> {
    const FIELDS: &[&str] = &[
        "title",
        "notes",
        "project",
        "ready_at",
        "soft_due",
        "hard_due",
        "hard_due_lead_days",
        "cost_of_delay",
        "required_contexts",
        "estimate_minutes",
        "recurrence",
    ];
    if !FIELDS.contains(&field) {
        return Err(ApiError::invalid(format!(
            "{field} is not a publicly correctable task field"
        )));
    }
    match field {
        "title" => {
            if value.as_str().map(str::trim).is_none_or(|value| {
                value.is_empty() || value.len() > 500 || has_forbidden_control(value, false)
            }) {
                return Err(ApiError::invalid("title must contain 1 to 500 characters"));
            }
        }
        "notes" => {
            if !value.is_null()
                && value
                    .as_str()
                    .is_none_or(|value| value.len() > 20_000 || has_forbidden_control(value, true))
            {
                return Err(ApiError::invalid(
                    "notes must be null or a string of at most 20000 characters",
                ));
            }
        }
        "project" => {
            if !value.is_null()
                && value.as_str().is_none_or(|value| {
                    value.trim().is_empty()
                        || value.len() > 160
                        || has_forbidden_control(value, false)
                })
            {
                return Err(ApiError::invalid(
                    "project must be null or a nonempty project name",
                ));
            }
        }
        "ready_at" | "hard_due" => {
            if !value.is_null() && value.as_str().and_then(parse_timestamp).is_none() {
                return Err(ApiError::invalid(format!(
                    "{field} must be null or RFC3339"
                )));
            }
        }
        "soft_due" => {
            if !value.is_null()
                && value
                    .as_str()
                    .map(parse_date_required)
                    .transpose()?
                    .is_none()
            {
                return Err(ApiError::invalid("soft_due must be null or YYYY-MM-DD"));
            }
        }
        "hard_due_lead_days" => {
            if !value.is_null()
                && value
                    .as_i64()
                    .is_none_or(|value| !(0..=3650).contains(&value))
            {
                return Err(ApiError::invalid(
                    "hard_due_lead_days must be null or 0..3650",
                ));
            }
        }
        "estimate_minutes" => {
            if !value.is_null()
                && value
                    .as_i64()
                    .is_none_or(|value| !(1..=10080).contains(&value))
            {
                return Err(ApiError::invalid(
                    "estimate_minutes must be null or 1..10080",
                ));
            }
        }
        "required_contexts" => {
            let contexts = value
                .as_array()
                .ok_or_else(|| ApiError::invalid("required_contexts must be an array"))?;
            if contexts.len() > 20 {
                return Err(ApiError::invalid(
                    "required_contexts accepts at most 20 values",
                ));
            }
            for context in contexts {
                normalize_slug(
                    context.as_str().ok_or_else(|| {
                        ApiError::invalid("required_contexts must contain strings")
                    })?,
                )?;
            }
        }
        "cost_of_delay" => {
            if value.is_null() {
                return Ok(());
            }
            let cost = value
                .as_object()
                .ok_or_else(|| ApiError::invalid("cost_of_delay must be null or an object"))?;
            let since = cost
                .get("since")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::invalid("cost_of_delay requires since"))?;
            parse_date_required(since)?;
            let flag = cost.get("flag").and_then(Value::as_bool) == Some(true);
            if flag {
                if cost
                    .keys()
                    .any(|key| !matches!(key.as_str(), "flag" | "since" | "note"))
                {
                    return Err(ApiError::invalid("flag cost_of_delay has unknown fields"));
                }
            } else {
                if cost
                    .get("amount_cents")
                    .and_then(Value::as_i64)
                    .is_none_or(|value| value < 0)
                    || !matches!(
                        cost.get("per").and_then(Value::as_str),
                        Some("day" | "week" | "month")
                    )
                    || cost.keys().any(|key| {
                        !matches!(key.as_str(), "amount_cents" | "per" | "since" | "note")
                    })
                {
                    return Err(ApiError::invalid(
                        "numeric cost_of_delay requires nonnegative amount_cents, day/week/month per, and since",
                    ));
                }
            }
            if cost
                .get("note")
                .is_some_and(|value| !value.is_null() && !value.is_string())
            {
                return Err(ApiError::invalid(
                    "cost_of_delay note must be a string or null",
                ));
            }
            if cost
                .get("note")
                .and_then(Value::as_str)
                .is_some_and(|note| note.len() > 1000 || has_forbidden_control(note, true))
            {
                return Err(ApiError::invalid(
                    "cost_of_delay note must be printable and at most 1000 characters",
                ));
            }
        }
        "recurrence" => {
            if !value.is_null() && (!value.is_object() || json_has_forbidden_control(value, false))
            {
                return Err(ApiError::invalid("recurrence must be null or an object"));
            }
        }
        _ => unreachable!("checked correction field"),
    }
    Ok(())
}

fn push_sourced_change(
    metadata: &mut Value,
    corrections: &mut Vec<CorrectionDelta>,
    field: &str,
    value: Value,
    source: &str,
    now: DateTime<Utc>,
    note: Option<&str>,
) -> ApiResult<()> {
    if let Some(correction) = apply_sourced_field(metadata, field, value, source, now, note, true)?
    {
        corrections.push(correction);
    }
    Ok(())
}

fn direct_task_object_mut(metadata: &mut Value) -> ApiResult<&mut Map<String, Value>> {
    effective_metadata_mut(metadata)
        .get_mut("task")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ApiError::invalid("task.v1 metadata requires a task object"))
}

async fn insert_corrections_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    task_id: Uuid,
    entry_id: Uuid,
    entry_version: i64,
    corrections: &[CorrectionDelta],
    reason: &str,
) -> ApiResult<Option<Uuid>> {
    let mut first = None;
    for correction in corrections {
        let id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO straylight.task_corrections (
              id,user_id,task_id,entry_id,entry_version,field_name,
              previous_value,previous_source,corrected_value,corrected_source,
              reason,credential_id
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            "#,
        )
        .bind(id)
        .bind(auth.user_id.0)
        .bind(task_id)
        .bind(entry_id)
        .bind(entry_version)
        .bind(&correction.field_name)
        .bind(&correction.previous_value)
        .bind(&correction.previous_source)
        .bind(&correction.corrected_value)
        .bind(&correction.corrected_source)
        .bind(reason)
        .bind(auth.credential_id.0)
        .execute(&mut **tx)
        .await?;
        first.get_or_insert(id);
    }
    Ok(first)
}

async fn owner_local_date_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> ApiResult<(NaiveDate, String)> {
    let timezone = sqlx::query_scalar::<_, String>(
        "SELECT timezone FROM straylight.task_settings WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    let timezone_parsed = timezone
        .parse::<Tz>()
        .map_err(|_| ApiError::Internal("stored task timezone is invalid".to_owned()))?;
    Ok((now.with_timezone(&timezone_parsed).date_naive(), timezone))
}

pub(crate) async fn update_task(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(task_ref): Path<String>,
    Json(request): Json<UpdateTaskRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskWrite)?;
    if request.expected_version < 1 {
        return Err(ApiError::invalid("expected_version must be positive"));
    }
    validate_update_operation(&auth, &request.operation)?;
    let task_id = parse_task_ref(&task_ref)?;
    let receipt_kind = format!(
        "task.update.{}",
        match &request.operation {
            UpdateOperation::Correct { .. } => "correct",
            UpdateOperation::Complete { .. } => "complete",
            UpdateOperation::Reopen { .. } => "reopen",
            UpdateOperation::Snooze { .. } => "snooze",
            UpdateOperation::Drop { .. } => "drop",
            UpdateOperation::WaitOn { .. } => "wait_on",
            UpdateOperation::Unpark { .. } => "unpark",
            UpdateOperation::PinToday { .. } => "pin_today",
            UpdateOperation::Unpin { .. } => "unpin",
            UpdateOperation::ConfirmHard { .. } => "confirm_hard",
            UpdateOperation::DowngradeToSoft { .. } => "downgrade_to_soft",
        }
    );
    let mut tx = state.begin_write(&auth).await?;
    let receipt_request = json!({"task_ref": task_id, "request": &request});
    match begin_receipt(
        &mut tx,
        &auth,
        &receipt_kind,
        &request.idempotency_key,
        &receipt_request,
    )
    .await?
    {
        ReceiptStart::Replay(receipt) => {
            tx.commit().await?;
            return Ok(envelope(ResponseStatus::NoOp, receipt));
        }
        ReceiptStart::New => {}
    }
    let row = fetch_task_row(&mut tx, auth.user_id.0, task_id, true)
        .await?
        .ok_or_else(|| ApiError::not_found("task_not_found", &task_ref))?;
    let current_version: i64 = row.get("current_version");
    if current_version != request.expected_version {
        return Err(ApiError::conflict(
            "task_version_conflict",
            "the task changed after the supplied expected_version",
            json!({
                "task_ref": task_id,
                "expected_version": request.expected_version,
                "current_version": current_version,
            }),
        ));
    }
    let current_status = row.get::<String, _>("status");
    validate_action_state(&request.operation, &current_status)?;
    let now = Utc::now();
    let mut metadata: Value = row.get("metadata");
    let mut content: String = row.get("content");
    let source = canonical_public_source(&auth, update_operation_source(&request.operation))?;
    let mut corrected_value_override = None;
    if let UpdateOperation::Correct { field, value, .. } = &request.operation {
        if field == "required_contexts" {
            let mut canonical = Vec::new();
            let mut review = Vec::new();
            for requested in value.as_array().expect("validated required_contexts array") {
                let requested = requested
                    .as_str()
                    .expect("validated required_contexts string");
                if let Some(context) =
                    resolve_context_in_tx(&mut tx, auth.user_id.0, requested).await?
                {
                    canonical.push(context);
                    continue;
                }
                let suggestions =
                    context_suggestions_in_tx(&mut tx, auth.user_id.0, requested).await?;
                if suggestions.is_empty() {
                    let slug = create_context_in_tx(
                        &mut tx,
                        auth.user_id.0,
                        auth.credential_id.0,
                        requested,
                        None,
                        None,
                        &source,
                        true,
                    )
                    .await?;
                    canonical.push(slug);
                } else {
                    review.push(json!({
                        "requested":requested,
                        "suggested_existing":suggestions.into_iter().map(|suggestion|json!({"slug":suggestion.slug,"reason":suggestion.reason})).collect::<Vec<_>>(),
                    }));
                }
            }
            if !review.is_empty() {
                tx.rollback().await?;
                return Ok(envelope(
                    ResponseStatus::NeedsReview,
                    json!({
                        "task_ref":task_id,
                        "field":"required_contexts",
                        "suggested_existing":review,
                        "replayed":false,
                    }),
                ));
            }
            canonical.sort();
            canonical.dedup();
            corrected_value_override = Some(json!(canonical));
        } else if field == "project" && !value.is_null() {
            corrected_value_override = Some(json!(
                resolve_project_in_tx(
                    &mut tx,
                    auth.user_id.0,
                    value.as_str().expect("validated project string"),
                )
                .await?
            ));
        }
    }
    let mut corrections = Vec::new();
    let (action, reason) = match &request.operation {
        UpdateOperation::Correct {
            field,
            value,
            note,
            reason,
            ..
        } => {
            let value = corrected_value_override.as_ref().unwrap_or(value);
            if field == "title" {
                let corrected = value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty() && value.len() <= 500)
                    .ok_or_else(|| {
                        ApiError::invalid("corrected title must contain 1 to 500 characters")
                    })?
                    .to_owned();
                let task = direct_task_object_mut(&mut metadata)?;
                let previous_value = task.get("title").cloned().unwrap_or(Value::Null);
                let previous_source = task
                    .get("provenance")
                    .and_then(|value| value.get("title_source"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                if previous_source.as_deref() == Some("owner") && source != "owner" {
                    return Err(ApiError::conflict(
                        "task_owner_value_precedence",
                        "an owner-set task title cannot be overwritten by an agent",
                        json!({"field":"title"}),
                    ));
                }
                if previous_value != json!(corrected) || previous_source.as_deref() != Some(&source)
                {
                    corrections.push(CorrectionDelta {
                        field_name: "title".to_owned(),
                        previous_value,
                        previous_source,
                        corrected_value: json!(corrected),
                        corrected_source: source.clone(),
                    });
                }
                task.insert("title".to_owned(), json!(corrected));
                let provenance = task
                    .entry("provenance")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or_else(|| ApiError::invalid("task provenance must be an object"))?;
                provenance.insert("title_source".to_owned(), json!(source));
                provenance.insert("title_set_at".to_owned(), json!(now));
                let remainder = content
                    .split_once('\n')
                    .map(|(_, remainder)| remainder)
                    .unwrap_or("");
                content = format!("# {corrected}\n{remainder}");
            } else {
                push_sourced_change(
                    &mut metadata,
                    &mut corrections,
                    field,
                    value.clone(),
                    &source,
                    now,
                    note.as_deref(),
                )?;
            }
            if field == "notes" {
                let title = direct_task_object_mut(&mut metadata)?
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled task")
                    .to_owned();
                content = match value.as_str() {
                    Some(notes) if !notes.is_empty() => format!("# {title}\n\n{notes}\n"),
                    _ => format!("# {title}\n"),
                };
            }
            (
                "correct",
                reason.as_deref().unwrap_or("explicit task correction"),
            )
        }
        UpdateOperation::Complete { completed_via, .. } => {
            let completed_via = canonical_completed_via(&auth, completed_via)?;
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "status",
                json!("done"),
                &source,
                now,
                None,
            )?;
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "completed_via",
                json!(completed_via),
                &source,
                now,
                None,
            )?;
            direct_task_object_mut(&mut metadata)?.insert("done_at".to_owned(), json!(now));
            ("complete", "explicit completion")
        }
        UpdateOperation::Reopen { .. } => {
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "status",
                json!("open"),
                &source,
                now,
                None,
            )?;
            let task = direct_task_object_mut(&mut metadata)?;
            task.remove("done_at");
            task.remove("dropped_at");
            if current_status == "waiting" {
                push_sourced_change(
                    &mut metadata,
                    &mut corrections,
                    "waiting_on",
                    Value::Null,
                    &source,
                    now,
                    None,
                )?;
                push_sourced_change(
                    &mut metadata,
                    &mut corrections,
                    "ready_at",
                    Value::Null,
                    &source,
                    now,
                    None,
                )?;
            }
            ("reopen", "explicit reopen")
        }
        UpdateOperation::Snooze { until, days, .. } => {
            let ready_at = until.unwrap_or_else(|| now + Duration::days(i64::from(days.unwrap())));
            if ready_at <= now {
                return Err(ApiError::invalid("snooze target must be in the future"));
            }
            let existing = effective_metadata(&metadata)
                .get("task")
                .and_then(Value::as_object)
                .map(|task| integer_value(task, "snooze_count"))
                .transpose()?
                .flatten()
                .unwrap_or(0);
            let (count, parked) = task_engine::snooze_transition(existing.max(0) as u32);
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "ready_at",
                json!(ready_at),
                &source,
                now,
                None,
            )?;
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "snooze_count",
                json!(count),
                &source,
                now,
                None,
            )?;
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "parked",
                json!(parked),
                &source,
                now,
                None,
            )?;
            ("snooze", "explicit snooze")
        }
        UpdateOperation::Drop { reason, .. } => {
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "status",
                json!("dropped"),
                &source,
                now,
                None,
            )?;
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "dropped_reason",
                json!(reason),
                &source,
                now,
                None,
            )?;
            direct_task_object_mut(&mut metadata)?.insert("dropped_at".to_owned(), json!(now));
            ("drop", "explicit drop")
        }
        UpdateOperation::WaitOn {
            who_or_what,
            check_back_at,
            ..
        } => {
            if check_back_at.is_some_and(|check_back_at| check_back_at <= now) {
                return Err(ApiError::invalid(
                    "wait_on check_back_at must be in the future",
                ));
            }
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "status",
                json!("waiting"),
                &source,
                now,
                None,
            )?;
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "waiting_on",
                json!({"who_or_what": who_or_what.trim(), "since": now, "check_back_at": check_back_at}),
                &source,
                now,
                None,
            )?;
            if let Some(check_back_at) = check_back_at {
                push_sourced_change(
                    &mut metadata,
                    &mut corrections,
                    "ready_at",
                    json!(check_back_at),
                    &source,
                    now,
                    None,
                )?;
            }
            ("wait_on", "explicit waiting state")
        }
        UpdateOperation::Unpark { .. } => {
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "parked",
                json!(false),
                &source,
                now,
                None,
            )?;
            ("unpark", "explicit unpark")
        }
        UpdateOperation::PinToday { .. } => {
            let (today, _) = owner_local_date_in_tx(&mut tx, auth.user_id.0, now).await?;
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                .bind(format!("task-pin:{}:{today}", auth.user_id.0))
                .execute(&mut *tx)
                .await?;
            let pins = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM straylight.task_index WHERE user_id=$1 AND today_pin=$2 AND task_id<>$3",
            )
            .bind(auth.user_id.0)
            .bind(today)
            .bind(task_id)
            .fetch_one(&mut *tx)
            .await?;
            if pins >= 5 {
                return Err(ApiError::conflict(
                    "today_pin_limit",
                    "at most five tasks may be pinned for the owner-local day",
                    json!({"date": today, "limit": 5}),
                ));
            }
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "today_pin",
                json!(today),
                &source,
                now,
                None,
            )?;
            ("pin_today", "explicit today pin")
        }
        UpdateOperation::Unpin { .. } => {
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "today_pin",
                Value::Null,
                &source,
                now,
                None,
            )?;
            ("unpin", "explicit unpin")
        }
        UpdateOperation::ConfirmHard { .. } => {
            let hard_due = effective_metadata(&metadata)
                .get("task")
                .and_then(Value::as_object)
                .map(|task| owned_value(task, "hard_due"))
                .transpose()?
                .flatten()
                .filter(|value| !value.is_null())
                .ok_or_else(|| ApiError::invalid("confirm_hard requires an existing hard_due"))?;
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "hard_due",
                hard_due,
                &source,
                now,
                Some("owner confirmed hard deadline"),
            )?;
            ("confirm_hard", "owner confirmed hard deadline")
        }
        UpdateOperation::DowngradeToSoft { .. } => {
            let hard_due = effective_metadata(&metadata)
                .get("task")
                .and_then(Value::as_object)
                .map(|task| timestamp_value(task, "hard_due"))
                .transpose()?
                .flatten()
                .ok_or_else(|| {
                    ApiError::invalid("downgrade_to_soft requires an existing hard_due")
                })?;
            let (_, timezone) = owner_local_date_in_tx(&mut tx, auth.user_id.0, now).await?;
            let timezone = timezone
                .parse::<Tz>()
                .map_err(|_| ApiError::Internal("stored task timezone is invalid".to_owned()))?;
            let local_due = hard_due.with_timezone(&timezone).date_naive();
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "hard_due",
                Value::Null,
                &source,
                now,
                Some("downgraded to soft due"),
            )?;
            push_sourced_change(
                &mut metadata,
                &mut corrections,
                "soft_due",
                json!(local_due),
                &source,
                now,
                Some("downgraded from hard due"),
            )?;
            ("downgrade_to_soft", "explicit hard-to-soft downgrade")
        }
    };
    let path: String = row.get("path");
    let prepared =
        simple_core::prepare_task_markdown_for_update(path, content, metadata, current_version)?;
    let result = simple_core::upsert_markdown_in_tx(
        &mut tx,
        auth.user_id.0,
        Some(auth.credential_id.0),
        prepared,
    )
    .await?;
    let correction_ref = insert_corrections_in_tx(
        &mut tx,
        &auth,
        task_id,
        result.entry_id,
        result.version,
        &corrections,
        reason,
    )
    .await?;
    let next_occurrence = if action == "complete" {
        materialize_next_todoist_occurrence_in_tx(
            &mut tx,
            auth.user_id.0,
            auth.credential_id.0,
            task_id,
            now,
        )
        .await?
    } else {
        None
    };
    sqlx::query(
        r#"
        INSERT INTO straylight.task_audit_events (
          user_id,task_id,credential_id,action,details
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(auth.user_id.0)
    .bind(task_id)
    .bind(auth.credential_id.0)
    .bind(format!("task.{action}"))
    .bind(json!({"source": source, "entry_version": result.version}))
    .execute(&mut *tx)
    .await?;
    let updated = fetch_task_row(&mut tx, auth.user_id.0, task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task_not_found", &task_ref))?;
    let done_today_count = if action == "complete" {
        let (_, timezone) = owner_local_date_in_tx(&mut tx, auth.user_id.0, now).await?;
        Some(
            sqlx::query_scalar::<_, i64>(
                r#"
                SELECT count(*) FROM straylight.task_index
                WHERE user_id=$1 AND status='done'
                  AND (done_at AT TIME ZONE $2)::date=(clock_timestamp() AT TIME ZONE $2)::date
                "#,
            )
            .bind(auth.user_id.0)
            .bind(timezone)
            .fetch_one(&mut *tx)
            .await?,
        )
    } else {
        None
    };
    let receipt = json!({
        "task": task_detail_from_row(&updated),
        "action": action,
        "correction_ref": correction_ref,
        "done_today_count": done_today_count,
        "next_occurrence_task_ref":next_occurrence,
        "replayed": false,
    });
    finalize_receipt(
        &mut tx,
        &auth,
        &receipt_kind,
        &request.idempotency_key,
        Some(task_id),
        &receipt,
    )
    .await?;
    tx.commit().await?;
    state.workspace_features.invalidate(auth.user_id.0).await;
    Ok(envelope(ResponseStatus::Committed, receipt))
}

#[derive(Default)]
struct CandidateQuery {
    view: Option<String>,
    limit: Option<usize>,
    contexts_available: BTreeSet<String>,
    project: Option<String>,
    include_waiting: bool,
    include_parked: bool,
    as_of: Option<DateTime<Utc>>,
    cursor: Option<Uuid>,
    deliberate_all: bool,
}

/// Exact scalar projection query used by the deployed candidates handler.
/// Ranking remains exclusively in task_engine; this query omits canonical task
/// JSON so a 2,000-task read does not deserialize the full documents.
pub const TASK_CANDIDATE_PROJECTION_SQL: &str = r#"
SELECT task.task_id,task.entry_id,task.entry_version,task.title,task.status,
       task.ready_at,task.soft_due,task.hard_due,task.hard_due_lead_days,
       task.cost_amount_cents,task.cost_period,task.cost_flag,task.cost_since,
       task.required_contexts,task.project_slug,task.parked,task.today_pin,
       task.triaged_at,task.created_at,task.provenance,task.source_timestamps,
       project.interest_override,project.interest_set_at,project.last_activity_at
FROM straylight.task_index AS task
LEFT JOIN straylight.task_projects AS project
  ON project.user_id=task.user_id AND project.slug=task.project_slug
WHERE task.user_id=$1 AND task.status IN ('open','waiting')
  AND ($2::boolean OR task.status='open')
  AND ($3::boolean OR NOT task.parked)
  AND task.created_at <= $4::timestamptz
  AND (task.ready_at IS NULL OR task.ready_at <= $4::timestamptz)
  AND ($5::text IS NULL OR task.project_slug=$5::text)
  AND task.required_contexts <@ $6::text[]
ORDER BY task.created_at,task.task_id
"#;

fn parse_bool_query(name: &str, value: &str) -> ApiResult<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ApiError::invalid(format!(
            "{name} query parameter must be true or false"
        ))),
    }
}

fn parse_candidate_query(raw: Option<&str>) -> ApiResult<CandidateQuery> {
    let mut parsed = CandidateQuery::default();
    let mut seen = BTreeSet::new();
    for (name, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        if name != "contexts_available" && !seen.insert(name.to_string()) {
            return Err(ApiError::invalid(format!(
                "repeated task candidates query parameter: {name}"
            )));
        }
        match name.as_ref() {
            "view" if parsed.view.is_none() => parsed.view = Some(value.into_owned()),
            "limit" if parsed.limit.is_none() => {
                parsed.limit = Some(
                    value
                        .parse()
                        .map_err(|_| ApiError::invalid("limit must be a positive integer"))?,
                )
            }
            "contexts_available" => {
                parsed.contexts_available.insert(normalize_slug(&value)?);
            }
            "project" if parsed.project.is_none() => parsed.project = Some(value.into_owned()),
            "include_waiting" => {
                parsed.include_waiting = parse_bool_query("include_waiting", &value)?
            }
            "include_parked" => parsed.include_parked = parse_bool_query("include_parked", &value)?,
            "deliberate_all" => parsed.deliberate_all = parse_bool_query("deliberate_all", &value)?,
            "as_of" if parsed.as_of.is_none() => {
                parsed.as_of = Some(
                    DateTime::parse_from_rfc3339(&value)
                        .map_err(|_| ApiError::invalid("as_of must be RFC3339"))?
                        .with_timezone(&Utc),
                )
            }
            "cursor" if parsed.cursor.is_none() => {
                parsed.cursor = Some(parse_task_ref(&value)?);
            }
            _ => {
                return Err(ApiError::invalid(format!(
                    "unknown or repeated task candidates query parameter: {name}"
                )));
            }
        }
    }
    if parsed.contexts_available.len() > 20 {
        return Err(ApiError::invalid(
            "contexts_available accepts at most 20 values",
        ));
    }
    Ok(parsed)
}

fn task_status(value: &str) -> ApiResult<TaskStatus> {
    match value {
        "open" => Ok(TaskStatus::Open),
        "waiting" => Ok(TaskStatus::Waiting),
        "done" => Ok(TaskStatus::Done),
        "dropped" => Ok(TaskStatus::Dropped),
        _ => Err(ApiError::Internal(
            "stored task status is invalid".to_owned(),
        )),
    }
}

fn projected_source<T>(
    provenance: &Value,
    source_timestamps: &Value,
    field: &str,
    value: Option<T>,
) -> ApiResult<Option<Sourced<T>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let source = provenance
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ApiError::Internal(format!(
                "stored task projection is missing {field} provenance"
            ))
        })?
        .to_owned();
    let set_at = source_timestamps
        .get(field)
        .and_then(Value::as_str)
        .and_then(parse_timestamp)
        .ok_or_else(|| {
            ApiError::Internal(format!(
                "stored task projection is missing {field} source timestamp"
            ))
        })?;
    Ok(Some(Sourced {
        value,
        source,
        set_at,
        note: None,
    }))
}

// Candidate reads deliberately use the typed projection instead of fetching
// every canonical task JSON document. The pure task engine remains the sole
// ranking authority; this function only reconstructs its typed input.
fn snapshot_from_projection_row(
    row: &sqlx::postgres::PgRow,
    as_of: DateTime<Utc>,
) -> ApiResult<TaskSnapshot> {
    // Decode each JSONB projection once. Re-decoding both objects for every
    // sourced field is material at the 2,000-task handler gate.
    let provenance = row.get::<Value, _>("provenance");
    let source_timestamps = row.get::<Value, _>("source_timestamps");
    let hard_due_lead_days = row
        .get::<Option<i32>, _>("hard_due_lead_days")
        .map(i64::from);
    let required_contexts = row.get::<Vec<String>, _>("required_contexts");
    let required_contexts =
        if required_contexts.is_empty() && provenance.get("required_contexts").is_none() {
            None
        } else {
            Some(required_contexts)
        };
    let cost_since = row.get::<Option<NaiveDate>, _>("cost_since");
    let cost = match (
        row.get::<Option<i64>, _>("cost_amount_cents"),
        row.get::<Option<String>, _>("cost_period").as_deref(),
        row.get::<bool, _>("cost_flag"),
        cost_since,
    ) {
        (Some(amount_cents), Some("day"), _, Some(since)) => Some(CostOfDelay::Rate {
            amount_cents,
            per: CostPeriod::Day,
            since,
            note: None,
        }),
        (Some(amount_cents), Some("week"), _, Some(since)) => Some(CostOfDelay::Rate {
            amount_cents,
            per: CostPeriod::Week,
            since,
            note: None,
        }),
        (Some(amount_cents), Some("month"), _, Some(since)) => Some(CostOfDelay::Rate {
            amount_cents,
            per: CostPeriod::Month,
            since,
            note: None,
        }),
        (None, None, true, Some(since)) => Some(CostOfDelay::Flag { since, note: None }),
        (None, None, false, None) => None,
        _ => {
            return Err(ApiError::Internal(
                "stored cost_of_delay projection is inconsistent".to_owned(),
            ));
        }
    };
    let explicit_interest = row
        .get::<Option<String>, _>("interest_override")
        .zip(row.get::<Option<DateTime<Utc>>, _>("interest_set_at"));
    let last_activity = row.get::<Option<DateTime<Utc>>, _>("last_activity_at");
    let status = row.get::<String, _>("status");
    Ok(TaskSnapshot {
        id: row.get("task_id"),
        title: row.get("title"),
        status: task_status(&status)?,
        created_at: row.get("created_at"),
        ready_at: projected_source(
            &provenance,
            &source_timestamps,
            "ready_at",
            row.get("ready_at"),
        )?,
        soft_due: projected_source(
            &provenance,
            &source_timestamps,
            "soft_due",
            row.get("soft_due"),
        )?,
        hard_due: projected_source(
            &provenance,
            &source_timestamps,
            "hard_due",
            row.get("hard_due"),
        )?,
        hard_due_lead_days: projected_source(
            &provenance,
            &source_timestamps,
            "hard_due_lead_days",
            hard_due_lead_days,
        )?,
        cost_of_delay: projected_source(&provenance, &source_timestamps, "cost_of_delay", cost)?,
        required_contexts: projected_source(
            &provenance,
            &source_timestamps,
            "required_contexts",
            required_contexts,
        )?,
        project: projected_source(
            &provenance,
            &source_timestamps,
            "project",
            row.get("project_slug"),
        )?,
        project_interest: derive_project_interest(
            explicit_interest
                .as_ref()
                .map(|(interest, set_at)| (interest.as_str(), *set_at)),
            last_activity,
            as_of,
        ),
        project_last_activity: last_activity,
        parked: row.get("parked"),
        waiting: status == "waiting",
        today_pin: projected_source(
            &provenance,
            &source_timestamps,
            "today_pin",
            row.get("today_pin"),
        )?,
        triaged_at: row.get("triaged_at"),
    })
}

fn ranked_item_json(
    ranked: &task_engine::RankedTask,
    details: &HashMap<Uuid, &sqlx::postgres::PgRow>,
) -> ApiResult<Value> {
    let row = details
        .get(&ranked.id)
        .ok_or_else(|| ApiError::Internal("ranked task detail is missing".to_owned()))?;
    let entry_id: Uuid = row.get("entry_id");
    Ok(json!({
        "task_ref": ranked.id,
        "entry_ref": format!("entry:{entry_id}"),
        "version": row.get::<i64,_>("entry_version"),
        "title": ranked.title,
        "status": row.get::<String,_>("status"),
        "project": row.get::<Option<String>,_>("project_slug"),
        "required_contexts": row.get::<Vec<String>,_>("required_contexts"),
        "tier": ranked.tier,
        "reason": ranked.reason,
        "provenance_markers": ranked.provenance_markers,
        "pinned": ranked.pinned,
    }))
}

pub(crate) async fn task_candidates(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskRead)?;
    let query = parse_candidate_query(raw.as_deref())?;
    let view = match query.view.as_deref().unwrap_or("next") {
        "urgent" => TaskView::Urgent,
        "next" => TaskView::Next,
        "triage" => TaskView::Triage,
        "all" if query.deliberate_all => TaskView::All,
        "all" => {
            return Err(ApiError::invalid("view=all requires deliberate_all=true"));
        }
        _ => {
            return Err(ApiError::invalid(
                "view must be urgent, next, triage, or all",
            ));
        }
    };
    if view == TaskView::Urgent && query.limit.is_some() {
        return Err(ApiError::invalid(
            "view=urgent is unbounded and does not accept limit",
        ));
    }
    if view != TaskView::All && query.deliberate_all {
        return Err(ApiError::invalid(
            "deliberate_all is valid only with view=all",
        ));
    }
    let limit = query.limit.unwrap_or(match view {
        TaskView::Next | TaskView::Urgent => 5,
        TaskView::Triage => 10,
        TaskView::All => 25,
    });
    if limit == 0 || limit > 25 || (view == TaskView::Triage && limit > 10) {
        return Err(ApiError::invalid(
            "limit must be 1..25 (and at most 10 for triage)",
        ));
    }
    if view != TaskView::All && query.cursor.is_some() {
        return Err(ApiError::invalid(
            "cursor is supported only with deliberate view=all",
        ));
    }
    let as_of = query.as_of.unwrap_or_else(Utc::now);
    let mut tx = state.begin_read(&auth).await?;
    let project = match query.project.as_deref() {
        Some(project) => Some(resolve_project_in_tx(&mut tx, auth.user_id.0, project).await?),
        None => None,
    };
    let rows = sqlx::query(TASK_CANDIDATE_PROJECTION_SQL)
        .bind(auth.user_id.0)
        .bind(query.include_waiting)
        .bind(query.include_parked)
        .bind(as_of)
        .bind(&project)
        .bind(query.contexts_available.iter().cloned().collect::<Vec<_>>())
        .fetch_all(&mut *tx)
        .await?;
    let backlog_total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM straylight.task_index WHERE user_id=$1 AND status IN ('open','waiting') AND ($2::text IS NULL OR project_slug=$2) AND created_at<=$3",
    )
    .bind(auth.user_id.0)
    .bind(&project)
    .bind(as_of)
    .fetch_one(&mut *tx)
    .await?;
    let settings = sqlx::query(
        "SELECT hard_lead_days,soft_window_days FROM straylight.task_settings WHERE user_id=$1",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?;
    let engine_settings = EngineSettings {
        hard_due_lead_days: settings.get::<i32, _>("hard_lead_days").into(),
        soft_due_window_days: settings.get::<i32, _>("soft_window_days").into(),
    };
    let mut snapshots = Vec::with_capacity(rows.len());
    let mut details = HashMap::with_capacity(rows.len());
    for row in &rows {
        let snapshot = snapshot_from_projection_row(row, as_of)?;
        details.insert(snapshot.id, row);
        snapshots.push(snapshot);
    }
    let effective_contexts = query.contexts_available.clone();
    let engine_request = EngineCandidateRequest {
        view,
        limit,
        contexts_available: query.contexts_available,
        include_waiting: query.include_waiting,
        include_parked: query.include_parked,
        as_of,
    };
    let initial = if view == TaskView::All {
        task_engine::rank_all_tasks(&snapshots, &engine_request, &engine_settings)
    } else {
        task_engine::rank_tasks(&snapshots, &engine_request, &engine_settings)
    };
    let (selected, next_cursor, next_remaining) = if view == TaskView::All {
        let start = match query.cursor {
            Some(cursor) => initial
                .items
                .iter()
                .position(|item| item.id == cursor)
                .map(|index| index + 1)
                .ok_or_else(|| ApiError::invalid("candidate cursor is not in the result set"))?,
            None => 0,
        };
        let end = (start + limit).min(initial.items.len());
        let next = (end < initial.items.len() && end > start).then(|| initial.items[end - 1].id);
        (
            initial.items[start..end].to_vec(),
            next,
            initial.items.len().saturating_sub(end),
        )
    } else {
        (initial.items.clone(), None, initial.next_remaining)
    };
    let items = selected
        .iter()
        .map(|item| ranked_item_json(item, &details))
        .collect::<ApiResult<Vec<_>>>()?;
    tx.commit().await?;
    Ok(envelope(
        ResponseStatus::Complete,
        json!({
            "view": match view { TaskView::Urgent => "urgent", TaskView::Next => "next", TaskView::Triage => "triage", TaskView::All => "all" },
            "as_of": as_of,
            "contexts_available": effective_contexts,
            "items": items,
            "urgent_total": initial.urgent_total,
            "next_remaining": next_remaining,
            "backlog_total": backlog_total,
            "next_cursor": next_cursor,
        }),
    ))
}

#[derive(Default)]
struct CorrectionQuery {
    task_ref: Option<Uuid>,
    limit: usize,
    cursor: Option<Uuid>,
}

fn parse_correction_query(raw: Option<&str>) -> ApiResult<CorrectionQuery> {
    let mut parsed = CorrectionQuery {
        limit: 25,
        ..Default::default()
    };
    let mut seen = BTreeSet::new();
    for (name, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        if !seen.insert(name.to_string()) {
            return Err(ApiError::invalid(format!(
                "repeated corrections query parameter: {name}"
            )));
        }
        match name.as_ref() {
            "task_ref" if parsed.task_ref.is_none() => {
                parsed.task_ref = Some(parse_task_ref(&value)?)
            }
            "limit" => {
                parsed.limit = value
                    .parse()
                    .map_err(|_| ApiError::invalid("limit must be an integer"))?;
            }
            "cursor" if parsed.cursor.is_none() => {
                parsed.cursor = Some(
                    Uuid::parse_str(&value)
                        .map_err(|_| ApiError::invalid("correction cursor must be a UUID"))?,
                );
            }
            _ => {
                return Err(ApiError::invalid(format!(
                    "unknown or repeated corrections query parameter: {name}"
                )));
            }
        }
    }
    if parsed.limit == 0 || parsed.limit > 100 {
        return Err(ApiError::invalid("corrections limit must be 1..100"));
    }
    Ok(parsed)
}

pub(crate) async fn task_corrections(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskRead)?;
    let query = parse_correction_query(raw.as_deref())?;
    let mut tx = state.begin_read(&auth).await?;
    let cursor = match query.cursor {
        Some(id) => Some(
            sqlx::query_scalar::<_, DateTime<Utc>>(
                "SELECT created_at FROM straylight.task_corrections WHERE user_id=$1 AND id=$2",
            )
            .bind(auth.user_id.0)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError::invalid("correction cursor was not found"))?,
        ),
        None => None,
    };
    let rows = sqlx::query(
        r#"
        SELECT id,task_id,entry_version,field_name,previous_value,previous_source,
               corrected_value,corrected_source,reason,created_at
        FROM straylight.task_corrections
        WHERE user_id=$1 AND ($2::uuid IS NULL OR task_id=$2)
          AND ($3::timestamptz IS NULL OR (created_at,id)<($3,$4))
        ORDER BY created_at DESC,id DESC
        LIMIT $5
        "#,
    )
    .bind(auth.user_id.0)
    .bind(query.task_ref)
    .bind(cursor)
    .bind(query.cursor)
    .bind(i64::try_from(query.limit + 1).unwrap_or(101))
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > query.limit;
    let visible = rows.iter().take(query.limit).collect::<Vec<_>>();
    let next_cursor = has_more
        .then(|| visible.last().map(|row| row.get::<Uuid, _>("id")))
        .flatten();
    let items = visible
        .into_iter()
        .map(|row| {
            json!({
                "correction_ref": row.get::<Uuid,_>("id"),
                "task_ref": row.get::<Uuid,_>("task_id"),
                "version": row.get::<i64,_>("entry_version"),
                "field": row.get::<String,_>("field_name"),
                "previous_value": row.get::<Option<Value>,_>("previous_value"),
                "previous_source": row.get::<Option<String>,_>("previous_source"),
                "corrected_value": row.get::<Option<Value>,_>("corrected_value"),
                "corrected_source": row.get::<String,_>("corrected_source"),
                "reason": row.get::<Option<String>,_>("reason"),
                "created_at": row.get::<DateTime<Utc>,_>("created_at"),
            })
        })
        .collect::<Vec<_>>();
    tx.commit().await?;
    Ok(envelope(
        ResponseStatus::Complete,
        json!({"items":items,"next_cursor":next_cursor}),
    ))
}

#[derive(Default)]
struct DoneQuery {
    from: Option<NaiveDate>,
    through: Option<NaiveDate>,
    as_of: Option<DateTime<Utc>>,
    limit: usize,
    cursor: Option<Uuid>,
}

fn parse_done_query(raw: Option<&str>) -> ApiResult<DoneQuery> {
    let mut parsed = DoneQuery {
        limit: 25,
        ..Default::default()
    };
    let mut seen = BTreeSet::new();
    for (name, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        if !seen.insert(name.to_string()) {
            return Err(ApiError::invalid(format!(
                "repeated done-summary query parameter: {name}"
            )));
        }
        match name.as_ref() {
            "from" if parsed.from.is_none() => parsed.from = Some(parse_date_required(&value)?),
            "through" if parsed.through.is_none() => {
                parsed.through = Some(parse_date_required(&value)?)
            }
            "as_of" if parsed.as_of.is_none() => {
                parsed.as_of = Some(
                    DateTime::parse_from_rfc3339(&value)
                        .map_err(|_| ApiError::invalid("as_of must be RFC3339"))?
                        .with_timezone(&Utc),
                )
            }
            "limit" => {
                parsed.limit = value
                    .parse()
                    .map_err(|_| ApiError::invalid("limit must be an integer"))?
            }
            "cursor" if parsed.cursor.is_none() => parsed.cursor = Some(parse_task_ref(&value)?),
            _ => {
                return Err(ApiError::invalid(format!(
                    "unknown or repeated done-summary query parameter: {name}"
                )));
            }
        }
    }
    if parsed.from.is_some() != parsed.through.is_some() {
        return Err(ApiError::invalid(
            "from and through must be supplied together",
        ));
    }
    if parsed.limit == 0 || parsed.limit > 100 {
        return Err(ApiError::invalid("done-summary limit must be 1..100"));
    }
    if parsed
        .from
        .zip(parsed.through)
        .is_some_and(|(from, through)| {
            from > through || through.signed_duration_since(from).num_days() > 366
        })
    {
        return Err(ApiError::invalid(
            "done-summary range must be ordered and at most 366 days",
        ));
    }
    Ok(parsed)
}

pub(crate) async fn task_done_summary(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskRead)?;
    let query = parse_done_query(raw.as_deref())?;
    let as_of = query.as_of.unwrap_or_else(Utc::now);
    let mut tx = state.begin_read(&auth).await?;
    let (today, timezone) = owner_local_date_in_tx(&mut tx, auth.user_id.0, as_of).await?;
    let from = query.from.unwrap_or(today);
    let through = query.through.unwrap_or(today);
    let cursor_done_at = match query.cursor {
        Some(task_id) => Some(
            sqlx::query_scalar::<_, DateTime<Utc>>(
                "SELECT done_at FROM straylight.task_index WHERE user_id=$1 AND task_id=$2 AND status='done'",
            )
            .bind(auth.user_id.0)
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError::invalid("done-summary cursor was not found"))?,
        ),
        None => None,
    };
    let total = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM straylight.task_index
        WHERE user_id=$1 AND status='done'
          AND (done_at AT TIME ZONE $2)::date BETWEEN $3 AND $4
          AND done_at <= $5
        "#,
    )
    .bind(auth.user_id.0)
    .bind(&timezone)
    .bind(from)
    .bind(through)
    .bind(as_of)
    .fetch_one(&mut *tx)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT task_id,entry_id,entry_version,title,done_at,task
        FROM straylight.task_index
        WHERE user_id=$1 AND status='done'
          AND (done_at AT TIME ZONE $2)::date BETWEEN $3 AND $4
          AND done_at <= $5
          AND ($6::timestamptz IS NULL OR (done_at,task_id)<($6,$7))
        ORDER BY done_at DESC,task_id DESC
        LIMIT $8
        "#,
    )
    .bind(auth.user_id.0)
    .bind(&timezone)
    .bind(from)
    .bind(through)
    .bind(as_of)
    .bind(cursor_done_at)
    .bind(query.cursor)
    .bind(i64::try_from(query.limit + 1).unwrap_or(101))
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > query.limit;
    let visible = rows.iter().take(query.limit).collect::<Vec<_>>();
    let next_cursor = has_more
        .then(|| visible.last().map(|row| row.get::<Uuid, _>("task_id")))
        .flatten();
    let items = visible
        .into_iter()
        .map(|row| {
            let entry_id: Uuid = row.get("entry_id");
            let completed_via = row
                .get::<Value, _>("task")
                .get("completed_via")
                .and_then(|value| value.get("value"))
                .cloned();
            json!({
                "task_ref": row.get::<Uuid,_>("task_id"),
                "entry_ref": format!("entry:{entry_id}"),
                "version": row.get::<i64,_>("entry_version"),
                "title": row.get::<String,_>("title"),
                "done_at": row.get::<DateTime<Utc>,_>("done_at"),
                "completed_via": completed_via,
            })
        })
        .collect::<Vec<_>>();
    let done_today_count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM straylight.task_index
        WHERE user_id=$1 AND status='done'
          AND (done_at AT TIME ZONE $2)::date=$3
          AND done_at <= $4
        "#,
    )
    .bind(auth.user_id.0)
    .bind(&timezone)
    .bind(today)
    .bind(as_of)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(envelope(
        ResponseStatus::Complete,
        json!({
            "from": from,
            "through": through,
            "timezone": timezone,
            "as_of": as_of,
            "count": total,
            "done_today_count": done_today_count,
            "items": items,
            "next_cursor": next_cursor,
        }),
    ))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateContextRequest {
    pub display_name: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub source: String,
    #[serde(default)]
    pub confirm_new: bool,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MergeContextsRequest {
    pub from: String,
    pub into: String,
    pub expected_from_version: i64,
    pub expected_into_version: i64,
    pub source: String,
    #[serde(default)]
    pub reason: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveContextRequest {
    pub archived: bool,
    pub expected_version: i64,
    pub source: String,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetAvailableContextsRequest {
    pub contexts_available: Vec<String>,
    pub expected_version: i64,
    pub source: String,
    pub idempotency_key: String,
}

#[derive(Default)]
struct ContextListQuery {
    include_archived: bool,
    limit: usize,
    cursor: Option<String>,
}

fn parse_context_list_query(raw: Option<&str>) -> ApiResult<ContextListQuery> {
    let mut parsed = ContextListQuery {
        limit: 50,
        ..Default::default()
    };
    let mut seen = BTreeSet::new();
    for (name, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        if !seen.insert(name.to_string()) {
            return Err(ApiError::invalid(format!(
                "repeated contexts query parameter: {name}"
            )));
        }
        match name.as_ref() {
            "include_archived" => {
                parsed.include_archived = parse_bool_query("include_archived", &value)?
            }
            "limit" => {
                parsed.limit = value
                    .parse()
                    .map_err(|_| ApiError::invalid("limit must be an integer"))?
            }
            "cursor" => {
                if normalize_slug(&value)? != value {
                    return Err(ApiError::invalid("context cursor must be a canonical slug"));
                }
                parsed.cursor = Some(value.into_owned());
            }
            _ => {
                return Err(ApiError::invalid(format!(
                    "unknown contexts query parameter: {name}"
                )));
            }
        }
    }
    if parsed.limit == 0 || parsed.limit > 100 {
        return Err(ApiError::invalid("contexts limit must be 1..100"));
    }
    Ok(parsed)
}

pub(crate) async fn list_contexts(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskRead)?;
    let query = parse_context_list_query(raw.as_deref())?;
    let mut tx = state.begin_read(&auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT context.slug,context.display_name,context.description,context.archived_at,
               context.created_by,context.version,context.created_at,context.updated_at,
               COALESCE(array_agg(alias.alias ORDER BY lower(alias.alias))
                 FILTER (WHERE alias.alias IS NOT NULL),'{}'::text[]) AS aliases,
               (SELECT count(*) FROM straylight.task_index AS task
                WHERE task.user_id=context.user_id
                  AND task.required_contexts @> ARRAY[context.slug]::text[]
                  AND task.status IN ('open','waiting')) AS active_task_count
        FROM straylight.task_contexts AS context
        LEFT JOIN straylight.task_context_aliases AS alias
          ON alias.user_id=context.user_id AND alias.context_slug=context.slug
        WHERE context.user_id=$1
          AND ($2 OR context.archived_at IS NULL)
          AND ($3::text IS NULL OR context.slug>$3)
        GROUP BY context.user_id,context.slug
        ORDER BY context.slug
        LIMIT $4
        "#,
    )
    .bind(auth.user_id.0)
    .bind(query.include_archived)
    .bind(&query.cursor)
    .bind(i64::try_from(query.limit + 1).unwrap_or(101))
    .fetch_all(&mut *tx)
    .await?;
    let defaults = sqlx::query(
        "SELECT surface,contexts,version,updated_at FROM straylight.task_surface_defaults WHERE user_id=$1 ORDER BY surface",
    )
    .bind(auth.user_id.0)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|row| {
        (
            row.get::<String, _>("surface"),
            json!({
                "contexts_available": row.get::<Vec<String>,_>("contexts"),
                "version": row.get::<i64,_>("version"),
                "updated_at": row.get::<DateTime<Utc>,_>("updated_at"),
            }),
        )
    })
    .collect::<BTreeMap<_, _>>();
    let has_more = rows.len() > query.limit;
    let visible = rows.iter().take(query.limit).collect::<Vec<_>>();
    let next_cursor = has_more
        .then(|| visible.last().map(|row| row.get::<String, _>("slug")))
        .flatten();
    let contexts = visible
        .into_iter()
        .map(|row| {
            json!({
                "slug": row.get::<String,_>("slug"),
                "display_name": row.get::<String,_>("display_name"),
                "aliases": row.get::<Vec<String>,_>("aliases"),
                "description": row.get::<Option<String>,_>("description"),
                "archived": row.get::<Option<DateTime<Utc>>,_>("archived_at").is_some(),
                "created_by": row.get::<String,_>("created_by"),
                "version": row.get::<i64,_>("version"),
                "active_task_count": row.get::<i64,_>("active_task_count"),
                "created_at": row.get::<DateTime<Utc>,_>("created_at"),
                "updated_at": row.get::<DateTime<Utc>,_>("updated_at"),
            })
        })
        .collect::<Vec<_>>();
    tx.commit().await?;
    Ok(envelope(
        ResponseStatus::Complete,
        json!({
            "contexts": contexts,
            "surface_defaults": defaults,
            "next_cursor": next_cursor,
        }),
    ))
}

pub(crate) async fn create_context(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CreateContextRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskWrite)?;
    let source = canonical_public_source(&auth, &request.source)?;
    let display_name = request.display_name.trim();
    if display_name.is_empty()
        || display_name.len() > 120
        || has_forbidden_control(display_name, false)
    {
        return Err(ApiError::invalid(
            "display_name must contain 1 to 120 printable characters",
        ));
    }
    if request
        .description
        .as_deref()
        .is_some_and(|value| value.len() > 1000 || has_forbidden_control(value, true))
    {
        return Err(ApiError::invalid(
            "context description must be printable and at most 1000 characters",
        ));
    }
    if request.aliases.len() > 32
        || request.aliases.iter().any(|alias| {
            alias.trim().is_empty()
                || alias.trim().len() > 120
                || alias.chars().any(char::is_control)
        })
    {
        return Err(ApiError::invalid(
            "context aliases must contain at most 32 printable values of 1 to 120 characters",
        ));
    }
    let slug = match request.slug.as_deref() {
        Some(slug) if normalize_slug(slug)? == slug => slug.to_owned(),
        Some(_) => {
            return Err(ApiError::invalid(
                "context slug must be canonical kebab case",
            ));
        }
        None => normalize_slug(display_name)?,
    };
    let mut tx = state.begin_write(&auth).await?;
    match begin_receipt(
        &mut tx,
        &auth,
        "context.create",
        &request.idempotency_key,
        &request,
    )
    .await?
    {
        ReceiptStart::Replay(receipt) => {
            tx.commit().await?;
            return Ok(envelope(ResponseStatus::NoOp, receipt));
        }
        ReceiptStart::New => {}
    }
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM straylight.task_contexts WHERE user_id=$1 AND slug=$2)",
    )
    .bind(auth.user_id.0)
    .bind(&slug)
    .fetch_one(&mut *tx)
    .await?
    {
        return Err(ApiError::conflict(
            "context_exists",
            "a context already uses this slug",
            json!({"slug": slug}),
        ));
    }
    let mut suggestions = BTreeMap::<String, &'static str>::new();
    for candidate in std::iter::once(display_name)
        .chain(std::iter::once(slug.as_str()))
        .chain(request.aliases.iter().map(String::as_str))
    {
        for suggestion in context_suggestions_in_tx(&mut tx, auth.user_id.0, candidate).await? {
            suggestions
                .entry(suggestion.slug)
                .or_insert(suggestion.reason);
        }
    }
    if !suggestions.is_empty() && !request.confirm_new {
        tx.rollback().await?;
        return Ok(envelope(
            ResponseStatus::NeedsReview,
            json!({
                "suggested_existing": suggestions.into_iter().map(|(slug,reason)| json!({"slug":slug,"reason":reason})).collect::<Vec<_>>(),
                "requested": {"slug":slug,"display_name":display_name},
                "replayed": false,
            }),
        ));
    }
    create_context_in_tx(
        &mut tx,
        auth.user_id.0,
        auth.credential_id.0,
        &slug,
        Some(display_name),
        request.description.as_deref(),
        &source,
        true,
    )
    .await?;
    let mut aliases = BTreeSet::new();
    for alias in &request.aliases {
        let normalized = normalize_slug(alias.trim())?;
        if normalized == slug || !aliases.insert(normalized.clone()) {
            continue;
        }
        let collision = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
              SELECT 1 FROM straylight.task_contexts WHERE user_id=$1 AND slug=$2
              UNION ALL
              SELECT 1 FROM straylight.task_context_aliases WHERE user_id=$1 AND lower(alias)=lower($3)
            )
            "#,
        )
        .bind(auth.user_id.0)
        .bind(&normalized)
        .bind(&normalized)
        .fetch_one(&mut *tx)
        .await?;
        if collision {
            return Err(ApiError::conflict(
                "context_alias_conflict",
                "a context or alias already uses the requested alias",
                json!({"alias":normalized}),
            ));
        }
        sqlx::query(
            "INSERT INTO straylight.task_context_aliases(user_id,alias,context_slug,reason) VALUES ($1,$2,$3,$4)",
        )
        .bind(auth.user_id.0)
        .bind(&normalized)
        .bind(&slug)
        .bind(&source)
        .execute(&mut *tx)
        .await?;
    }
    let receipt = json!({
        "context": {"slug":slug,"display_name":display_name,"aliases":aliases,"description":request.description,"version":1},
        "replayed": false,
    });
    finalize_receipt(
        &mut tx,
        &auth,
        "context.create",
        &request.idempotency_key,
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(envelope(ResponseStatus::Committed, receipt))
}

pub(crate) async fn merge_contexts(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<MergeContextsRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskWrite)?;
    let source = canonical_public_source(&auth, &request.source)?;
    if request.expected_from_version < 1 || request.expected_into_version < 1 {
        return Err(ApiError::invalid(
            "context merge expected versions must be positive",
        ));
    }
    if request
        .reason
        .as_deref()
        .is_some_and(|value| value.len() > 1000 || has_forbidden_control(value, true))
    {
        return Err(ApiError::invalid(
            "merge reason must be printable and at most 1000 characters",
        ));
    }
    let mut tx = state.begin_write(&auth).await?;
    match begin_receipt(
        &mut tx,
        &auth,
        "context.merge",
        &request.idempotency_key,
        &request,
    )
    .await?
    {
        ReceiptStart::Replay(receipt) => {
            tx.commit().await?;
            return Ok(envelope(ResponseStatus::NoOp, receipt));
        }
        ReceiptStart::New => {}
    }
    let from = normalize_slug(&request.from)?;
    let into = normalize_slug(&request.into)?;
    if from == into {
        return Err(ApiError::invalid(
            "context merge source and target must differ",
        ));
    }
    let locked = sqlx::query(
        "SELECT slug,version FROM straylight.task_contexts WHERE user_id=$1 AND slug=ANY($2) ORDER BY slug FOR UPDATE",
    )
    .bind(auth.user_id.0)
    .bind(vec![from.clone(), into.clone()])
    .fetch_all(&mut *tx)
    .await?;
    if locked.len() != 2 {
        return Err(ApiError::not_found(
            "context_not_found",
            "context merge source or target",
        ));
    }
    let versions = locked
        .into_iter()
        .map(|row| (row.get::<String, _>("slug"), row.get::<i64, _>("version")))
        .collect::<BTreeMap<_, _>>();
    let current_from = versions[&from];
    let current_into = versions[&into];
    if current_from != request.expected_from_version
        || current_into != request.expected_into_version
    {
        return Err(ApiError::conflict(
            "context_version_conflict",
            "a context changed after the merge expected versions",
            json!({
                "from":{"expected":request.expected_from_version,"current":current_from},
                "into":{"expected":request.expected_into_version,"current":current_into},
            }),
        ));
    }
    let rewritten = merge_contexts_in_tx(
        &mut tx,
        auth.user_id.0,
        auth.credential_id.0,
        &request.from,
        &request.into,
        &source,
        Utc::now(),
    )
    .await?;
    sqlx::query(
        "UPDATE straylight.task_contexts SET version=version+1,updated_at=clock_timestamp() WHERE user_id=$1 AND slug=$2",
    )
    .bind(auth.user_id.0)
    .bind(normalize_slug(&request.into)?)
    .execute(&mut *tx)
    .await?;
    if let Some(reason) = &request.reason {
        sqlx::query(
            "INSERT INTO straylight.task_audit_events(user_id,credential_id,action,details) VALUES ($1,$2,'context.merge.reason',$3)",
        )
        .bind(auth.user_id.0)
        .bind(auth.credential_id.0)
        .bind(json!({"reason":reason,"source":source}))
        .execute(&mut *tx)
        .await?;
    }
    let receipt = json!({
        "from": normalize_slug(&request.from)?,
        "into": normalize_slug(&request.into)?,
        "tasks_rewritten": rewritten,
        "replayed": false,
    });
    finalize_receipt(
        &mut tx,
        &auth,
        "context.merge",
        &request.idempotency_key,
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    state.workspace_features.invalidate(auth.user_id.0).await;
    Ok(envelope(ResponseStatus::Committed, receipt))
}

pub(crate) async fn archive_context(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(slug): Path<String>,
    Json(request): Json<ArchiveContextRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskWrite)?;
    let source = canonical_public_source(&auth, &request.source)?;
    if normalize_slug(&slug)? != slug || request.expected_version < 1 {
        return Err(ApiError::invalid(
            "context slug or expected_version is invalid",
        ));
    }
    let mut tx = state.begin_write(&auth).await?;
    let receipt_request = json!({"slug":slug,"request":&request});
    match begin_receipt(
        &mut tx,
        &auth,
        "context.archive",
        &request.idempotency_key,
        &receipt_request,
    )
    .await?
    {
        ReceiptStart::Replay(receipt) => {
            tx.commit().await?;
            return Ok(envelope(ResponseStatus::NoOp, receipt));
        }
        ReceiptStart::New => {}
    }
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM straylight.task_contexts WHERE user_id=$1 AND slug=$2 FOR UPDATE",
    )
    .bind(auth.user_id.0)
    .bind(&slug)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("context_not_found", &slug))?;
    if current != request.expected_version {
        return Err(ApiError::conflict(
            "context_version_conflict",
            "the context changed after expected_version",
            json!({"expected_version":request.expected_version,"current_version":current}),
        ));
    }
    let next = current + 1;
    sqlx::query(
        "UPDATE straylight.task_contexts SET archived_at=CASE WHEN $3 THEN clock_timestamp() ELSE NULL END,updated_at=clock_timestamp(),version=$4 WHERE user_id=$1 AND slug=$2",
    )
    .bind(auth.user_id.0)
    .bind(&slug)
    .bind(request.archived)
    .bind(next)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO straylight.task_audit_events(user_id,credential_id,action,details) VALUES ($1,$2,'context.archive',$3)",
    )
    .bind(auth.user_id.0)
    .bind(auth.credential_id.0)
    .bind(json!({"slug":slug,"archived":request.archived,"source":source}))
    .execute(&mut *tx)
    .await?;
    let receipt = json!({"context":{"slug":slug,"archived":request.archived,"version":next},"replayed":false});
    finalize_receipt(
        &mut tx,
        &auth,
        "context.archive",
        &request.idempotency_key,
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(envelope(ResponseStatus::Committed, receipt))
}

pub(crate) async fn set_available_contexts(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(surface): Path<String>,
    Json(request): Json<SetAvailableContextsRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskWrite)?;
    let source = canonical_public_source(&auth, &request.source)?;
    let valid_surface = !surface.is_empty()
        && surface.len() <= 64
        && surface
            .chars()
            .next()
            .is_some_and(|value| value.is_ascii_lowercase())
        && surface.chars().all(|value| {
            value.is_ascii_lowercase() || value.is_ascii_digit() || "._-".contains(value)
        });
    if !valid_surface || request.contexts_available.len() > 20 || request.expected_version < 0 {
        return Err(ApiError::invalid(
            "surface or contexts_available is invalid",
        ));
    }
    let mut contexts = request
        .contexts_available
        .iter()
        .map(|value| normalize_slug(value))
        .collect::<ApiResult<Vec<_>>>()?;
    contexts.sort();
    contexts.dedup();
    let mut tx = state.begin_write(&auth).await?;
    let receipt_request = json!({"surface":surface,"request":&request});
    match begin_receipt(
        &mut tx,
        &auth,
        "context.set_available",
        &request.idempotency_key,
        &receipt_request,
    )
    .await?
    {
        ReceiptStart::Replay(receipt) => {
            tx.commit().await?;
            return Ok(envelope(ResponseStatus::NoOp, receipt));
        }
        ReceiptStart::New => {}
    }
    let settings_version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM straylight.task_settings WHERE user_id=$1 FOR UPDATE",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?;
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM straylight.task_surface_defaults WHERE user_id=$1 AND surface=$2 FOR UPDATE",
    )
    .bind(auth.user_id.0)
    .bind(&surface)
    .fetch_optional(&mut *tx)
    .await?;
    if current.unwrap_or(0) != request.expected_version {
        return Err(ApiError::conflict(
            "surface_version_conflict",
            "the surface defaults changed after expected_version",
            json!({"expected_version":request.expected_version,"current_version":current.unwrap_or(0)}),
        ));
    }
    let existing = sqlx::query_scalar::<_, String>(
        "SELECT slug FROM straylight.task_contexts WHERE user_id=$1 AND slug=ANY($2) AND archived_at IS NULL",
    )
    .bind(auth.user_id.0)
    .bind(&contexts)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    let missing = contexts
        .iter()
        .filter(|value| !existing.contains(*value))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(ApiError::invalid(format!(
            "unknown or archived contexts: {}",
            missing.into_iter().cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    let version = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO straylight.task_surface_defaults(user_id,surface,contexts,version)
        VALUES ($1,$2,$3,1)
        ON CONFLICT (user_id,surface) DO UPDATE SET
          contexts=EXCLUDED.contexts,version=task_surface_defaults.version+1,
          updated_at=clock_timestamp()
        RETURNING version
        "#,
    )
    .bind(auth.user_id.0)
    .bind(&surface)
    .bind(&contexts)
    .fetch_one(&mut *tx)
    .await?;
    let next_settings_version = settings_version + 1;
    sqlx::query(
        "UPDATE straylight.task_settings SET version=$2,updated_at=clock_timestamp() WHERE user_id=$1",
    )
    .bind(auth.user_id.0)
    .bind(next_settings_version)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO straylight.task_audit_events(user_id,credential_id,action,details) VALUES ($1,$2,'context.set_available',$3)",
    )
    .bind(auth.user_id.0)
    .bind(auth.credential_id.0)
    .bind(json!({"surface":surface,"count":contexts.len(),"source":source}))
    .execute(&mut *tx)
    .await?;
    let receipt = json!({"surface":surface,"contexts_available":contexts,"version":version,"settings_version":next_settings_version,"replayed":false});
    finalize_receipt(
        &mut tx,
        &auth,
        "context.set_available",
        &request.idempotency_key,
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(envelope(ResponseStatus::Committed, receipt))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisterProjectRequest {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub hub_path: Option<String>,
    #[serde(default)]
    pub repo_path: Option<String>,
    #[serde(default)]
    pub archived: Option<bool>,
    #[serde(default)]
    pub expected_version: Option<i64>,
    pub source: String,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetProjectInterestRequest {
    pub interest: String,
    pub expected_version: i64,
    pub source: String,
    pub idempotency_key: String,
}

fn canonical_optional_path(
    value: Option<&str>,
    name: &str,
    allow_absolute: bool,
) -> ApiResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(ApiError::invalid(format!("{name} is invalid")));
    }
    if value == "/" {
        return Err(ApiError::invalid(format!(
            "{name} cannot be the filesystem root"
        )));
    }
    let value = value.trim_end_matches('/');
    if value.contains('\\')
        || value.contains("//")
        || value.split('/').enumerate().any(|(index, component)| {
            (component.is_empty() && !(allow_absolute && index == 0))
                || matches!(component, "." | "..")
        })
    {
        return Err(ApiError::invalid(format!(
            "{name} must use canonical path components without traversal"
        )));
    }
    if !allow_absolute && value.starts_with('/') {
        return Err(ApiError::invalid(format!(
            "{name} must be workspace-relative"
        )));
    }
    Ok(Some(value.to_owned()))
}

fn validate_project_path_slug(slug: &str) -> ApiResult<()> {
    validate_project_slug(slug)?;
    if slug != slug.to_ascii_lowercase() {
        return Err(ApiError::invalid(
            "project slug must be canonical lowercase kebab case",
        ));
    }
    Ok(())
}

pub(crate) async fn register_project(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(slug): Path<String>,
    Json(request): Json<RegisterProjectRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskWrite)?;
    let source = canonical_public_source(&auth, &request.source)?;
    validate_project_path_slug(&slug)?;
    let title = request.title.trim();
    if title.is_empty() || title.len() > 200 || has_forbidden_control(title, false) {
        return Err(ApiError::invalid(
            "project title must contain 1 to 200 printable characters",
        ));
    }
    if request.expected_version.is_some_and(|value| value < 0) {
        return Err(ApiError::invalid(
            "expected_version must be nonnegative when supplied",
        ));
    }
    if request
        .description
        .as_deref()
        .is_some_and(|value| value.len() > 2000 || has_forbidden_control(value, true))
    {
        return Err(ApiError::invalid(
            "project description must be printable and at most 2000 characters",
        ));
    }
    if request.aliases.len() > 32 {
        return Err(ApiError::invalid("projects accept at most 32 aliases"));
    }
    let hub_path = canonical_optional_path(request.hub_path.as_deref(), "hub_path", false)?;
    let repo_path = canonical_optional_path(request.repo_path.as_deref(), "repo_path", true)?;
    let aliases = request
        .aliases
        .iter()
        .map(|alias| normalize_slug(alias))
        .collect::<ApiResult<BTreeSet<_>>>()?;
    let mut tx = state.begin_write(&auth).await?;
    let receipt_request = json!({"slug":slug,"request":&request});
    match begin_receipt(
        &mut tx,
        &auth,
        "project.register",
        &request.idempotency_key,
        &receipt_request,
    )
    .await?
    {
        ReceiptStart::Replay(receipt) => {
            tx.commit().await?;
            return Ok(envelope(ResponseStatus::NoOp, receipt));
        }
        ReceiptStart::New => {}
    }
    let existing = sqlx::query(
        "SELECT version,archived_at FROM straylight.task_projects WHERE user_id=$1 AND slug=$2 FOR UPDATE",
    )
    .bind(auth.user_id.0)
    .bind(&slug)
    .fetch_optional(&mut *tx)
    .await?;
    let version = if let Some(existing) = existing {
        let current: i64 = existing.get("version");
        let expected = request.expected_version.ok_or_else(|| {
            ApiError::conflict(
                "project_expected_version_required",
                "expected_version is required when replacing an existing project",
                json!({"current_version":current}),
            )
        })?;
        if expected != current {
            return Err(ApiError::conflict(
                "project_version_conflict",
                "the project changed after expected_version",
                json!({"expected_version":expected,"current_version":current}),
            ));
        }
        let next = current + 1;
        sqlx::query(
            r#"
            UPDATE straylight.task_projects SET
              title=$3,description=$4,hub_path=$5,repo_path=$6,
              archived_at=CASE
                WHEN $7::boolean IS NULL THEN archived_at
                WHEN $7 THEN clock_timestamp()
                ELSE NULL
              END,
              version=$8,updated_at=clock_timestamp()
            WHERE user_id=$1 AND slug=$2
            "#,
        )
        .bind(auth.user_id.0)
        .bind(&slug)
        .bind(title)
        .bind(&request.description)
        .bind(&hub_path)
        .bind(&repo_path)
        .bind(request.archived)
        .bind(next)
        .execute(&mut *tx)
        .await?;
        next
    } else {
        if request.expected_version.is_some_and(|value| value != 0) {
            return Err(ApiError::conflict(
                "project_not_found",
                "expected_version cannot create a missing project",
                json!({"slug":slug}),
            ));
        }
        sqlx::query(
            r#"
            INSERT INTO straylight.task_projects(
              user_id,slug,title,description,hub_path,repo_path,archived_at,created_by
            ) VALUES ($1,$2,$3,$4,$5,$6,CASE WHEN $7 THEN clock_timestamp() ELSE NULL END,$8)
            "#,
        )
        .bind(auth.user_id.0)
        .bind(&slug)
        .bind(title)
        .bind(&request.description)
        .bind(&hub_path)
        .bind(&repo_path)
        .bind(request.archived.unwrap_or(false))
        .bind(&source)
        .execute(&mut *tx)
        .await?;
        1
    };
    sqlx::query("DELETE FROM straylight.task_project_aliases WHERE user_id=$1 AND project_slug=$2")
        .bind(auth.user_id.0)
        .bind(&slug)
        .execute(&mut *tx)
        .await?;
    for alias in &aliases {
        if alias == &slug {
            continue;
        }
        let collision = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
              SELECT 1 FROM straylight.task_projects WHERE user_id=$1 AND slug=$2
              UNION ALL
              SELECT 1 FROM straylight.task_project_aliases WHERE user_id=$1 AND lower(alias)=lower($2)
            )
            "#,
        )
        .bind(auth.user_id.0)
        .bind(alias)
        .fetch_one(&mut *tx)
        .await?;
        if collision {
            return Err(ApiError::conflict(
                "project_alias_conflict",
                "a project or alias already uses the requested alias",
                json!({"alias":alias}),
            ));
        }
        sqlx::query(
            "INSERT INTO straylight.task_project_aliases(user_id,alias,project_slug,reason) VALUES ($1,$2,$3,$4)",
        )
        .bind(auth.user_id.0)
        .bind(alias)
        .bind(&slug)
        .bind(&source)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO straylight.task_audit_events(user_id,credential_id,action,details) VALUES ($1,$2,'project.register',$3)",
    )
    .bind(auth.user_id.0)
    .bind(auth.credential_id.0)
    .bind(json!({"slug":slug,"version":version,"source":source}))
    .execute(&mut *tx)
    .await?;
    let archived = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NOT NULL FROM straylight.task_projects WHERE user_id=$1 AND slug=$2",
    )
    .bind(auth.user_id.0)
    .bind(&slug)
    .fetch_one(&mut *tx)
    .await?;
    let receipt = json!({
        "project":{"slug":slug,"title":title,"description":request.description,"aliases":aliases,"hub_path":hub_path,"repo_path":repo_path,"archived":archived,"version":version},
        "replayed":false,
    });
    finalize_receipt(
        &mut tx,
        &auth,
        "project.register",
        &request.idempotency_key,
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(envelope(ResponseStatus::Committed, receipt))
}

#[derive(Default)]
struct ProjectListQuery {
    include_archived: bool,
    limit: usize,
    cursor: Option<String>,
    as_of: Option<DateTime<Utc>>,
}

fn parse_project_list_query(raw: Option<&str>) -> ApiResult<ProjectListQuery> {
    let mut parsed = ProjectListQuery {
        limit: 50,
        ..Default::default()
    };
    let mut seen = BTreeSet::new();
    for (name, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        if !seen.insert(name.to_string()) {
            return Err(ApiError::invalid(format!(
                "repeated projects query parameter: {name}"
            )));
        }
        match name.as_ref() {
            "include_archived" => {
                parsed.include_archived = parse_bool_query("include_archived", &value)?
            }
            "limit" => {
                parsed.limit = value
                    .parse()
                    .map_err(|_| ApiError::invalid("limit must be an integer"))?
            }
            "cursor" => {
                validate_project_path_slug(&value)?;
                parsed.cursor = Some(value.into_owned());
            }
            "as_of" => {
                parsed.as_of = Some(
                    DateTime::parse_from_rfc3339(&value)
                        .map_err(|_| ApiError::invalid("as_of must be RFC3339"))?
                        .with_timezone(&Utc),
                )
            }
            _ => {
                return Err(ApiError::invalid(format!(
                    "unknown projects query parameter: {name}"
                )));
            }
        }
    }
    if parsed.limit == 0 || parsed.limit > 100 {
        return Err(ApiError::invalid("projects limit must be 1..100"));
    }
    Ok(parsed)
}

pub(crate) async fn list_projects(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskRead)?;
    let query = parse_project_list_query(raw.as_deref())?;
    let as_of = query.as_of.unwrap_or_else(Utc::now);
    let mut tx = state.begin_read(&auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT project.slug,project.title,project.description,project.hub_path,
               project.repo_path,project.interest_override,project.interest_set_by,
               project.interest_set_at,project.last_activity_at,project.archived_at,
               project.created_by,project.version,project.created_at,project.updated_at,
               COALESCE(aliases.values,'{}'::text[]) AS aliases,
               COALESCE(tasks.open_task_count,0) AS open_task_count,
               checkpoints.last_checkpoint_at
        FROM straylight.task_projects AS project
        LEFT JOIN LATERAL(
          SELECT array_agg(alias ORDER BY alias) AS values
          FROM straylight.task_project_aliases
          WHERE user_id=project.user_id AND project_slug=project.slug
        ) AS aliases ON true
        LEFT JOIN LATERAL(
          SELECT count(*) AS open_task_count FROM straylight.task_index
          WHERE user_id=project.user_id AND project_slug=project.slug
            AND status IN ('open','waiting')
            AND created_at <= $4
        ) AS tasks ON true
        LEFT JOIN LATERAL(
          SELECT max(version.created_at) AS last_checkpoint_at
          FROM straylight.task_checkpoint_links AS link
          JOIN straylight.entries AS entry
            ON entry.user_id=link.user_id AND entry.id=link.checkpoint_entry_id
          JOIN straylight.entry_versions AS version
            ON version.user_id=entry.user_id
           AND version.entry_id=entry.id
           AND version.version=entry.current_version
          WHERE link.user_id=project.user_id AND link.project_slug=project.slug
            AND version.created_at <= $4
        ) AS checkpoints ON true
        WHERE project.user_id=$1 AND project.created_at <= $4
          AND ($2 OR project.archived_at IS NULL OR project.archived_at > $4)
          AND ($3::text IS NULL OR project.slug>$3)
        ORDER BY project.slug
        LIMIT $5
        "#,
    )
    .bind(auth.user_id.0)
    .bind(query.include_archived)
    .bind(&query.cursor)
    .bind(as_of)
    .bind(i64::try_from(query.limit + 1).unwrap_or(101))
    .fetch_all(&mut *tx)
    .await?;
    let has_more = rows.len() > query.limit;
    let visible = rows.iter().take(query.limit).collect::<Vec<_>>();
    let next_cursor = has_more
        .then(|| visible.last().map(|row| row.get::<String, _>("slug")))
        .flatten();
    let projects = visible.into_iter().map(|row| {
        let override_value = row.get::<Option<String>,_>("interest_override");
        let set_at = row.get::<Option<DateTime<Utc>>,_>("interest_set_at");
        let last_activity = row.get::<Option<DateTime<Utc>>,_>("last_activity_at");
        let interest = derive_project_interest(
            override_value.as_deref().zip(set_at),
            last_activity,
            as_of,
        );
        json!({
            "slug":row.get::<String,_>("slug"),"title":row.get::<String,_>("title"),
            "description":row.get::<Option<String>,_>("description"),
            "aliases":row.get::<Vec<String>,_>("aliases"),
            "hub_path":row.get::<Option<String>,_>("hub_path"),"repo_path":row.get::<Option<String>,_>("repo_path"),
            "interest":match interest { ProjectInterest::Hot=>"hot",ProjectInterest::Normal=>"normal",ProjectInterest::Parked=>"parked" },
            "interest_override":override_value,"interest_set_by":row.get::<Option<String>,_>("interest_set_by"),
            "interest_set_at":set_at,"last_activity_at":last_activity,
            "archived":row.get::<Option<DateTime<Utc>>,_>("archived_at").is_some(),
            "open_task_count":row.get::<i64,_>("open_task_count"),
            "last_checkpoint_at":row.get::<Option<DateTime<Utc>>,_>("last_checkpoint_at"),
            "version":row.get::<i64,_>("version"),"created_by":row.get::<String,_>("created_by"),
        })
    }).collect::<Vec<_>>();
    tx.commit().await?;
    Ok(envelope(
        ResponseStatus::Complete,
        json!({"projects":projects,"as_of":as_of,"next_cursor":next_cursor}),
    ))
}

pub(crate) async fn set_project_interest(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(slug): Path<String>,
    Json(request): Json<SetProjectInterestRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskWrite)?;
    let source = canonical_public_source(&auth, &request.source)?;
    validate_project_path_slug(&slug)?;
    if !matches!(request.interest.as_str(), "hot" | "normal" | "parked")
        || request.expected_version < 1
    {
        return Err(ApiError::invalid("interest or expected_version is invalid"));
    }
    let mut tx = state.begin_write(&auth).await?;
    let receipt_request = json!({"slug":slug,"request":&request});
    match begin_receipt(
        &mut tx,
        &auth,
        "project.set_interest",
        &request.idempotency_key,
        &receipt_request,
    )
    .await?
    {
        ReceiptStart::Replay(receipt) => {
            tx.commit().await?;
            return Ok(envelope(ResponseStatus::NoOp, receipt));
        }
        ReceiptStart::New => {}
    }
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM straylight.task_projects WHERE user_id=$1 AND slug=$2 FOR UPDATE",
    )
    .bind(auth.user_id.0)
    .bind(&slug)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("project_not_found", &slug))?;
    if current != request.expected_version {
        return Err(ApiError::conflict(
            "project_version_conflict",
            "the project changed after expected_version",
            json!({"expected_version":request.expected_version,"current_version":current}),
        ));
    }
    let version = current + 1;
    sqlx::query(
        "UPDATE straylight.task_projects SET interest_override=$3,interest_set_by=$4,interest_set_at=clock_timestamp(),version=$5,updated_at=clock_timestamp() WHERE user_id=$1 AND slug=$2",
    ).bind(auth.user_id.0).bind(&slug).bind(&request.interest).bind(&source).bind(version).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO straylight.task_audit_events(user_id,credential_id,action,details) VALUES ($1,$2,'project.set_interest',$3)")
        .bind(auth.user_id.0).bind(auth.credential_id.0).bind(json!({"slug":slug,"interest":request.interest,"source":source,"version":version})).execute(&mut *tx).await?;
    let receipt = json!({"project":{"slug":slug,"interest":request.interest,"interest_set_at":Utc::now(),"version":version},"replayed":false});
    finalize_receipt(
        &mut tx,
        &auth,
        "project.set_interest",
        &request.idempotency_key,
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(envelope(ResponseStatus::Committed, receipt))
}

fn parse_project_state_as_of(raw: Option<&str>) -> ApiResult<Option<DateTime<Utc>>> {
    let mut as_of = None;
    for (name, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        if name != "as_of" || as_of.is_some() {
            return Err(ApiError::invalid(format!(
                "unknown or repeated project state query parameter: {name}"
            )));
        }
        as_of = Some(
            DateTime::parse_from_rfc3339(&value)
                .map_err(|_| ApiError::invalid("as_of must be RFC3339"))?
                .with_timezone(&Utc),
        );
    }
    Ok(as_of)
}

/// Exact scalar projection query used by the deployed project.state handler.
/// It intentionally excludes the canonical task JSON documents.
pub const TASK_PROJECT_STATE_PROJECTION_SQL: &str = r#"
SELECT task.task_id,task.entry_id,task.entry_version,task.title,task.status,
       task.ready_at,task.soft_due,task.hard_due,task.hard_due_lead_days,
       task.cost_amount_cents,task.cost_period,task.cost_flag,task.cost_since,
       task.required_contexts,task.project_slug,task.parked,task.today_pin,
       task.triaged_at,task.created_at,task.updated_at,task.waiting_on,
       task.provenance,task.source_timestamps,
       project.interest_override,project.interest_set_at,project.last_activity_at
FROM straylight.task_index AS task
JOIN straylight.task_projects AS project
  ON project.user_id=task.user_id AND project.slug=task.project_slug
WHERE task.user_id=$1 AND task.project_slug=$2
  AND task.status IN ('open','waiting') AND task.created_at<=$3
ORDER BY task.updated_at DESC
"#;

pub(crate) async fn project_state(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(slug): Path<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskRead)?;
    validate_project_path_slug(&slug)?;
    let as_of = parse_project_state_as_of(raw.as_deref())?.unwrap_or_else(Utc::now);
    let mut tx = state.begin_read(&auth).await?;
    let project=sqlx::query("SELECT title,interest_override,interest_set_at,last_activity_at,version FROM straylight.task_projects WHERE user_id=$1 AND slug=$2 AND archived_at IS NULL")
        .bind(auth.user_id.0).bind(&slug).fetch_optional(&mut *tx).await?
        .ok_or_else(||ApiError::not_found("project_not_found",&slug))?;
    let checkpoint=sqlx::query(
        r#"
        SELECT link.checkpoint_entry_id,link.attribution,link.matched_path,link.linked_at,
               entry.current_version,version.metadata,version.created_at AS checkpoint_at
        FROM straylight.task_checkpoint_links AS link
        JOIN straylight.entries AS entry ON entry.user_id=link.user_id AND entry.id=link.checkpoint_entry_id
        JOIN straylight.entry_versions AS version ON version.user_id=entry.user_id AND version.entry_id=entry.id AND version.version=entry.current_version
        WHERE link.user_id=$1 AND link.project_slug=$2 AND version.created_at<=$3
        ORDER BY version.created_at DESC,link.checkpoint_entry_id DESC LIMIT 1
        "#,
    ).bind(auth.user_id.0).bind(&slug).bind(as_of).fetch_optional(&mut *tx).await?;
    let rows = sqlx::query(TASK_PROJECT_STATE_PROJECTION_SQL)
        .bind(auth.user_id.0)
        .bind(&slug)
        .bind(as_of)
        .fetch_all(&mut *tx)
        .await?;
    let contexts = sqlx::query_scalar::<_, String>(
        "SELECT slug FROM straylight.task_contexts WHERE user_id=$1 AND archived_at IS NULL",
    )
    .bind(auth.user_id.0)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    let settings = sqlx::query(
        "SELECT hard_lead_days,soft_window_days FROM straylight.task_settings WHERE user_id=$1",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?;
    let snapshots = rows
        .iter()
        .map(|row| snapshot_from_projection_row(row, as_of))
        .collect::<ApiResult<Vec<_>>>()?;
    let ranked = task_engine::rank_tasks(
        &snapshots,
        &EngineCandidateRequest {
            view: TaskView::Next,
            limit: 3,
            contexts_available: contexts,
            include_waiting: false,
            include_parked: false,
            as_of,
        },
        &EngineSettings {
            hard_due_lead_days: i64::from(settings.get::<i32, _>("hard_lead_days")),
            soft_due_window_days: i64::from(settings.get::<i32, _>("soft_window_days")),
        },
    );
    let details = rows
        .iter()
        .map(|row| (row.get::<Uuid, _>("task_id"), row))
        .collect::<HashMap<_, _>>();
    let next = ranked
        .items
        .iter()
        .map(|item| ranked_item_json(item, &details))
        .collect::<ApiResult<Vec<_>>>()?;
    let mut waiting=rows.iter().filter(|row|row.get::<String,_>("status")=="waiting").map(|row|{
        let waiting_on=row.get::<Option<Value>,_>("waiting_on");
        let since=waiting_on.as_ref().and_then(|value|value.get("since")).and_then(Value::as_str).and_then(parse_timestamp).unwrap_or_else(||row.get::<DateTime<Utc>,_>("updated_at"));
        let task_id=row.get::<Uuid,_>("task_id");
        (since,task_id,json!({"task_ref":task_id,"title":row.get::<String,_>("title"),"waiting_on":waiting_on,"since":since,"age_days":as_of.signed_duration_since(since).num_days().max(0)}))
    }).collect::<Vec<_>>();
    waiting.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let waiting_total = waiting.len();
    let waiting = waiting
        .into_iter()
        .take(10)
        .map(|(_, _, item)| item)
        .collect::<Vec<_>>();
    let parked_count = rows
        .iter()
        .filter(|row| row.get::<bool, _>("parked"))
        .count();
    let rollups=sqlx::query("SELECT count(*) FILTER(WHERE status='open') AS open_count,count(*) FILTER(WHERE status='waiting') AS waiting_count,count(*) FILTER(WHERE status='done' AND done_at<=$3) AS done_count,count(*) FILTER(WHERE status='dropped' AND dropped_at<=$3) AS dropped_count FROM straylight.task_index WHERE user_id=$1 AND project_slug=$2 AND created_at<=$3")
        .bind(auth.user_id.0).bind(&slug).bind(as_of).fetch_one(&mut *tx).await?;
    let explicit = project.get::<Option<String>, _>("interest_override");
    let set_at = project.get::<Option<DateTime<Utc>>, _>("interest_set_at");
    let last = project.get::<Option<DateTime<Utc>>, _>("last_activity_at");
    let interest = derive_project_interest(explicit.as_deref().zip(set_at), last, as_of);
    let checkpoint=checkpoint.map(|row|{let entry_id:Uuid=row.get("checkpoint_entry_id");let metadata:Value=row.get("metadata");json!({"entry_ref":format!("entry:{entry_id}"),"version":row.get::<i64,_>("current_version"),"attribution":row.get::<String,_>("attribution"),"matched_path":row.get::<Option<String>,_>("matched_path"),"checkpoint_at":row.get::<DateTime<Utc>,_>("checkpoint_at"),"linked_at":row.get::<DateTime<Utc>,_>("linked_at"),"state":effective_metadata(&metadata).get("checkpoint_state").cloned().unwrap_or(Value::Null)})});
    tx.commit().await?;
    Ok(envelope(
        ResponseStatus::Complete,
        json!({"project":{"slug":slug,"title":project.get::<String,_>("title"),"interest":match interest{ProjectInterest::Hot=>"hot",ProjectInterest::Normal=>"normal",ProjectInterest::Parked=>"parked"},"last_activity_at":last,"version":project.get::<i64,_>("version")},"checkpoint":checkpoint,"urgent_count":ranked.urgent_total,"next":next,"waiting":waiting,"waiting_total":waiting_total,"waiting_remaining":waiting_total.saturating_sub(10),"parked_count":parked_count,"rollups":{"open":rollups.get::<i64,_>("open_count"),"waiting":rollups.get::<i64,_>("waiting_count"),"done":rollups.get::<i64,_>("done_count"),"dropped":rollups.get::<i64,_>("dropped_count")},"as_of":as_of}),
    ))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateTaskSettingsRequest {
    pub expected_version: i64,
    pub idempotency_key: String,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub hard_lead_days: Option<i32>,
    #[serde(default)]
    pub hard_second_lead_hours: Option<i32>,
    #[serde(default)]
    pub due_day_local_time: Option<String>,
    #[serde(default)]
    pub soft_window_days: Option<i32>,
    #[serde(default)]
    pub triage_after_days: Option<i32>,
    #[serde(default)]
    pub waiting_followup_days: Option<i32>,
    #[serde(default)]
    pub quiet_hours_start: Option<String>,
    #[serde(default)]
    pub quiet_hours_end: Option<String>,
    #[serde(default)]
    pub quiet_override_enabled: Option<bool>,
    #[serde(default)]
    pub quiet_override_within_hours: Option<i32>,
    #[serde(default)]
    pub surface_defaults: Option<BTreeMap<String, Vec<String>>>,
}

fn parse_local_time(value: &str, name: &str) -> ApiResult<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M:%S"))
        .map_err(|_| ApiError::invalid(format!("{name} must be HH:MM or HH:MM:SS")))
}

async fn settings_json_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> ApiResult<Value> {
    let row = sqlx::query(
        r#"
        SELECT timezone,hard_lead_days,hard_second_lead_hours,due_day_local_time,
               soft_window_days,triage_after_days,waiting_followup_days,
               quiet_hours_start,quiet_hours_end,quiet_override_enabled,
               quiet_override_within_hours,version,updated_at
        FROM straylight.task_settings WHERE user_id=$1
        "#,
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    let surfaces = sqlx::query(
        "SELECT surface,contexts,version FROM straylight.task_surface_defaults WHERE user_id=$1 ORDER BY surface",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|surface| {
        (
            surface.get::<String, _>("surface"),
            json!({
                "contexts_available":surface.get::<Vec<String>,_>("contexts"),
                "version":surface.get::<i64,_>("version"),
            }),
        )
    })
    .collect::<BTreeMap<_, _>>();
    Ok(json!({
        "timezone":row.get::<String,_>("timezone"),
        "hard_lead_days":row.get::<i32,_>("hard_lead_days"),
        "hard_second_lead_hours":row.get::<i32,_>("hard_second_lead_hours"),
        "due_day_local_time":row.get::<NaiveTime,_>("due_day_local_time").format("%H:%M:%S").to_string(),
        "soft_window_days":row.get::<i32,_>("soft_window_days"),
        "triage_after_days":row.get::<i32,_>("triage_after_days"),
        "waiting_followup_days":row.get::<i32,_>("waiting_followup_days"),
        "quiet_hours_start":row.get::<NaiveTime,_>("quiet_hours_start").format("%H:%M:%S").to_string(),
        "quiet_hours_end":row.get::<NaiveTime,_>("quiet_hours_end").format("%H:%M:%S").to_string(),
        "quiet_override_enabled":row.get::<bool,_>("quiet_override_enabled"),
        "quiet_override_within_hours":row.get::<i32,_>("quiet_override_within_hours"),
        "surface_defaults":surfaces,
        "version":row.get::<i64,_>("version"),
        "updated_at":row.get::<DateTime<Utc>,_>("updated_at"),
    }))
}

pub(crate) async fn get_task_settings(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskRead)?;
    let mut tx = state.begin_read(&auth).await?;
    let settings = settings_json_in_tx(&mut tx, auth.user_id.0).await?;
    tx.commit().await?;
    Ok(envelope(
        ResponseStatus::Complete,
        json!({"settings":settings}),
    ))
}

pub(crate) async fn update_task_settings(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<UpdateTaskSettingsRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskWrite)?;
    if request.expected_version < 1 {
        return Err(ApiError::invalid("expected_version must be positive"));
    }
    if let Some(timezone) = &request.timezone {
        if has_forbidden_control(timezone, false) {
            return Err(ApiError::invalid(
                "timezone must not contain control characters",
            ));
        }
        timezone
            .parse::<Tz>()
            .map_err(|_| ApiError::invalid("timezone must be an IANA timezone"))?;
    }
    for (name, value, minimum, maximum) in [
        ("hard_lead_days", request.hard_lead_days, 1, 90),
        (
            "hard_second_lead_hours",
            request.hard_second_lead_hours,
            1,
            2160,
        ),
        ("soft_window_days", request.soft_window_days, 1, 90),
        ("triage_after_days", request.triage_after_days, 1, 3650),
        (
            "waiting_followup_days",
            request.waiting_followup_days,
            1,
            3650,
        ),
        (
            "quiet_override_within_hours",
            request.quiet_override_within_hours,
            1,
            168,
        ),
    ] {
        if value.is_some_and(|value| value < minimum || value > maximum) {
            return Err(ApiError::invalid(format!("{name} is out of range")));
        }
    }
    let due_time = request
        .due_day_local_time
        .as_deref()
        .map(|value| parse_local_time(value, "due_day_local_time"))
        .transpose()?;
    let quiet_start = request
        .quiet_hours_start
        .as_deref()
        .map(|value| parse_local_time(value, "quiet_hours_start"))
        .transpose()?;
    let quiet_end = request
        .quiet_hours_end
        .as_deref()
        .map(|value| parse_local_time(value, "quiet_hours_end"))
        .transpose()?;
    if request.surface_defaults.as_ref().is_some_and(|surfaces| {
        surfaces.len() > 20
            || surfaces
                .keys()
                .any(|surface| has_forbidden_control(surface, false))
            || surfaces.values().any(|contexts| {
                contexts.len() > 20
                    || contexts
                        .iter()
                        .any(|context| has_forbidden_control(context, false))
            })
    }) {
        return Err(ApiError::invalid(
            "surface_defaults accepts at most 20 surfaces and 20 contexts per surface",
        ));
    }
    let mut tx = state.begin_write(&auth).await?;
    match begin_receipt(
        &mut tx,
        &auth,
        "task.settings.update",
        &request.idempotency_key,
        &request,
    )
    .await?
    {
        ReceiptStart::Replay(receipt) => {
            tx.commit().await?;
            return Ok(envelope(ResponseStatus::NoOp, receipt));
        }
        ReceiptStart::New => {}
    }
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM straylight.task_settings WHERE user_id=$1 FOR UPDATE",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?;
    if current != request.expected_version {
        return Err(ApiError::conflict(
            "task_settings_version_conflict",
            "task settings changed after expected_version",
            json!({"expected_version":request.expected_version,"current_version":current}),
        ));
    }
    let version = current + 1;
    sqlx::query(
        r#"
        UPDATE straylight.task_settings SET
          timezone=COALESCE($2,timezone),hard_lead_days=COALESCE($3,hard_lead_days),
          hard_second_lead_hours=COALESCE($4,hard_second_lead_hours),
          due_day_local_time=COALESCE($5,due_day_local_time),soft_window_days=COALESCE($6,soft_window_days),
          triage_after_days=COALESCE($7,triage_after_days),waiting_followup_days=COALESCE($8,waiting_followup_days),
          quiet_hours_start=COALESCE($9,quiet_hours_start),quiet_hours_end=COALESCE($10,quiet_hours_end),
          quiet_override_enabled=COALESCE($11,quiet_override_enabled),
          quiet_override_within_hours=COALESCE($12,quiet_override_within_hours),
          version=$13,updated_at=clock_timestamp()
        WHERE user_id=$1
        "#,
    ).bind(auth.user_id.0).bind(&request.timezone).bind(request.hard_lead_days)
      .bind(request.hard_second_lead_hours).bind(due_time).bind(request.soft_window_days)
      .bind(request.triage_after_days).bind(request.waiting_followup_days).bind(quiet_start).bind(quiet_end)
      .bind(request.quiet_override_enabled).bind(request.quiet_override_within_hours).bind(version)
      .execute(&mut *tx).await?;
    if let Some(surfaces) = &request.surface_defaults {
        for (surface, raw_contexts) in surfaces {
            let valid_surface = !surface.is_empty()
                && surface.len() <= 64
                && surface
                    .chars()
                    .next()
                    .is_some_and(|value| value.is_ascii_lowercase())
                && surface.chars().all(|value| {
                    value.is_ascii_lowercase() || value.is_ascii_digit() || "._-".contains(value)
                });
            if !valid_surface {
                return Err(ApiError::invalid(
                    "surface_defaults contains an invalid surface",
                ));
            }
            let mut contexts = raw_contexts
                .iter()
                .map(|value| normalize_slug(value))
                .collect::<ApiResult<Vec<_>>>()?;
            contexts.sort();
            contexts.dedup();
            let active=sqlx::query_scalar::<_,String>("SELECT slug FROM straylight.task_contexts WHERE user_id=$1 AND slug=ANY($2) AND archived_at IS NULL")
                .bind(auth.user_id.0).bind(&contexts).fetch_all(&mut *tx).await?.into_iter().collect::<BTreeSet<_>>();
            if contexts.iter().any(|context| !active.contains(context)) {
                return Err(ApiError::invalid(
                    "surface_defaults contains an unknown or archived context",
                ));
            }
            sqlx::query("INSERT INTO straylight.task_surface_defaults(user_id,surface,contexts) VALUES ($1,$2,$3) ON CONFLICT(user_id,surface) DO UPDATE SET contexts=EXCLUDED.contexts,version=task_surface_defaults.version+1,updated_at=clock_timestamp()")
                .bind(auth.user_id.0).bind(surface).bind(contexts).execute(&mut *tx).await?;
        }
    }
    sqlx::query("INSERT INTO straylight.task_audit_events(user_id,credential_id,action,details) VALUES ($1,$2,'task.settings.update',$3)")
        .bind(auth.user_id.0).bind(auth.credential_id.0).bind(json!({"version":version})).execute(&mut *tx).await?;
    let settings = settings_json_in_tx(&mut tx, auth.user_id.0).await?;
    let receipt = json!({"settings":settings,"replayed":false});
    finalize_receipt(
        &mut tx,
        &auth,
        "task.settings.update",
        &request.idempotency_key,
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(envelope(ResponseStatus::Committed, receipt))
}

#[derive(Clone, Debug, Default, Serialize)]
#[doc(hidden)]
pub struct TodoistApplyReport {
    pub(crate) projects_seen: usize,
    pub(crate) items_seen: usize,
    pub(crate) created: usize,
    pub(crate) updated: usize,
    pub(crate) completed: usize,
    pub(crate) dropped: usize,
    pub(crate) unchanged: usize,
}

fn todoist_refresh_allowed(raw: Option<&Value>) -> bool {
    match raw {
        None | Some(Value::Null) => true,
        Some(raw) => raw
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|source| source == "todoist"),
    }
}

fn todoist_should_repoint(current_occurrence: Option<&str>, incoming_occurrence: &str) -> bool {
    let _ = incoming_occurrence;
    !current_occurrence.is_some_and(|current| current.starts_with("review:"))
}

fn todoist_task_url(external_id: &str) -> String {
    format!("https://app.todoist.com/app/task/{external_id}")
}

fn refresh_todoist_field(
    metadata: &mut Value,
    field: &str,
    value: Value,
    now: DateTime<Utc>,
    note: Option<&str>,
) -> ApiResult<bool> {
    let current = effective_metadata(metadata)
        .get("task")
        .and_then(Value::as_object)
        .and_then(|task| task.get(field));
    if !todoist_refresh_allowed(current) {
        return Ok(false);
    }
    let unchanged = current.is_some_and(|cell| {
        cell.get("value") == Some(&value)
            && cell.get("source").and_then(Value::as_str) == Some("todoist")
            && cell.get("note").and_then(Value::as_str) == note
    });
    if unchanged {
        return Ok(false);
    }
    if current.is_some_and(|cell| {
        cell.get("value") == Some(&value)
            && cell.get("source").and_then(Value::as_str) == Some("todoist")
    }) {
        let task = direct_task_object_mut(metadata)?;
        let cell = task
            .get_mut(field)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| ApiError::invalid("Todoist task field is not a sourced cell"))?;
        cell.insert("set_at".to_owned(), json!(now));
        cell.insert(
            "note".to_owned(),
            note.map_or(Value::Null, |value| json!(value)),
        );
        return Ok(true);
    }
    apply_sourced_field(metadata, field, value, "todoist", now, note, false)?;
    Ok(true)
}

fn todoist_recurrence_from_value(value: &Value) -> ApiResult<MappedRecurrence> {
    let recurrence = value
        .as_object()
        .ok_or_else(|| ApiError::invalid("Todoist recurrence must be an object"))?;
    let required = |name: &str| {
        recurrence
            .get(name)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| ApiError::invalid(format!("Todoist recurrence requires {name}")))
    };
    if required("recurrence_source")? != "todoist" {
        return Err(ApiError::invalid(
            "Todoist recurrence_source must be todoist",
        ));
    }
    Ok(MappedRecurrence {
        recurrence_source: "todoist",
        original: required("original")?,
        lang: required("lang")?,
        series_id: required("series_id")?,
        due: required("due")?,
        timezone: recurrence
            .get("timezone")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        rrule: recurrence
            .get("rrule")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        needs_review: recurrence
            .get("needs_review")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn validate_todoist_project(project_id: &str, name: &str, deleted: bool) -> ApiResult<()> {
    if project_id.is_empty()
        || project_id.len() > 512
        || project_id.chars().any(char::is_control)
        || (!deleted
            && (name.trim().is_empty()
                || name.chars().count() > 200
                || has_forbidden_control(name, false)))
        || (deleted && (name.chars().count() > 200 || has_forbidden_control(name, false)))
    {
        return Err(ApiError::invalid("Todoist project payload is invalid"));
    }
    Ok(())
}

async fn cache_todoist_project_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    project_id: &str,
    name: &str,
    deleted: bool,
) -> ApiResult<()> {
    validate_todoist_project(project_id, name, deleted)?;
    if deleted && name.trim().is_empty() {
        sqlx::query(
            r#"
            UPDATE straylight.task_todoist_projects
            SET is_deleted=true,updated_at=clock_timestamp()
            WHERE user_id=$1 AND external_id=$2
            "#,
        )
        .bind(user_id)
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
        return Ok(());
    }
    sqlx::query(
        r#"
        INSERT INTO straylight.task_todoist_projects(
          user_id,external_id,name,is_deleted
        ) VALUES($1,$2,$3,$4)
        ON CONFLICT(user_id,external_id) DO UPDATE SET
          name=EXCLUDED.name,is_deleted=EXCLUDED.is_deleted,
          updated_at=clock_timestamp()
        "#,
    )
    .bind(user_id)
    .bind(project_id)
    .bind(name.trim())
    .bind(deleted)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn todoist_project_slug_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    external_project_id: &str,
) -> ApiResult<String> {
    let name = sqlx::query_scalar::<_, String>(
        "SELECT name FROM straylight.task_todoist_projects WHERE user_id=$1 AND external_id=$2",
    )
    .bind(user_id)
    .bind(external_project_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(name) = name else {
        return Ok("todoist-inbox".to_owned());
    };
    let normalized = normalize_slug(&name).ok();
    let matched = sqlx::query_scalar::<_, String>(
        r#"
        SELECT project_slug FROM (
          SELECT project.slug AS project_slug,0 AS priority
          FROM straylight.task_projects AS project
          WHERE project.user_id=$1 AND project.archived_at IS NULL
            AND ($2::text IS NOT NULL AND project.slug=$2)
          UNION ALL
          SELECT project.slug,1
          FROM straylight.task_projects AS project
          WHERE project.user_id=$1 AND project.archived_at IS NULL
            AND lower(project.title)=lower($3)
          UNION ALL
          SELECT alias.project_slug,2
          FROM straylight.task_project_aliases AS alias
          JOIN straylight.task_projects AS project
            ON project.user_id=alias.user_id AND project.slug=alias.project_slug
          WHERE alias.user_id=$1 AND lower(alias.alias)=lower($3)
            AND project.archived_at IS NULL
        ) AS matches
        ORDER BY priority,project_slug
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(normalized)
    .bind(name.trim())
    .fetch_optional(&mut **tx)
    .await?;
    Ok(matched.unwrap_or_else(|| "todoist-inbox".to_owned()))
}

async fn remap_todoist_project_tasks_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    external_project_id: &str,
    now: DateTime<Utc>,
) -> ApiResult<usize> {
    let project_slug = todoist_project_slug_in_tx(tx, user_id, external_project_id).await?;
    let task_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT task_id
        FROM straylight.task_external_refs
        WHERE user_id=$1 AND system='todoist'
          AND metadata->>'project_id'=$2
        ORDER BY task_id
        "#,
    )
    .bind(user_id)
    .bind(external_project_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut changed = 0;
    for task_id in task_ids {
        let Some(row) = fetch_task_row(tx, user_id, task_id, true).await? else {
            continue;
        };
        let mut metadata: Value = row.get("metadata");
        if refresh_todoist_field(&mut metadata, "project", json!(project_slug), now, None)? {
            persist_loaded_todoist_task_in_tx(tx, user_id, producer_credential_id, &row, metadata)
                .await?;
            changed += 1;
        }
    }
    Ok(changed)
}

async fn todoist_context_slug_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    label: &str,
) -> ApiResult<Result<String, Vec<ContextSuggestion>>> {
    if let Some(canonical) = resolve_context_in_tx(tx, user_id, label).await? {
        return Ok(Ok(canonical));
    }
    if label.trim().is_empty() || label.chars().count() > 120 || has_forbidden_control(label, false)
    {
        return Err(ApiError::invalid("Todoist label is invalid"));
    }
    let suggestions = context_suggestions_in_tx(tx, user_id, label).await?;
    if !suggestions.is_empty() {
        return Ok(Err(suggestions));
    }
    let slug = create_context_in_tx(
        tx,
        user_id,
        producer_credential_id,
        label,
        Some(label.trim()),
        Some("Imported from a Todoist label."),
        "todoist",
        false,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.task_context_aliases(
          user_id,alias,context_slug,reason
        ) VALUES($1,$2,$3,'todoist')
        ON CONFLICT(user_id,alias) DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(label.trim())
    .bind(&slug)
    .execute(&mut **tx)
    .await?;
    Ok(Ok(slug))
}

async fn todoist_contexts_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    labels: &[String],
) -> ApiResult<(Vec<String>, Vec<Value>)> {
    let mut contexts = Vec::with_capacity(labels.len());
    let mut review = Vec::new();
    for label in labels {
        match todoist_context_slug_in_tx(tx, user_id, producer_credential_id, label).await? {
            Ok(context) => contexts.push(context),
            Err(suggestions) => review.push(json!({
                "requested":label,
                "suggested_existing":suggestions.into_iter().map(|suggestion|json!({
                    "slug":suggestion.slug,
                    "reason":suggestion.reason,
                })).collect::<Vec<_>>(),
            })),
        }
    }
    contexts.sort();
    contexts.dedup();
    Ok((contexts, review))
}

fn todoist_task_content(task: &Map<String, Value>) -> ApiResult<String> {
    let title = task
        .get("title")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("Todoist task requires a title"))?;
    let notes = string_value(task, "notes")?.unwrap_or_default();
    Ok(if notes.is_empty() {
        format!("# {title}\n")
    } else {
        format!("# {title}\n\n{notes}\n")
    })
}

async fn create_todoist_task_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    mapped: &MappedTodoistItem,
    project_slug: &str,
    contexts: &[String],
    context_review: &[Value],
    now: DateTime<Utc>,
) -> ApiResult<(Uuid, Uuid)> {
    if mapped.title.trim().is_empty()
        || mapped.title.len() > 500
        || has_forbidden_control(&mapped.title, false)
        || mapped.notes.len() > 20_000
        || has_forbidden_control(&mapped.notes, true)
    {
        return Err(ApiError::invalid(
            "Todoist task title or description exceeds the canonical task boundary",
        ));
    }
    let task_id = Uuid::now_v7();
    let mut task = Map::new();
    task.insert("id".to_owned(), json!(task_id));
    task.insert("title".to_owned(), json!(mapped.title.trim()));
    task.insert(
        "provenance".to_owned(),
        json!({
            "created_at":now,
            "created_by":"todoist",
            "captured_by":"todoist",
            "captured_from":format!("todoist:item:{}",mapped.external_id),
            "credential_id":producer_credential_id,
            "title_source":"todoist",
            "title_set_at":now,
        }),
    );
    task.insert(
        "status".to_owned(),
        json!({"value":"open","source":"todoist","set_at":now}),
    );
    task.insert(
        "notes".to_owned(),
        json!({"value":mapped.notes,"source":"todoist","set_at":now}),
    );
    task.insert(
        "project".to_owned(),
        json!({"value":project_slug,"source":"todoist","set_at":now}),
    );
    task.insert(
        "required_contexts".to_owned(),
        json!({"value":contexts,"source":"todoist","set_at":now}),
    );
    task.insert(
        "soft_due".to_owned(),
        json!({"value":mapped.soft_due,"source":"todoist","set_at":now}),
    );
    task.insert(
        "hard_due".to_owned(),
        json!({
            "value":mapped.hard_due,
            "source":"todoist",
            "set_at":now,
            "note":mapped.hard_due_note,
        }),
    );
    task.insert(
        "triaged_at".to_owned(),
        json!({"value":null,"source":"todoist","set_at":now}),
    );
    task.insert(
        "recurrence".to_owned(),
        json!({"value":mapped.recurrence,"source":"todoist","set_at":now}),
    );
    task.insert(
        "external_refs".to_owned(),
        json!([{
            "system":"todoist",
            "id":mapped.external_id,
            "url":todoist_task_url(&mapped.external_id),
            "project_id":mapped.project_id,
            "series_id":mapped.recurrence.as_ref().map(|value|value.series_id.as_str()),
            "occurrence_key":mapped.occurrence_key,
            "current":true,
            "last_seen_at":now,
        }]),
    );
    if !context_review.is_empty() {
        task.insert(
            "todoist_context_suggestions".to_owned(),
            json!(context_review),
        );
    }
    match mapped.terminal {
        TodoistTerminal::Open => {}
        TodoistTerminal::Completed => {
            let completed_at = mapped.completed_at.unwrap_or(now);
            task.insert(
                "status".to_owned(),
                json!({"value":"done","source":"todoist","set_at":completed_at}),
            );
            task.insert(
                "completed_via".to_owned(),
                json!({"value":"todoist","source":"todoist","set_at":completed_at}),
            );
            task.insert("done_at".to_owned(), json!(completed_at));
        }
        TodoistTerminal::Deleted => {
            return Err(ApiError::invalid(
                "an unknown Todoist tombstone cannot create a task",
            ));
        }
    }
    let content = todoist_task_content(&task)?;
    let metadata = json!({"kind":"task","schema":TASK_SCHEMA,"task":task});
    let path = format!("{TASK_ENTRY_PREFIX}{task_id}.md");
    let prepared = simple_core::prepare_task_markdown_for_update(path, content, metadata, 0)?;
    let result =
        simple_core::upsert_markdown_in_tx(tx, user_id, Some(producer_credential_id), prepared)
            .await?;
    Ok((task_id, result.entry_id))
}

fn set_todoist_terminal(
    metadata: &mut Value,
    terminal: TodoistTerminal,
    now: DateTime<Utc>,
) -> ApiResult<bool> {
    let task = direct_task_object_mut(metadata)?;
    let current_status = string_value(task, "status")?.unwrap_or_else(|| "open".to_owned());
    let status_source = task
        .get("status")
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str);
    match terminal {
        TodoistTerminal::Open => Ok(false),
        TodoistTerminal::Completed if current_status == "done" => Ok(false),
        TodoistTerminal::Completed if status_source != Some("todoist") => Ok(false),
        TodoistTerminal::Completed => {
            task.insert(
                "status".to_owned(),
                json!({"value":"done","source":"todoist","set_at":now}),
            );
            task.insert(
                "completed_via".to_owned(),
                json!({"value":"todoist","source":"todoist","set_at":now}),
            );
            task.insert("done_at".to_owned(), json!(now));
            if todoist_refresh_allowed(task.get("dropped_reason")) {
                task.remove("dropped_at");
                task.remove("dropped_reason");
            }
            Ok(true)
        }
        TodoistTerminal::Deleted if current_status == "done" || current_status == "dropped" => {
            Ok(false)
        }
        TodoistTerminal::Deleted if status_source != Some("todoist") => Ok(false),
        TodoistTerminal::Deleted => {
            task.insert(
                "status".to_owned(),
                json!({"value":"dropped","source":"todoist","set_at":now}),
            );
            task.insert(
                "dropped_reason".to_owned(),
                if todoist_refresh_allowed(task.get("dropped_reason")) {
                    json!({"value":"todoist_deleted","source":"todoist","set_at":now})
                } else {
                    task.get("dropped_reason").cloned().unwrap_or(Value::Null)
                },
            );
            task.insert("dropped_at".to_owned(), json!(now));
            Ok(true)
        }
    }
}

async fn persist_loaded_todoist_task_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    row: &sqlx::postgres::PgRow,
    metadata: Value,
) -> ApiResult<Uuid> {
    let path: String = row.get("path");
    let current_version: i64 = row.get("current_version");
    let task = effective_metadata(&metadata)
        .get("task")
        .and_then(Value::as_object)
        .ok_or_else(|| ApiError::invalid("Todoist task metadata is invalid"))?;
    let content = todoist_task_content(task)?;
    let prepared =
        simple_core::prepare_task_markdown_for_update(path, content, metadata, current_version)?;
    let result =
        simple_core::upsert_markdown_in_tx(tx, user_id, Some(producer_credential_id), prepared)
            .await?;
    Ok(result.entry_id)
}

async fn refresh_existing_todoist_task_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    task_id: Uuid,
    mapped: &MappedTodoistItem,
    project_slug: Option<&str>,
    contexts: Option<&[String]>,
    context_review: Option<&[Value]>,
    now: DateTime<Utc>,
) -> ApiResult<(Uuid, bool, bool)> {
    let row = fetch_task_row(tx, user_id, task_id, true)
        .await?
        .ok_or_else(|| ApiError::not_found("task_not_found", &task_id.to_string()))?;
    let mut metadata: Value = row.get("metadata");
    let original = metadata.clone();
    if mapped.terminal != TodoistTerminal::Deleted {
        if mapped.title.trim().is_empty()
            || mapped.title.len() > 500
            || mapped.notes.len() > 20_000
            || has_forbidden_control(&mapped.title, false)
            || has_forbidden_control(&mapped.notes, true)
        {
            return Err(ApiError::invalid(
                "Todoist task title or description exceeds the canonical task boundary",
            ));
        }
        let task = direct_task_object_mut(&mut metadata)?;
        let title_source = task
            .get("provenance")
            .and_then(|value| value.get("title_source"))
            .and_then(Value::as_str);
        if title_source == Some("todoist")
            && task.get("title").and_then(Value::as_str) != Some(mapped.title.trim())
        {
            task.insert("title".to_owned(), json!(mapped.title.trim()));
            let provenance = task
                .get_mut("provenance")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| ApiError::invalid("task provenance must be an object"))?;
            provenance.insert("title_set_at".to_owned(), json!(now));
        }
        if let Some(external_refs) = task.get_mut("external_refs").and_then(Value::as_array_mut) {
            for external in external_refs {
                if external.get("system").and_then(Value::as_str) == Some("todoist")
                    && external.get("id").and_then(Value::as_str)
                        == Some(mapped.external_id.as_str())
                {
                    let external = external.as_object_mut().ok_or_else(|| {
                        ApiError::invalid("Todoist external ref must be an object")
                    })?;
                    external.insert("project_id".to_owned(), json!(mapped.project_id));
                    external.insert(
                        "url".to_owned(),
                        json!(todoist_task_url(&mapped.external_id)),
                    );
                }
            }
        }
        refresh_todoist_field(&mut metadata, "notes", json!(mapped.notes), now, None)?;
        if let Some(project_slug) = project_slug {
            refresh_todoist_field(&mut metadata, "project", json!(project_slug), now, None)?;
        }
        if let Some(contexts) = contexts {
            refresh_todoist_field(
                &mut metadata,
                "required_contexts",
                json!(contexts),
                now,
                None,
            )?;
        }
        if let Some(context_review) = context_review {
            let task = direct_task_object_mut(&mut metadata)?;
            if context_review.is_empty() {
                task.remove("todoist_context_suggestions");
            } else {
                task.insert(
                    "todoist_context_suggestions".to_owned(),
                    json!(context_review),
                );
                refresh_todoist_field(
                    &mut metadata,
                    "triaged_at",
                    Value::Null,
                    now,
                    Some("todoist_context_confirmation_required"),
                )?;
            }
        }
        refresh_todoist_field(&mut metadata, "soft_due", json!(mapped.soft_due), now, None)?;
        refresh_todoist_field(
            &mut metadata,
            "hard_due",
            json!(mapped.hard_due),
            now,
            mapped.hard_due_note,
        )?;
        refresh_todoist_field(
            &mut metadata,
            "recurrence",
            json!(mapped.recurrence),
            now,
            None,
        )?;
    }
    let terminal_changed = set_todoist_terminal(
        &mut metadata,
        mapped.terminal,
        mapped.completed_at.unwrap_or(now),
    )?;
    let changed = metadata != original;
    let entry_id: Uuid = if changed {
        persist_loaded_todoist_task_in_tx(tx, user_id, producer_credential_id, &row, metadata)
            .await?
    } else {
        row.get("entry_id")
    };
    Ok((entry_id, changed, terminal_changed))
}

async fn complete_existing_todoist_task_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    task_id: Uuid,
    now: DateTime<Utc>,
) -> ApiResult<bool> {
    let row = fetch_task_row(tx, user_id, task_id, true)
        .await?
        .ok_or_else(|| ApiError::not_found("task_not_found", &task_id.to_string()))?;
    let mut metadata: Value = row.get("metadata");
    if !set_todoist_terminal(&mut metadata, TodoistTerminal::Completed, now)? {
        return Ok(false);
    }
    persist_loaded_todoist_task_in_tx(tx, user_id, producer_credential_id, &row, metadata).await?;
    Ok(true)
}

async fn todoist_recurrence_is_authoritative_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    task_id: Uuid,
) -> ApiResult<bool> {
    let task = sqlx::query_scalar::<_, Value>(
        "SELECT task FROM straylight.task_index WHERE user_id=$1 AND task_id=$2",
    )
    .bind(user_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(task) = task else {
        return Ok(false);
    };
    let recurrence = task.get("recurrence");
    Ok(recurrence
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some("todoist"))
}

#[allow(clippy::too_many_arguments)]
async fn set_todoist_canonical_identity_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    task_id: Uuid,
    external_id: &str,
    series_id: Option<&str>,
    occurrence_key: Option<&str>,
    current: bool,
    now: DateTime<Utc>,
) -> ApiResult<Uuid> {
    let row = fetch_task_row(tx, user_id, task_id, true)
        .await?
        .ok_or_else(|| ApiError::not_found("task_not_found", &task_id.to_string()))?;
    let mut metadata: Value = row.get("metadata");
    let original = metadata.clone();
    let task = direct_task_object_mut(&mut metadata)?;
    let refs = task
        .entry("external_refs")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| ApiError::invalid("task external_refs must be an array"))?;
    let mut found = false;
    for external in refs.iter_mut() {
        if external.get("system").and_then(Value::as_str) == Some("todoist")
            && external.get("id").and_then(Value::as_str) == Some(external_id)
        {
            let external = external
                .as_object_mut()
                .ok_or_else(|| ApiError::invalid("Todoist external ref must be an object"))?;
            external.insert("series_id".to_owned(), json!(series_id));
            external.insert("occurrence_key".to_owned(), json!(occurrence_key));
            external.insert("current".to_owned(), json!(current));
            found = true;
        }
    }
    if !found {
        refs.push(json!({
            "system":"todoist",
            "id":external_id,
            "url":todoist_task_url(external_id),
            "series_id":series_id,
            "occurrence_key":occurrence_key,
            "current":current,
            "last_seen_at":now,
        }));
    }
    if metadata == original {
        return Ok(row.get("entry_id"));
    }
    persist_loaded_todoist_task_in_tx(tx, user_id, producer_credential_id, &row, metadata).await
}

#[derive(Clone, Debug)]
struct TodoistExternalRef {
    task_id: Uuid,
    series_id: Option<String>,
    occurrence_key: Option<String>,
}

async fn todoist_external_ref_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    external_id: &str,
) -> ApiResult<Option<TodoistExternalRef>> {
    Ok(sqlx::query(
        r#"
        SELECT task_id,series_id,occurrence_key
        FROM straylight.task_external_refs
        WHERE user_id=$1 AND system='todoist' AND external_id=$2
        "#,
    )
    .bind(user_id)
    .bind(external_id)
    .fetch_optional(&mut **tx)
    .await?
    .map(|row| TodoistExternalRef {
        task_id: row.get("task_id"),
        series_id: row.get("series_id"),
        occurrence_key: row.get("occurrence_key"),
    }))
}

async fn todoist_occurrence_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    series_id: &str,
    occurrence_key: &str,
) -> ApiResult<Option<(Uuid, Uuid)>> {
    Ok(sqlx::query(
        r#"
        SELECT task_id,entry_id
        FROM straylight.task_todoist_occurrences
        WHERE user_id=$1 AND series_id=$2 AND occurrence_key=$3
        "#,
    )
    .bind(user_id)
    .bind(series_id)
    .bind(occurrence_key)
    .fetch_optional(&mut **tx)
    .await?
    .map(|row| (row.get("task_id"), row.get("entry_id"))))
}

async fn record_todoist_occurrence_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    series_id: &str,
    occurrence_key: &str,
    task_id: Uuid,
    entry_id: Uuid,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.task_todoist_occurrences(
          user_id,series_id,occurrence_key,task_id,entry_id
        ) VALUES($1,$2,$3,$4,$5)
        ON CONFLICT(user_id,series_id,occurrence_key) DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(series_id)
    .bind(occurrence_key)
    .bind(task_id)
    .bind(entry_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn record_todoist_external_ref_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    external_id: &str,
    task_id: Uuid,
    entry_id: Uuid,
    series_id: Option<&str>,
    occurrence_key: Option<&str>,
    repoint: bool,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.task_external_refs(
          user_id,system,external_id,task_id,entry_id,series_id,
          occurrence_key,metadata
        ) VALUES($1,'todoist',$2,$3,$4,$5,$6,$7)
        ON CONFLICT(user_id,system,external_id) DO UPDATE SET
          task_id=CASE WHEN $8 THEN EXCLUDED.task_id ELSE task_external_refs.task_id END,
          entry_id=CASE WHEN $8 THEN EXCLUDED.entry_id ELSE task_external_refs.entry_id END,
          series_id=CASE WHEN $8 THEN EXCLUDED.series_id ELSE task_external_refs.series_id END,
          occurrence_key=CASE WHEN $8 THEN EXCLUDED.occurrence_key ELSE task_external_refs.occurrence_key END,
          metadata=CASE WHEN $8 THEN task_external_refs.metadata || EXCLUDED.metadata ELSE task_external_refs.metadata END,
          last_seen_at=clock_timestamp()
        "#,
    )
    .bind(user_id)
    .bind(external_id)
    .bind(task_id)
    .bind(entry_id)
    .bind(series_id)
    .bind(occurrence_key)
    .bind(json!({"url":todoist_task_url(external_id)}))
    .bind(repoint)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[doc(hidden)]
pub async fn materialize_next_todoist_occurrence_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    completed_task_id: Uuid,
    now: DateTime<Utc>,
) -> ApiResult<Option<Uuid>> {
    let occurrence = sqlx::query(
        r#"
        SELECT series_id,occurrence_key
        FROM straylight.task_todoist_occurrences
        WHERE user_id=$1 AND task_id=$2
        "#,
    )
    .bind(user_id)
    .bind(completed_task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(occurrence) = occurrence else {
        return Ok(None);
    };
    let series_id: String = occurrence.get("series_id");
    let current_key: String = occurrence.get("occurrence_key");
    let row = fetch_task_row(tx, user_id, completed_task_id, true)
        .await?
        .ok_or_else(|| ApiError::not_found("task_not_found", &completed_task_id.to_string()))?;
    if row.get::<String, _>("status") != "done" {
        return Ok(None);
    }
    let metadata: Value = row.get("metadata");
    let task = effective_metadata(&metadata)
        .get("task")
        .and_then(Value::as_object)
        .ok_or_else(|| ApiError::invalid("Todoist task metadata is invalid"))?;
    if task
        .get("recurrence")
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        != Some("todoist")
    {
        return Ok(None);
    }
    let recurrence_value = owned_value(task, "recurrence")?
        .filter(|value| !value.is_null())
        .ok_or_else(|| ApiError::invalid("Todoist occurrence requires recurrence metadata"))?;
    let recurrence = todoist_recurrence_from_value(&recurrence_value)?;
    if recurrence.series_id != series_id {
        return Err(ApiError::conflict(
            "todoist_series_conflict",
            "Todoist occurrence ledger and canonical recurrence disagree",
            json!({"task_ref":completed_task_id}),
        ));
    }
    let external_project_id = task
        .get("external_refs")
        .and_then(Value::as_array)
        .and_then(|refs| {
            refs.iter().find(|external| {
                external.get("system").and_then(Value::as_str) == Some("todoist")
                    && external.get("series_id").and_then(Value::as_str) == Some(series_id.as_str())
            })
        })
        .and_then(|external| external.get("project_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let timezone_name = sqlx::query_scalar::<_, String>(
        "SELECT timezone FROM straylight.task_settings WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    let timezone = timezone_name
        .parse::<Tz>()
        .map_err(|_| ApiError::Internal("stored task timezone is invalid".to_owned()))?;
    let next = next_todoist_occurrence(&recurrence, timezone)?;
    let (next_key, next_soft_due, next_due, needs_review) = match next {
        Some(next) => (
            next.occurrence_key,
            Some(next.soft_due),
            Some(next.due_instant),
            false,
        ),
        None => (format!("review:{completed_task_id}"), None, None, true),
    };
    if let Some((task_id, entry_id)) =
        todoist_occurrence_in_tx(tx, user_id, &series_id, &next_key).await?
    {
        if task_id != completed_task_id {
            set_todoist_canonical_identity_in_tx(
                tx,
                user_id,
                producer_credential_id,
                completed_task_id,
                &series_id,
                Some(&series_id),
                Some(&current_key),
                false,
                now,
            )
            .await?;
            set_todoist_canonical_identity_in_tx(
                tx,
                user_id,
                producer_credential_id,
                task_id,
                &series_id,
                Some(&series_id),
                Some(&next_key),
                true,
                now,
            )
            .await?;
            record_todoist_external_ref_in_tx(
                tx,
                user_id,
                &series_id,
                task_id,
                entry_id,
                Some(&series_id),
                Some(&next_key),
                true,
            )
            .await?;
        }
        return Ok(Some(task_id));
    }

    let mut next_metadata = metadata.clone();
    let next_task_id = Uuid::now_v7();
    let next_task = direct_task_object_mut(&mut next_metadata)?;
    next_task.insert("id".to_owned(), json!(next_task_id));
    next_task.insert(
        "status".to_owned(),
        json!({"value":"open","source":"todoist","set_at":now}),
    );
    next_task.remove("done_at");
    next_task.remove("dropped_at");
    next_task.remove("completed_via");
    next_task.remove("dropped_reason");
    next_task.remove("today_pin");
    let provenance = next_task
        .entry("provenance")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| ApiError::invalid("task provenance must be an object"))?;
    provenance.insert("created_at".to_owned(), json!(now));
    provenance.insert("created_by".to_owned(), json!("todoist"));
    provenance.insert(
        "captured_from".to_owned(),
        json!(format!("todoist:series:{series_id}")),
    );
    let mut next_recurrence = recurrence_value;
    let recurrence_object = next_recurrence
        .as_object_mut()
        .ok_or_else(|| ApiError::invalid("Todoist recurrence must be an object"))?;
    if next_due.is_some() {
        // `occurrence_key` preserves Todoist's representation class (date,
        // floating local time, or fixed UTC). `due_instant` is only for local
        // hard-deadline calculations and must not turn a floating series into
        // a fixed-zone series after one roll.
        recurrence_object.insert("due".to_owned(), json!(next_key.clone()));
    }
    recurrence_object.insert("needs_review".to_owned(), json!(needs_review));
    next_task.insert(
        "recurrence".to_owned(),
        json!({"value":next_recurrence,"source":"todoist","set_at":now}),
    );
    next_task.insert(
        "soft_due".to_owned(),
        json!({"value":next_soft_due,"source":"todoist","set_at":now}),
    );
    let previous_hard_note = task
        .get("hard_due")
        .and_then(|value| value.get("note"))
        .and_then(Value::as_str);
    let next_hard_due = match previous_hard_note {
        Some("todoist_priority_p1" | "todoist_hard_label") => next_due,
        _ => None,
    };
    next_task.insert(
        "hard_due".to_owned(),
        json!({
            "value":next_hard_due,
            "source":"todoist",
            "set_at":now,
            "note":previous_hard_note.filter(|note|*note!="todoist_deadline"),
        }),
    );
    if needs_review {
        next_task.insert(
            "triaged_at".to_owned(),
            json!({"value":null,"source":"todoist","set_at":now,"note":"todoist_recurrence_review"}),
        );
    }
    next_task.insert(
        "external_refs".to_owned(),
        json!([{
            "system":"todoist",
            "id":series_id,
            "url":todoist_task_url(&series_id),
            "project_id":external_project_id,
            "series_id":series_id,
            "occurrence_key":next_key,
            "current":true,
            "last_seen_at":now,
        }]),
    );
    set_todoist_canonical_identity_in_tx(
        tx,
        user_id,
        producer_credential_id,
        completed_task_id,
        &series_id,
        Some(&series_id),
        Some(&current_key),
        false,
        now,
    )
    .await?;
    let content = todoist_task_content(next_task)?;
    let path = format!("{TASK_ENTRY_PREFIX}{next_task_id}.md");
    let prepared = simple_core::prepare_task_markdown_for_update(path, content, next_metadata, 0)?;
    let result =
        simple_core::upsert_markdown_in_tx(tx, user_id, Some(producer_credential_id), prepared)
            .await?;
    record_todoist_occurrence_in_tx(
        tx,
        user_id,
        &series_id,
        &next_key,
        next_task_id,
        result.entry_id,
    )
    .await?;
    record_todoist_external_ref_in_tx(
        tx,
        user_id,
        &series_id,
        next_task_id,
        result.entry_id,
        Some(&series_id),
        Some(&next_key),
        true,
    )
    .await?;
    Ok(Some(next_task_id))
}

async fn materialize_unparseable_completed_occurrence_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    review_task_id: Uuid,
    series_id: &str,
    occurrence_key: &str,
    completed_at: DateTime<Utc>,
) -> ApiResult<(Uuid, bool)> {
    if let Some((task_id, _)) =
        todoist_occurrence_in_tx(tx, user_id, series_id, occurrence_key).await?
    {
        return Ok((task_id, false));
    }
    let row = fetch_task_row(tx, user_id, review_task_id, true)
        .await?
        .ok_or_else(|| ApiError::not_found("task_not_found", &review_task_id.to_string()))?;
    let mut metadata: Value = row.get("metadata");
    let next_task_id = Uuid::now_v7();
    let task = direct_task_object_mut(&mut metadata)?;
    let current_external = task
        .get("external_refs")
        .and_then(Value::as_array)
        .and_then(|refs| {
            refs.iter()
                .find(|external| external.get("system").and_then(Value::as_str) == Some("todoist"))
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    task.insert("id".to_owned(), json!(next_task_id));
    task.insert(
        "status".to_owned(),
        json!({"value":"done","source":"todoist","set_at":completed_at}),
    );
    task.insert(
        "completed_via".to_owned(),
        json!({"value":"todoist","source":"todoist","set_at":completed_at}),
    );
    task.insert("done_at".to_owned(), json!(completed_at));
    task.remove("dropped_at");
    task.remove("dropped_reason");
    task.remove("today_pin");
    if let Some(provenance) = task.get_mut("provenance").and_then(Value::as_object_mut) {
        provenance.insert("created_at".to_owned(), json!(completed_at));
        provenance.insert("created_by".to_owned(), json!("todoist"));
        provenance.insert(
            "captured_from".to_owned(),
            json!(format!("todoist:series:{series_id}")),
        );
    }
    if let Some(recurrence) = task
        .get_mut("recurrence")
        .and_then(|cell| cell.get_mut("value"))
        .and_then(Value::as_object_mut)
    {
        recurrence.insert("due".to_owned(), json!(occurrence_key));
        recurrence.insert("needs_review".to_owned(), json!(true));
    }
    let soft_due = occurrence_key
        .get(..10)
        .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
    task.insert(
        "soft_due".to_owned(),
        json!({"value":soft_due,"source":"todoist","set_at":completed_at}),
    );
    task.insert(
        "external_refs".to_owned(),
        json!([{
            "system":"todoist",
            "id":series_id,
            "url":current_external.get("url").cloned().unwrap_or_else(||json!(todoist_task_url(series_id))),
            "project_id":current_external.get("project_id").cloned().unwrap_or(Value::Null),
            "series_id":series_id,
            "occurrence_key":occurrence_key,
            "current":false,
            "last_seen_at":completed_at,
        }]),
    );
    let content = todoist_task_content(task)?;
    let path = format!("{TASK_ENTRY_PREFIX}{next_task_id}.md");
    let prepared = simple_core::prepare_task_markdown_for_update(path, content, metadata, 0)?;
    let result =
        simple_core::upsert_markdown_in_tx(tx, user_id, Some(producer_credential_id), prepared)
            .await?;
    record_todoist_occurrence_in_tx(
        tx,
        user_id,
        series_id,
        occurrence_key,
        next_task_id,
        result.entry_id,
    )
    .await?;
    Ok((next_task_id, true))
}

async fn seed_unknown_full_sync_items_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    owner_timezone: Tz,
    responses: &[TodoistSyncResponse],
    now: DateTime<Utc>,
) -> ApiResult<usize> {
    let mut created = 0;
    for response in responses.iter().filter(|response| response.full_sync) {
        for item in &response.items {
            let mapped = map_item(item, owner_timezone)?;
            if mapped.terminal == TodoistTerminal::Deleted
                || todoist_external_ref_in_tx(tx, user_id, &mapped.external_id)
                    .await?
                    .is_some()
            {
                continue;
            }
            let project_slug = todoist_project_slug_in_tx(tx, user_id, &mapped.project_id).await?;
            let (contexts, context_review) =
                todoist_contexts_in_tx(tx, user_id, producer_credential_id, &mapped.labels).await?;
            let (task_id, entry_id) = create_todoist_task_in_tx(
                tx,
                user_id,
                producer_credential_id,
                &mapped,
                &project_slug,
                &contexts,
                &context_review,
                now,
            )
            .await?;
            let series_id = mapped
                .recurrence
                .as_ref()
                .map(|recurrence| recurrence.series_id.as_str());
            if let (Some(series_id), Some(occurrence_key)) =
                (series_id, mapped.occurrence_key.as_deref())
            {
                record_todoist_occurrence_in_tx(
                    tx,
                    user_id,
                    series_id,
                    occurrence_key,
                    task_id,
                    entry_id,
                )
                .await?;
            }
            record_todoist_external_ref_in_tx(
                tx,
                user_id,
                &mapped.external_id,
                task_id,
                entry_id,
                series_id,
                mapped.occurrence_key.as_deref(),
                true,
            )
            .await?;
            created += 1;
        }
    }
    Ok(created)
}

#[doc(hidden)]
pub async fn apply_todoist_sync_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    producer_credential_id: Uuid,
    owner_timezone: Tz,
    responses: &[TodoistSyncResponse],
    completed_occurrences: &[TodoistCompletedOccurrence],
) -> ApiResult<TodoistApplyReport> {
    let mut report = TodoistApplyReport::default();
    let now = Utc::now();
    // Cache every project delta before applying any items. A full snapshot
    // followed by its immediate incremental catch-up can otherwise route an
    // item using a project name that the second response already renamed.
    let mut changed_project_ids = BTreeSet::new();
    for response in responses {
        for project in &response.projects {
            cache_todoist_project_in_tx(
                tx,
                user_id,
                &project.id,
                &project.name,
                project.is_deleted,
            )
            .await?;
            changed_project_ids.insert(project.id.clone());
            report.projects_seen += 1;
        }
    }
    for project_id in changed_project_ids {
        report.updated += remap_todoist_project_tasks_in_tx(
            tx,
            user_id,
            producer_credential_id,
            &project_id,
            now,
        )
        .await?;
    }
    // On a cursorless logical pull, establish the stale full-sync baseline
    // before consuming completion evidence. The immediate incremental item
    // pass below then refreshes/reuses the occurrence materialized by that
    // evidence instead of erasing the completed baseline occurrence.
    report.created += seed_unknown_full_sync_items_in_tx(
        tx,
        user_id,
        producer_credential_id,
        owner_timezone,
        responses,
        now,
    )
    .await?;
    let initial_full_baseline = responses.iter().any(|response| response.full_sync);
    let mut ordered_completions = completed_occurrences.to_vec();
    ordered_completions.sort_by(|left, right| {
        left.completed_at
            .cmp(&right.completed_at)
            .then_with(|| left.external_id.cmp(&right.external_id))
            .then_with(|| left.occurrence_key.cmp(&right.occurrence_key))
    });
    for completed in &ordered_completions {
        if completed.external_id.is_empty()
            || completed.external_id.len() > 512
            || completed.external_id.chars().any(char::is_control)
        {
            return Err(ApiError::invalid(
                "Todoist completed occurrence identity is invalid",
            ));
        }
        let Some(external) =
            todoist_external_ref_in_tx(tx, user_id, &completed.external_id).await?
        else {
            continue;
        };
        let (target_task_id, advances_current_occurrence) = if let Some(series_id) =
            external.series_id.as_deref()
        {
            let occurrence_key = completed.occurrence_key.as_deref().ok_or_else(|| {
                ApiError::invalid("Todoist recurring completion requires an occurrence identity")
            })?;
            if external.occurrence_key.as_deref() == Some(occurrence_key) {
                (external.task_id, true)
            } else if external
                .occurrence_key
                .as_deref()
                .is_some_and(|key| key.starts_with("review:"))
            {
                let (_, created) = materialize_unparseable_completed_occurrence_in_tx(
                    tx,
                    user_id,
                    producer_credential_id,
                    external.task_id,
                    series_id,
                    occurrence_key,
                    completed.completed_at,
                )
                .await?;
                report.completed += usize::from(created);
                continue;
            } else if let Some((task_id, _)) =
                todoist_occurrence_in_tx(tx, user_id, series_id, occurrence_key).await?
            {
                // A replay of already materialized history may name an
                // older occurrence. Complete it monotonically without
                // moving the current external pointer backwards.
                (task_id, false)
            } else if initial_full_baseline
                && external
                    .occurrence_key
                    .as_deref()
                    .is_some_and(|current| occurrence_key < current)
            {
                // A current full snapshot defines the import baseline.
                // The bounded completion overlap may legitimately include
                // a pre-import occurrence older than that baseline; it has
                // no canonical local task to complete and must not wedge
                // initial configuration. The stale-full case is handled
                // above because its baseline key matches before the
                // immediate incremental response advances the series.
                continue;
            } else {
                // Never commit the Sync cursor across an unexplained
                // occurrence gap. A later retry or owner review can
                // reconcile it without silently losing task history.
                return Err(ApiError::conflict(
                    "todoist_occurrence_gap",
                    "Todoist completion history does not follow the canonical occurrence ledger",
                    json!({
                        "series_id":series_id,
                        "occurrence_key":occurrence_key,
                    }),
                ));
            }
        } else {
            (external.task_id, false)
        };
        let recurrence_authoritative = advances_current_occurrence
            && todoist_recurrence_is_authoritative_in_tx(tx, user_id, target_task_id).await?;
        if complete_existing_todoist_task_in_tx(
            tx,
            user_id,
            producer_credential_id,
            target_task_id,
            completed.completed_at,
        )
        .await?
        {
            report.completed += 1;
        }
        // Advance the current pointer before consuming the next chronological
        // completion record. This preserves every occurrence across long poll
        // gaps and makes the later active Sync item reuse the last materialized
        // occurrence rather than creating a duplicate.
        if recurrence_authoritative {
            materialize_next_todoist_occurrence_in_tx(
                tx,
                user_id,
                producer_credential_id,
                target_task_id,
                completed.completed_at,
            )
            .await?;
        }
    }
    for response in responses {
        for item in &response.items {
            let mapped = map_item(item, owner_timezone)?;
            report.items_seen += 1;
            let external = todoist_external_ref_in_tx(tx, user_id, &mapped.external_id).await?;
            if mapped.terminal == TodoistTerminal::Deleted && external.is_none() {
                report.unchanged += 1;
                continue;
            }

            let project_slug = if mapped.terminal == TodoistTerminal::Deleted {
                None
            } else {
                Some(todoist_project_slug_in_tx(tx, user_id, &mapped.project_id).await?)
            };
            let (contexts, context_review) = if mapped.terminal == TodoistTerminal::Deleted {
                (None, None)
            } else {
                let (contexts, review) =
                    todoist_contexts_in_tx(tx, user_id, producer_credential_id, &mapped.labels)
                        .await?;
                (Some(contexts), Some(review))
            };

            let Some(recurrence) = mapped.recurrence.as_ref() else {
                let (task_id, entry_id, created, changed, terminal_changed) =
                    if let Some(external) = external.as_ref() {
                        let (entry_id, changed, terminal_changed) =
                            refresh_existing_todoist_task_in_tx(
                                tx,
                                user_id,
                                producer_credential_id,
                                external.task_id,
                                &mapped,
                                project_slug.as_deref(),
                                contexts.as_deref(),
                                context_review.as_deref(),
                                now,
                            )
                            .await?;
                        (external.task_id, entry_id, false, changed, terminal_changed)
                    } else {
                        let (task_id, entry_id) = create_todoist_task_in_tx(
                            tx,
                            user_id,
                            producer_credential_id,
                            &mapped,
                            project_slug.as_deref().unwrap_or("todoist-inbox"),
                            contexts.as_deref().unwrap_or_default(),
                            context_review.as_deref().unwrap_or_default(),
                            now,
                        )
                        .await?;
                        (
                            task_id,
                            entry_id,
                            true,
                            true,
                            mapped.terminal != TodoistTerminal::Open,
                        )
                    };
                let clear_series = mapped.terminal != TodoistTerminal::Deleted
                    && external
                        .as_ref()
                        .is_some_and(|value| value.series_id.is_some());
                let entry_id = if clear_series {
                    set_todoist_canonical_identity_in_tx(
                        tx,
                        user_id,
                        producer_credential_id,
                        task_id,
                        &mapped.external_id,
                        None,
                        None,
                        true,
                        now,
                    )
                    .await?
                } else {
                    entry_id
                };
                record_todoist_external_ref_in_tx(
                    tx,
                    user_id,
                    &mapped.external_id,
                    task_id,
                    entry_id,
                    if clear_series {
                        None
                    } else {
                        external
                            .as_ref()
                            .and_then(|value| value.series_id.as_deref())
                    },
                    if clear_series {
                        None
                    } else {
                        external
                            .as_ref()
                            .and_then(|value| value.occurrence_key.as_deref())
                    },
                    true,
                )
                .await?;
                report.created += usize::from(created);
                report.updated += usize::from(!created && changed);
                report.completed +=
                    usize::from(terminal_changed && mapped.terminal == TodoistTerminal::Completed);
                report.dropped +=
                    usize::from(terminal_changed && mapped.terminal == TodoistTerminal::Deleted);
                report.unchanged += usize::from(!changed);
                continue;
            };

            if let Some(external) = external.as_ref()
                && !todoist_recurrence_is_authoritative_in_tx(tx, user_id, external.task_id).await?
            {
                let (entry_id, changed, terminal_changed) = refresh_existing_todoist_task_in_tx(
                    tx,
                    user_id,
                    producer_credential_id,
                    external.task_id,
                    &mapped,
                    project_slug.as_deref(),
                    contexts.as_deref(),
                    context_review.as_deref(),
                    now,
                )
                .await?;
                record_todoist_external_ref_in_tx(
                    tx,
                    user_id,
                    &mapped.external_id,
                    external.task_id,
                    entry_id,
                    external.series_id.as_deref(),
                    external.occurrence_key.as_deref(),
                    false,
                )
                .await?;
                report.updated += usize::from(changed);
                report.completed +=
                    usize::from(terminal_changed && mapped.terminal == TodoistTerminal::Completed);
                report.dropped +=
                    usize::from(terminal_changed && mapped.terminal == TodoistTerminal::Deleted);
                report.unchanged += usize::from(!changed);
                continue;
            }

            let occurrence_key = mapped.occurrence_key.as_deref().ok_or_else(|| {
                ApiError::invalid("Todoist recurrence requires an occurrence key")
            })?;
            let occurrence =
                todoist_occurrence_in_tx(tx, user_id, &recurrence.series_id, occurrence_key)
                    .await?;
            let repoint = todoist_should_repoint(
                external
                    .as_ref()
                    .and_then(|value| value.occurrence_key.as_deref()),
                occurrence_key,
            ) && !matches!(
                (external.as_ref(), occurrence.as_ref()),
                (Some(external), Some((occurrence_task_id, _)))
                    if *occurrence_task_id != external.task_id
            );
            if let Some(external) = external.as_ref()
                && external
                    .occurrence_key
                    .as_deref()
                    .is_some_and(|current| current.starts_with("review:"))
            {
                if occurrence.is_some() {
                    report.unchanged += 1;
                    continue;
                }
                let (entry_id, changed, terminal_changed) = refresh_existing_todoist_task_in_tx(
                    tx,
                    user_id,
                    producer_credential_id,
                    external.task_id,
                    &mapped,
                    project_slug.as_deref(),
                    contexts.as_deref(),
                    context_review.as_deref(),
                    now,
                )
                .await?;
                record_todoist_external_ref_in_tx(
                    tx,
                    user_id,
                    &mapped.external_id,
                    external.task_id,
                    entry_id,
                    external.series_id.as_deref(),
                    external.occurrence_key.as_deref(),
                    false,
                )
                .await?;
                report.updated += usize::from(changed);
                report.completed +=
                    usize::from(terminal_changed && mapped.terminal == TodoistTerminal::Completed);
                report.dropped +=
                    usize::from(terminal_changed && mapped.terminal == TodoistTerminal::Deleted);
                report.unchanged += usize::from(!changed);
                continue;
            }
            if occurrence.is_none()
                && repoint
                && external
                    .as_ref()
                    .is_some_and(|value| value.occurrence_key.as_deref() != Some(occurrence_key))
            {
                let external = external.as_ref().expect("checked external");
                let current_status = sqlx::query_scalar::<_, String>(
                    "SELECT status FROM straylight.task_index WHERE user_id=$1 AND task_id=$2",
                )
                .bind(user_id)
                .bind(external.task_id)
                .fetch_one(&mut **tx)
                .await?;
                if current_status != "done" {
                    let (_, changed, terminal_changed) = refresh_existing_todoist_task_in_tx(
                        tx,
                        user_id,
                        producer_credential_id,
                        external.task_id,
                        &mapped,
                        project_slug.as_deref(),
                        contexts.as_deref(),
                        context_review.as_deref(),
                        now,
                    )
                    .await?;
                    let entry_id = set_todoist_canonical_identity_in_tx(
                        tx,
                        user_id,
                        producer_credential_id,
                        external.task_id,
                        &mapped.external_id,
                        Some(&recurrence.series_id),
                        Some(occurrence_key),
                        true,
                        now,
                    )
                    .await?;
                    record_todoist_occurrence_in_tx(
                        tx,
                        user_id,
                        &recurrence.series_id,
                        occurrence_key,
                        external.task_id,
                        entry_id,
                    )
                    .await?;
                    record_todoist_external_ref_in_tx(
                        tx,
                        user_id,
                        &mapped.external_id,
                        external.task_id,
                        entry_id,
                        Some(&recurrence.series_id),
                        Some(occurrence_key),
                        true,
                    )
                    .await?;
                    report.updated += usize::from(changed);
                    report.completed += usize::from(
                        terminal_changed && mapped.terminal == TodoistTerminal::Completed,
                    );
                    report.dropped += usize::from(
                        terminal_changed && mapped.terminal == TodoistTerminal::Deleted,
                    );
                    report.unchanged += usize::from(!changed);
                    continue;
                }
            }
            if occurrence.is_none() && external.is_some() && !repoint {
                report.unchanged += 1;
                continue;
            }

            if repoint
                && external.as_ref().is_some_and(|external| {
                    occurrence
                        .as_ref()
                        .is_none_or(|(task_id, _)| *task_id != external.task_id)
                })
            {
                let external = external.as_ref().expect("checked external");
                set_todoist_canonical_identity_in_tx(
                    tx,
                    user_id,
                    producer_credential_id,
                    external.task_id,
                    &mapped.external_id,
                    external.series_id.as_deref(),
                    external.occurrence_key.as_deref(),
                    false,
                    now,
                )
                .await?;
            }
            let (task_id, entry_id, created, changed, terminal_changed) =
                if let Some((task_id, _)) = occurrence {
                    let (entry_id, changed, terminal_changed) =
                        refresh_existing_todoist_task_in_tx(
                            tx,
                            user_id,
                            producer_credential_id,
                            task_id,
                            &mapped,
                            project_slug.as_deref(),
                            contexts.as_deref(),
                            context_review.as_deref(),
                            now,
                        )
                        .await?;
                    (task_id, entry_id, false, changed, terminal_changed)
                } else {
                    let (task_id, entry_id) = create_todoist_task_in_tx(
                        tx,
                        user_id,
                        producer_credential_id,
                        &mapped,
                        project_slug.as_deref().unwrap_or("todoist-inbox"),
                        contexts.as_deref().unwrap_or_default(),
                        context_review.as_deref().unwrap_or_default(),
                        now,
                    )
                    .await?;
                    (
                        task_id,
                        entry_id,
                        true,
                        true,
                        mapped.terminal != TodoistTerminal::Open,
                    )
                };
            let entry_id = if repoint {
                set_todoist_canonical_identity_in_tx(
                    tx,
                    user_id,
                    producer_credential_id,
                    task_id,
                    &mapped.external_id,
                    Some(&recurrence.series_id),
                    Some(occurrence_key),
                    true,
                    now,
                )
                .await?
            } else {
                entry_id
            };
            record_todoist_occurrence_in_tx(
                tx,
                user_id,
                &recurrence.series_id,
                occurrence_key,
                task_id,
                entry_id,
            )
            .await?;
            record_todoist_external_ref_in_tx(
                tx,
                user_id,
                &mapped.external_id,
                task_id,
                entry_id,
                Some(&recurrence.series_id),
                Some(occurrence_key),
                repoint,
            )
            .await?;
            report.created += usize::from(created);
            report.updated += usize::from(!created && changed);
            report.completed +=
                usize::from(terminal_changed && mapped.terminal == TodoistTerminal::Completed);
            report.dropped +=
                usize::from(terminal_changed && mapped.terminal == TodoistTerminal::Deleted);
            report.unchanged += usize::from(!changed);
        }
    }
    sqlx::query(
        r#"
        INSERT INTO straylight.task_audit_events(
          user_id,credential_id,action,details
        ) VALUES($1,$2,'todoist.pull.apply',$3)
        "#,
    )
    .bind(user_id)
    .bind(producer_credential_id)
    .bind(json!({
        "projects_seen":report.projects_seen,
        "items_seen":report.items_seen,
        "created":report.created,
        "updated":report.updated,
        "completed":report.completed,
        "dropped":report.dropped,
        "unchanged":report.unchanged,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(report)
}

async fn todoist_status_json_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    environment_enabled: bool,
) -> ApiResult<Value> {
    let config=sqlx::query("SELECT mode,configuration_generation,updated_at FROM straylight.task_integration_config WHERE user_id=$1 AND system='todoist'")
        .bind(user_id).fetch_one(&mut **tx).await?;
    let sync=sqlx::query("SELECT configuration_generation,last_run_at,last_outcome,last_error_code,next_run_at,updated_at FROM straylight.task_sync_state WHERE user_id=$1 AND system='todoist'")
        .bind(user_id).fetch_optional(&mut **tx).await?;
    let token_configured =
        sqlx::query_scalar::<_, bool>("SELECT straylight.task_todoist_token_configured($1)")
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await?;
    let saved_mode: String = config.get("mode");
    let effective_mode = if environment_enabled && token_configured {
        saved_mode.as_str()
    } else {
        "off"
    };
    Ok(json!({
        "environment_enabled":environment_enabled,
        "saved_mode":saved_mode,
        "effective_mode":effective_mode,
        "token_configured":token_configured,
        "configuration_generation":config.get::<i64,_>("configuration_generation"),
        "last_run_at":sync.as_ref().and_then(|row|row.get::<Option<DateTime<Utc>>,_>("last_run_at")),
        "last_outcome":sync.as_ref().and_then(|row|row.get::<Option<String>,_>("last_outcome")),
        "last_error_code":sync.as_ref().and_then(|row|row.get::<Option<String>,_>("last_error_code")),
        "next_run_at":sync.as_ref().and_then(|row|row.get::<Option<DateTime<Utc>>,_>("next_run_at")),
    }))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigureTodoistRequest {
    pub expected_generation: i64,
    pub idempotency_key: String,
    pub mode: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PullTodoistRequest {
    pub idempotency_key: String,
}

async fn require_todoist_web_owner_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> ApiResult<()> {
    let result = sqlx::query("SELECT straylight.require_todoist_web_owner($1)")
        .bind(user_id)
        .execute(&mut **tx)
        .await;
    if let Err(error) = result {
        if error
            .as_database_error()
            .and_then(|database| database.code())
            .as_deref()
            == Some("42501")
        {
            return Err(ApiError::public(
                axum::http::StatusCode::FORBIDDEN,
                "todoist_owner_web_required",
                "Todoist integration management requires an owner Web session",
            ));
        }
        return Err(error.into());
    }
    Ok(())
}

pub(crate) async fn configure_todoist(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<ConfigureTodoistRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::IntegrationManage)?;
    if request.expected_generation < 1 {
        return Err(ApiError::invalid("expected_generation must be positive"));
    }
    if !["off", "import_once", "pull"].contains(&request.mode.as_str()) {
        return Err(ApiError::invalid(
            "Todoist mode must be off, import_once, or pull",
        ));
    }
    let mut tx = state.begin_write(&auth).await?;
    require_todoist_web_owner_in_tx(&mut tx, auth.user_id.0).await?;
    match begin_receipt(
        &mut tx,
        &auth,
        "task.todoist.configure",
        &request.idempotency_key,
        &request,
    )
    .await?
    {
        ReceiptStart::Replay(receipt) => {
            tx.commit().await?;
            return Ok(envelope(ResponseStatus::NoOp, receipt));
        }
        ReceiptStart::New => {}
    }

    let row = sqlx::query(
        "SELECT mode,configuration_generation FROM straylight.task_integration_config WHERE user_id=$1 AND system='todoist' FOR UPDATE",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?;
    let current_mode: String = row.get("mode");
    let current_generation: i64 = row.get("configuration_generation");
    if current_generation != request.expected_generation {
        return Err(ApiError::conflict(
            "todoist_configuration_conflict",
            "Todoist configuration changed after expected_generation",
            json!({
                "expected_generation":request.expected_generation,
                "current_generation":current_generation,
            }),
        ));
    }
    let changed = current_mode != request.mode;
    let generation = current_generation + i64::from(changed);
    if changed {
        sqlx::query(
            r#"
            UPDATE straylight.task_integration_config
            SET mode=$2,configuration_generation=$3,updated_at=clock_timestamp()
            WHERE user_id=$1 AND system='todoist'
            "#,
        )
        .bind(auth.user_id.0)
        .bind(&request.mode)
        .bind(generation)
        .execute(&mut *tx)
        .await?;
    }
    let token_configured =
        sqlx::query_scalar::<_, bool>("SELECT straylight.task_todoist_token_configured($1)")
            .bind(auth.user_id.0)
            .fetch_one(&mut *tx)
            .await?;
    let import_once_complete = !changed
        && request.mode == "import_once"
        && sqlx::query_scalar::<_, Option<String>>(
            "SELECT last_outcome FROM straylight.task_sync_state WHERE user_id=$1 AND system='todoist'",
        )
        .bind(auth.user_id.0)
        .fetch_one(&mut *tx)
        .await?
        .as_deref()
            == Some("success");
    let eligible = state.config.todoist_sync_enabled
        && token_configured
        && request.mode.as_str() != "off"
        && !import_once_complete;
    if changed {
        sqlx::query(
            r#"
            UPDATE straylight.task_sync_state
            SET configuration_generation=$2,last_outcome=NULL,last_error_code=NULL,
                next_run_at=CASE WHEN $3 THEN clock_timestamp() ELSE NULL END,
                manual_requested_at=NULL,lease_owner=NULL,lease_expires_at=NULL,
                updated_at=clock_timestamp()
            WHERE user_id=$1 AND system='todoist'
            "#,
        )
        .bind(auth.user_id.0)
        .bind(generation)
        .bind(eligible)
        .execute(&mut *tx)
        .await?;
    } else if !eligible {
        sqlx::query(
            r#"
            UPDATE straylight.task_sync_state
            SET next_run_at=NULL,manual_requested_at=NULL,
                lease_owner=NULL,lease_expires_at=NULL,updated_at=clock_timestamp()
            WHERE user_id=$1 AND system='todoist'
            "#,
        )
        .bind(auth.user_id.0)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO straylight.task_audit_events(user_id,credential_id,action,details) VALUES($1,$2,'todoist.configure',$3)",
    )
    .bind(auth.user_id.0)
    .bind(auth.credential_id.0)
    .bind(json!({"mode":request.mode,"generation":generation,"changed":changed}))
    .execute(&mut *tx)
    .await?;
    let status =
        todoist_status_json_in_tx(&mut tx, auth.user_id.0, state.config.todoist_sync_enabled)
            .await?;
    let receipt = json!({"status":status,"changed":changed,"replayed":false});
    finalize_receipt(
        &mut tx,
        &auth,
        "task.todoist.configure",
        &request.idempotency_key,
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(envelope(
        if changed {
            ResponseStatus::Committed
        } else {
            ResponseStatus::NoOp
        },
        receipt,
    ))
}

pub(crate) async fn pull_todoist(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<PullTodoistRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::IntegrationManage)?;
    let mut tx = state.begin_write(&auth).await?;
    require_todoist_web_owner_in_tx(&mut tx, auth.user_id.0).await?;
    match begin_receipt(
        &mut tx,
        &auth,
        "task.todoist.pull",
        &request.idempotency_key,
        &request,
    )
    .await?
    {
        ReceiptStart::Replay(receipt) => {
            tx.commit().await?;
            return Ok(envelope(ResponseStatus::NoOp, receipt));
        }
        ReceiptStart::New => {}
    }
    let row = sqlx::query(
        r#"
        SELECT config.mode,config.configuration_generation,
               state.last_outcome,state.configuration_generation AS state_generation
        FROM straylight.task_integration_config AS config
        JOIN straylight.task_sync_state AS state
          ON state.user_id=config.user_id AND state.system=config.system
        WHERE config.user_id=$1 AND config.system='todoist'
        FOR UPDATE OF config,state
        "#,
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?;
    let mode: String = row.get("mode");
    let generation: i64 = row.get("configuration_generation");
    let state_generation: i64 = row.get("state_generation");
    let last_outcome: Option<String> = row.get("last_outcome");
    let token_configured =
        sqlx::query_scalar::<_, bool>("SELECT straylight.task_todoist_token_configured($1)")
            .bind(auth.user_id.0)
            .fetch_one(&mut *tx)
            .await?;
    let eligible = state.config.todoist_sync_enabled
        && token_configured
        && matches!(mode.as_str(), "import_once" | "pull")
        && !(mode == "import_once"
            && state_generation == generation
            && last_outcome.as_deref() == Some("success"));
    if eligible {
        sqlx::query(
            r#"
            UPDATE straylight.task_sync_state
            SET manual_requested_at=COALESCE(manual_requested_at,clock_timestamp()),
                next_run_at=COALESCE(next_run_at,clock_timestamp()),
                updated_at=clock_timestamp()
            WHERE user_id=$1 AND system='todoist'
            "#,
        )
        .bind(auth.user_id.0)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            r#"
            UPDATE straylight.task_sync_state
            SET next_run_at=NULL,manual_requested_at=NULL,
                lease_owner=NULL,lease_expires_at=NULL,updated_at=clock_timestamp()
            WHERE user_id=$1 AND system='todoist'
            "#,
        )
        .bind(auth.user_id.0)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO straylight.task_audit_events(user_id,credential_id,action,details) VALUES($1,$2,'todoist.pull.request',$3)",
    )
    .bind(auth.user_id.0)
    .bind(auth.credential_id.0)
    .bind(json!({"queued":eligible,"mode":mode,"generation":generation}))
    .execute(&mut *tx)
    .await?;
    let status =
        todoist_status_json_in_tx(&mut tx, auth.user_id.0, state.config.todoist_sync_enabled)
            .await?;
    let receipt = json!({"queued":eligible,"status":status,"replayed":false});
    finalize_receipt(
        &mut tx,
        &auth,
        "task.todoist.pull",
        &request.idempotency_key,
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(envelope(
        if eligible {
            ResponseStatus::Committed
        } else {
            ResponseStatus::NoOp
        },
        receipt,
    ))
}

pub(crate) async fn todoist_status(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::TaskRead)?;
    let mut tx = state.begin_read(&auth).await?;
    let data =
        todoist_status_json_in_tx(&mut tx, auth.user_id.0, state.config.todoist_sync_enabled)
            .await?;
    tx.commit().await?;
    Ok(envelope(ResponseStatus::Complete, data))
}

fn display_name(slug: &str) -> String {
    slug.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn collect_string_values(value: Option<&Value>, output: &mut Vec<String>) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::String(value) => output.push(value.to_owned()),
        Value::Array(values) => {
            for value in values {
                collect_string_values(Some(value), output);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_string_values(Some(value), output);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::models::{CredentialId, UserId};

    use super::*;

    fn cell(value: Value, source: &str) -> Value {
        json!({
            "value": value,
            "source": source,
            "set_at": "2026-08-27T07:00:00Z"
        })
    }

    fn auth(capabilities: &[&str]) -> AuthContext {
        AuthContext {
            credential_id: CredentialId(
                Uuid::parse_str("0198f000-0000-7000-8000-000000000001").unwrap(),
            ),
            user_id: UserId(Uuid::parse_str("0198f000-0000-7000-8000-000000000002").unwrap()),
            capabilities: capabilities
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<HashSet<_>>(),
            scope_refs: vec!["scope:test".to_owned()],
            read_only: true,
        }
    }

    #[test]
    fn public_actor_sources_are_bound_without_losing_trusted_delegation() {
        let narrow = auth(&["task.write"]);
        assert_eq!(
            canonical_public_source(&narrow, "agent:spoofed").unwrap(),
            "agent:0198f000-0000-7000-8000-000000000001"
        );
        assert_eq!(
            canonical_completed_via(&narrow, "agent:victim").unwrap(),
            "agent:0198f000-0000-7000-8000-000000000001"
        );
        assert!(canonical_completed_via(&narrow, "ios").is_err());
        assert!(canonical_completed_via(&narrow, "web").is_err());
        assert!(canonical_public_source(&narrow, "owner").is_err());

        let owner_device = auth(&["task.write", "notification:manage"]);
        assert_eq!(
            canonical_public_source(&owner_device, "owner").unwrap(),
            "owner"
        );
        assert_eq!(
            canonical_public_source(&owner_device, "agent:spoofed").unwrap(),
            "agent:0198f000-0000-7000-8000-000000000001"
        );
        assert_eq!(
            canonical_completed_via(&owner_device, "ios").unwrap(),
            "ios"
        );
        assert!(canonical_completed_via(&owner_device, "web").is_err());

        let trusted = auth(&["task.write", "credential:manage"]);
        assert_eq!(
            canonical_public_source(&trusted, "agent:gate12").unwrap(),
            "agent:gate12"
        );
        assert_eq!(
            canonical_completed_via(&trusted, "agent:gate12").unwrap(),
            "agent:gate12"
        );
        assert_eq!(canonical_completed_via(&trusted, "web").unwrap(), "web");
        assert!(canonical_completed_via(&trusted, "ios").is_err());
        assert!(canonical_public_source(&trusted, &format!("agent:{}", "x".repeat(201))).is_err());
    }

    #[test]
    fn public_strings_and_portable_numeric_ranges_fail_closed() {
        let writer = auth(&["task.write"]);
        let capture = CaptureItem {
            client_ref: Some("bad\0ref".to_owned()),
            raw_text: "valid title".to_owned(),
            captured_from: None,
            title: None,
            notes: None,
            project: None,
            ready_at: None,
            soft_due: None,
            hard_due: None,
            hard_due_lead_days: None,
            cost_of_delay: None,
            required_contexts: None,
            estimate_minutes: None,
        };
        assert!(validate_capture_item(&writer, &capture).is_err());
        assert!(validate_correction_value("title", &json!("bad\0title")).is_err());
        assert!(validate_correction_value("notes", &json!("ok\nmarkdown\u{0007}")).is_err());
        assert!(normalize_slug("home\0office").is_err());

        let task_id = Uuid::now_v7();
        for (field, value) in [
            ("hard_due_lead_days", json!(3651)),
            ("estimate_minutes", json!(0)),
        ] {
            let metadata = json!({
                "kind":"task",
                "schema":TASK_SCHEMA,
                "task":{
                    "id":task_id,
                    "title":"Portable range",
                    field:cell(value,"owner")
                }
            });
            assert!(
                validate_task_entry(&format!("{TASK_ENTRY_PREFIX}{task_id}.md"), &metadata)
                    .is_err(),
                "portable task field {field} must enforce the public range"
            );
        }
    }

    #[test]
    fn task_refs_and_managed_paths_require_lowercase_uuid_v7() {
        let v7 = Uuid::parse_str("0198f000-0000-7000-8000-000000000001").unwrap();
        assert_eq!(parse_task_ref(&v7.to_string()).unwrap(), v7);
        assert_eq!(
            task_id_from_path(&format!("{TASK_ENTRY_PREFIX}{v7}.md")),
            Some(v7)
        );
        assert!(parse_task_ref(&v7.to_string().to_uppercase()).is_err());
        let v4 = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert!(parse_task_ref(&v4.to_string()).is_err());
        assert!(task_id_from_path(&format!("{TASK_ENTRY_PREFIX}{v4}.md")).is_none());
    }

    #[test]
    fn corrections_are_strict_typed_and_actions_reject_unknown_fields() {
        assert!(validate_correction_value("status", &json!("done")).is_err());
        assert!(validate_correction_value("estimate_minutes", &json!(0)).is_err());
        assert!(
            validate_correction_value("cost_of_delay", &json!({"amount_cents":100,"per":"day"}))
                .is_err()
        );
        assert!(validate_correction_value("required_contexts", &json!(["home", "online"])).is_ok());
        let unknown = json!({"expected_version":1,"idempotency_key":"k","operation":{"type":"complete","source":"owner","completed_via":"ios","extra":true}});
        assert!(serde_json::from_value::<UpdateTaskRequest>(unknown).is_err());
        let todoist = json!({"expected_version":1,"idempotency_key":"k","operation":{"type":"complete","source":"agent:test","completed_via":"todoist"}});
        let parsed = serde_json::from_value::<UpdateTaskRequest>(todoist).unwrap();
        assert!(validate_update_operation(&auth(&["task.write"]), &parsed.operation).is_err());
        let complete=serde_json::from_value::<UpdateTaskRequest>(json!({"expected_version":1,"idempotency_key":"k","operation":{"type":"complete","source":"agent:test","completed_via":"agent:test"}})).unwrap();
        assert!(validate_action_state(&complete.operation, "done").is_err());
        assert!(validate_action_state(&complete.operation, "open").is_ok());
        let reopen=serde_json::from_value::<UpdateTaskRequest>(json!({"expected_version":1,"idempotency_key":"k","operation":{"type":"reopen","source":"agent:test"}})).unwrap();
        assert!(validate_action_state(&reopen.operation, "done").is_ok());
        assert!(validate_action_state(&reopen.operation, "waiting").is_ok());
        assert!(validate_action_state(&reopen.operation, "open").is_err());
    }

    #[test]
    fn path_fallback_is_segment_bounded_and_project_paths_are_canonical() {
        assert!(path_prefix_matches("/tmp/repo/file.rs", "/tmp/repo"));
        assert!(path_prefix_matches("/tmp/repo", "/tmp/repo/"));
        assert!(!path_prefix_matches(
            "/tmp/repository-evil/file",
            "/tmp/repo"
        ));
        assert_eq!(
            canonical_optional_path(Some("/tmp/gate12-repo/"), "repo_path", true)
                .unwrap()
                .as_deref(),
            Some("/tmp/gate12-repo")
        );
        assert!(canonical_optional_path(Some("/tmp/repo/../escape"), "repo_path", true).is_err());
        assert!(canonical_optional_path(Some("sources//Project"), "hub_path", false).is_err());
    }

    #[test]
    fn canonical_cells_and_receipt_targets_fail_closed() {
        let task_id = Uuid::parse_str("0198f000-0000-7000-8000-000000000001").unwrap();
        let raw = json!({"kind":"task","schema":TASK_SCHEMA,"task":{"id":task_id,"title":"Raw","status":"open"}});
        assert!(validate_task_entry(&format!("{TASK_ENTRY_PREFIX}{task_id}.md"), &raw).is_err());
        let body = json!({"expected_version":1,"operation":{"type":"complete"}});
        let first = request_hash(
            &json!({"task_ref":"0198f000-0000-7000-8000-000000000001","request":body.clone()}),
        )
        .unwrap();
        let second = request_hash(
            &json!({"task_ref":"0198f000-0000-7000-8000-000000000002","request":body}),
        )
        .unwrap();
        assert_ne!(
            first, second,
            "path task_ref is part of the durable receipt hash"
        );
    }

    #[test]
    fn canonical_task_projection_is_typed_and_tracks_sources() {
        let task_id = Uuid::now_v7();
        let metadata = json!({
            "kind": "task",
            "schema": "task.v1",
            "task": {
                "id": task_id,
                "title": "Downgrade Charlemagne",
                "status": cell(json!("open"), "owner"),
                "project": cell(json!("straylight"), "agent:codex"),
                "soft_due": cell(json!("2026-08-31"), "agent:codex"),
                "cost_of_delay": cell(json!({
                    "amount_cents": 700,
                    "per": "week",
                    "since": "2026-08-01"
                }), "agent:codex"),
                "required_contexts": cell(json!(["home", "online"]), "agent:codex"),
                "today_pin": cell(json!("2026-08-27"), "owner"),
                "provenance": {
                    "captured_by": "agent:codex",
                    "created_at": "2026-08-27T07:00:00Z"
                }
            }
        });
        let path = format!("{TASK_ENTRY_PREFIX}{task_id}.md");
        assert!(validate_task_entry(&path, &metadata).unwrap());
        let projection = parse_projection(&metadata).unwrap();
        assert_eq!(projection.soft_due.unwrap().to_string(), "2026-08-31");
        assert_eq!(projection.cost_amount_cents, Some(700));
        assert_eq!(projection.cost_period.as_deref(), Some("week"));
        assert_eq!(projection.required_contexts, ["home", "online"]);
        assert_eq!(projection.today_pin.unwrap().to_string(), "2026-08-27");
        assert_eq!(projection.provenance["soft_due"], "agent:codex");
    }

    #[test]
    fn task_identity_and_provenance_fail_closed() {
        let task_id = Uuid::now_v7();
        let other_id = Uuid::now_v7();
        let bad_identity = json!({
            "kind": "task",
            "schema": "task.v1",
            "task": {"id": other_id, "title": "Wrong id"}
        });
        assert!(
            validate_task_entry(&format!("{TASK_ENTRY_PREFIX}{task_id}.md"), &bad_identity)
                .is_err()
        );
        let bad_source = json!({
            "kind": "task",
            "schema": "task.v1",
            "task": {
                "id": task_id,
                "title": "Wrong source",
                "project": cell(json!("straylight"), "model")
            }
        });
        assert!(
            validate_task_entry(&format!("{TASK_ENTRY_PREFIX}{task_id}.md"), &bad_source).is_err()
        );
    }

    #[test]
    fn portable_import_uses_lossless_client_metadata() {
        let task_id = Uuid::now_v7();
        let metadata = json!({
            "_straylight_import": {"format": "straylight-workspace-import-manifest@v1"},
            "client": {
                "kind": "task",
                "schema": "task.v1",
                "task": {"id": task_id, "title": "Imported task"}
            }
        });
        assert!(
            validate_task_entry(&format!("{TASK_ENTRY_PREFIX}{task_id}.md"), &metadata).unwrap()
        );
    }

    #[test]
    fn sourced_precedence_requires_explicit_corrections_and_protects_owner_values() {
        let task_id = Uuid::now_v7();
        let mut metadata = json!({
            "kind": "task",
            "schema": "task.v1",
            "task": {
                "id": task_id,
                "title": "Precedence",
                "project": cell(json!("straylight"), "owner"),
                "soft_due": cell(json!("2026-08-30"), "agent:first"),
                "required_contexts": cell(json!(["home"]), "todoist")
            }
        });
        let as_of = "2026-08-27T08:00:00Z".parse().unwrap();
        assert!(
            apply_sourced_field(
                &mut metadata,
                "project",
                json!("other"),
                "agent:later",
                as_of,
                None,
                true,
            )
            .is_err(),
            "even an explicit agent correction cannot overwrite owner state"
        );
        assert!(
            apply_sourced_field(
                &mut metadata,
                "soft_due",
                json!("2026-09-01"),
                "agent:later",
                as_of,
                None,
                false,
            )
            .is_err(),
            "agent state requires a recorded correction"
        );
        let correction = apply_sourced_field(
            &mut metadata,
            "soft_due",
            json!("2026-09-01"),
            "agent:later",
            as_of,
            Some("owner corrected the inference"),
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(correction.previous_source.as_deref(), Some("agent:first"));
        assert_eq!(correction.corrected_source, "agent:later");
        apply_sourced_field(
            &mut metadata,
            "required_contexts",
            json!(["phone"]),
            "todoist",
            as_of,
            None,
            false,
        )
        .expect("Todoist refreshes a still-Todoist field");
        let todoist_takeover = apply_sourced_field(
            &mut metadata,
            "required_contexts",
            json!(["online"]),
            "agent:codex",
            as_of,
            None,
            false,
        )
        .expect("an agent can take authority from Todoist")
        .expect("the caller must persist the Todoist-to-agent correction");
        assert_eq!(todoist_takeover.previous_source.as_deref(), Some("todoist"));
        assert_eq!(todoist_takeover.corrected_source, "agent:codex");
        assert!(
            apply_sourced_field(
                &mut metadata,
                "required_contexts",
                json!(["phone"]),
                "todoist",
                as_of,
                None,
                false,
            )
            .is_err(),
            "Todoist cannot regain authority from an agent"
        );
    }

    #[test]
    fn context_normalization_and_near_match_rules_are_deterministic() {
        assert_eq!(normalize_slug("  Home Office  ").unwrap(), "home-office");
        assert_eq!(normalize_slug("Téléphone").unwrap(), "telephone");
        assert!(shared_token("home-office", "office-nyx"));
        assert_eq!(damerau_levenshtein("errands", "erands"), 1);
        assert_eq!(damerau_levenshtein("phone", "phnoe"), 1);
        assert!(damerau_levenshtein("phone", "workshop") > 2);
    }

    #[test]
    fn todoist_refresh_and_occurrence_order_preserve_local_authority() {
        let todoist = cell(json!("remote"), "todoist");
        let owner = cell(json!("local"), "owner");
        let agent = cell(json!("local"), "agent:codex");
        assert!(todoist_refresh_allowed(Some(&todoist)));
        assert!(todoist_refresh_allowed(None));
        assert!(!todoist_refresh_allowed(Some(&owner)));
        assert!(!todoist_refresh_allowed(Some(&agent)));

        assert!(todoist_should_repoint(None, "2026-09-01"));
        assert!(todoist_should_repoint(Some("2026-08-25"), "2026-09-01"));
        assert!(todoist_should_repoint(Some("2026-09-01"), "2026-09-01"));
        assert!(todoist_should_repoint(Some("2026-09-08"), "2026-09-01"));
        assert!(!todoist_should_repoint(
            Some("review:0198f000-0000-7000-8000-000000000001"),
            "2026-09-01"
        ));
        assert_eq!(
            todoist_task_url("6X4F7q8v2R9p3M5N"),
            "https://app.todoist.com/app/task/6X4F7q8v2R9p3M5N"
        );

        let now: DateTime<Utc> = "2026-08-27T12:00:00Z".parse().unwrap();
        let task_id = Uuid::now_v7();
        let mut owner_dropped = json!({
            "kind":"task","schema":TASK_SCHEMA,"task":{
                "id":task_id,"title":"Owner decision",
                "status":cell(json!("dropped"),"owner"),
                "dropped_reason":cell(json!("not doing this"),"owner"),
                "dropped_at":now,
            }
        });
        assert!(
            !set_todoist_terminal(&mut owner_dropped, TodoistTerminal::Completed, now,).unwrap()
        );
        assert_eq!(
            string_value(
                direct_task_object_mut(&mut owner_dropped).unwrap(),
                "status"
            )
            .unwrap()
            .as_deref(),
            Some("dropped")
        );
        assert_eq!(
            string_value(
                direct_task_object_mut(&mut owner_dropped).unwrap(),
                "dropped_reason"
            )
            .unwrap()
            .as_deref(),
            Some("not doing this")
        );

        let mut owner_open = json!({
            "kind":"task","schema":TASK_SCHEMA,"task":{
                "id":task_id,"title":"Owner kept open",
                "status":cell(json!("open"),"agent:codex"),
            }
        });
        assert!(!set_todoist_terminal(&mut owner_open, TodoistTerminal::Deleted, now,).unwrap());
        assert_eq!(
            string_value(direct_task_object_mut(&mut owner_open).unwrap(), "status")
                .unwrap()
                .as_deref(),
            Some("open")
        );
    }

    #[test]
    fn project_interest_obeys_exact_seven_fourteen_and_sixty_day_boundaries() {
        let as_of: DateTime<Utc> = "2026-08-27T12:00:00Z".parse().unwrap();
        assert_eq!(
            derive_project_interest(
                Some((
                    "parked",
                    as_of - chrono::Duration::days(14) + chrono::Duration::seconds(1)
                )),
                Some(as_of),
                as_of,
            ),
            ProjectInterest::Parked,
            "an explicit setting wins until the full 14-day window elapses"
        );
        assert_eq!(
            derive_project_interest(
                Some(("parked", as_of - chrono::Duration::days(14))),
                Some(as_of),
                as_of,
            ),
            ProjectInterest::Hot,
            "at 14 days the explicit setting decays to derived interest"
        );
        assert_eq!(
            derive_project_interest(None, Some(as_of - chrono::Duration::days(7)), as_of),
            ProjectInterest::Hot,
            "activity exactly seven days old is still hot"
        );
        assert_eq!(
            derive_project_interest(
                None,
                Some(as_of - chrono::Duration::days(7) - chrono::Duration::seconds(1)),
                as_of,
            ),
            ProjectInterest::Normal
        );
        assert_eq!(
            derive_project_interest(
                None,
                Some(as_of - chrono::Duration::days(60) + chrono::Duration::seconds(1)),
                as_of,
            ),
            ProjectInterest::Normal
        );
        assert_eq!(
            derive_project_interest(None, Some(as_of - chrono::Duration::days(60)), as_of),
            ProjectInterest::Parked,
            "at 60 days without activity a project is parked"
        );
        assert_eq!(
            derive_project_interest(None, None, as_of),
            ProjectInterest::Parked
        );
        assert_eq!(
            derive_project_interest(
                Some(("hot", as_of + chrono::Duration::seconds(1))),
                Some(as_of + chrono::Duration::seconds(1)),
                as_of
            ),
            ProjectInterest::Parked,
            "future activity and overrides do not affect deterministic as_of state",
        );
    }
}
