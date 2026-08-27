use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value, json};
use sqlx::{Postgres, Row, Transaction};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use uuid::Uuid;

use crate::{
    error::{ApiError, ApiResult},
    simple_core,
    task_engine::ProjectInterest,
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
        && as_of.signed_duration_since(set_at) < chrono::Duration::days(14)
    {
        return match interest {
            "hot" => ProjectInterest::Hot,
            "parked" => ProjectInterest::Parked,
            _ => ProjectInterest::Normal,
        };
    }
    match last_activity_at.map(|activity| as_of.signed_duration_since(activity)) {
        Some(age) if age <= chrono::Duration::days(7) => ProjectInterest::Hot,
        Some(age) if age < chrono::Duration::days(60) => ProjectInterest::Normal,
        _ => ProjectInterest::Parked,
    }
}

pub(crate) fn task_id_from_path(path: &str) -> Option<Uuid> {
    let raw = path.strip_prefix(TASK_ENTRY_PREFIX)?.strip_suffix(".md")?;
    let task_id = Uuid::parse_str(raw).ok()?;
    (raw == task_id.to_string()).then_some(task_id)
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

pub(crate) async fn sync_managed_entry_in_tx(
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
        SET archived_at=$3,updated_at=$3
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
        ),updated_at=$4
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
                    .any(|reference| reference.starts_with(&prefix))
                    && best.as_ref().is_none_or(|current| prefix.len() > current.0)
                {
                    best = Some((prefix.len(), slug.clone(), prefix));
                }
            }
        }
        best.map(|(_, slug, matched)| (slug, "path_fallback", Some(matched)))
    };

    if let Some((project_slug, attribution, matched_path)) = match_result {
        let checkpoint_created_at = sqlx::query_scalar::<_, DateTime<Utc>>(
            r#"
            SELECT version.created_at
            FROM straylight.entries AS entry
            JOIN straylight.entry_versions AS version
              ON version.user_id=entry.user_id
             AND version.entry_id=entry.id
             AND version.version=entry.current_version
            WHERE entry.user_id=$1 AND entry.id=$2
            "#,
        )
        .bind(user_id)
        .bind(checkpoint_entry_id)
        .fetch_one(&mut **tx)
        .await?;
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
        sqlx::query(
            r#"
            UPDATE straylight.task_projects
            SET last_activity_at=GREATEST(last_activity_at,$3),
                updated_at=GREATEST(updated_at,$3)
            WHERE user_id=$1 AND slug=$2
            "#,
        )
        .bind(user_id)
        .bind(project_slug)
        .bind(checkpoint_created_at)
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
        .filter(|value| !value.is_empty() && value.len() <= 500)
        .ok_or_else(|| ApiError::invalid("task title must contain 1 to 500 characters"))?
        .to_owned();
    let status = string_value(task, "status")?.unwrap_or_else(|| "open".to_owned());
    if !["open", "waiting", "done", "dropped"].contains(&status.as_str()) {
        return Err(ApiError::invalid(
            "task status must be open, waiting, done, or dropped",
        ));
    }
    let ready_at = timestamp_value(task, "ready_at")?;
    let soft_due = date_value(task, "soft_due")?;
    let hard_due = timestamp_value(task, "hard_due")?;
    let hard_due_lead_days = integer_value(task, "hard_due_lead_days")?
        .map(i32::try_from)
        .transpose()
        .map_err(|_| ApiError::invalid("hard_due_lead_days is out of range"))?;
    let required_contexts = string_array_value(task, "required_contexts")?;
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
        .map_err(|_| ApiError::invalid("estimate_minutes is out of range"))?;
    let waiting_on = owned_value(task, "waiting_on")?;
    if waiting_on.as_ref().is_some_and(|value| !value.is_object()) {
        return Err(ApiError::invalid("waiting_on must be an object"));
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
        .transpose()?;
    if cost.get("flag").and_then(Value::as_bool) == Some(true) {
        return Ok((None, None, true, since));
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
    Ok((Some(amount), Some(period.to_owned()), false, since))
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
        return Ok(Some(Cell {
            value: raw,
            source: None,
            set_at: None,
        }));
    };
    if !object.contains_key("value") {
        return Ok(Some(Cell {
            value: raw,
            source: None,
            set_at: None,
        }));
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
    if object
        .get("note")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        return Err(ApiError::invalid(format!(
            "task {field} cell note must be a string or null"
        )));
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
        || source
            .strip_prefix("agent:")
            .is_some_and(|agent| !agent.is_empty() && !agent.chars().any(char::is_whitespace))
    {
        Ok(())
    } else {
        Err(ApiError::invalid(
            "task field source must be owner, todoist, derived, or agent:<id>",
        ))
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
    use super::*;

    fn cell(value: Value, source: &str) -> Value {
        json!({
            "value": value,
            "source": source,
            "set_at": "2026-08-27T07:00:00Z"
        })
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
    }
}
