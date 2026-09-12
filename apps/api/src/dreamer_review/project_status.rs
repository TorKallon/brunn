//! Nightly project status judged by the Dreamer. A disposable daily signal on
//! the project record itself: no review candidates, no CONTROL or decision
//! state, applied in both CONTROL modes under the active attempt fence.
use super::*;
use crate::{
    dreamer::project_status::{Assignment, MAX_REASON_CHARS, STATUSES},
    task_engine::ProjectInterest,
    task_service::{derive_project_interest, effective_metadata, owner_local_date_in_tx},
};

const MAX_TASKS: usize = 40;
const HUB_EXCERPT_CHARS: usize = 4000;

/// Runner authority plus the task-table authority the server owns: the runner
/// credential itself never carries task capabilities.
async fn begin<'a>(
    state: &'a AppState,
    auth: &AuthContext,
    body: &Value,
) -> ApiResult<(Transaction<'a, Postgres>, AuthContext, RunState, i64)> {
    let auth = runner_auth(auth)?;
    let mut tx = begin_runner_write(state, &auth).await?;
    let (data, version) = load_state(&mut tx, auth.user_id.0).await?;
    research::checked_attempt(&data, body, &auth, version, false)?;
    let mut internal = auth.clone();
    for capability in ["read", "save", "task.read", "task.write"] {
        internal.capabilities.insert(capability.into());
    }
    sqlx::query("SELECT set_config('app.capabilities',$1,true)")
        .bind(internal.capability_guc())
        .execute(&mut *tx)
        .await?;
    Ok((tx, auth, data, version))
}

/// The projected provenance map is field -> source; markers are the
/// non-owner sources, as the task engine reports them.
fn provenance_markers(provenance: &Value) -> Vec<String> {
    let mut markers: Vec<String> = provenance
        .as_object()
        .into_iter()
        .flat_map(|fields| fields.values())
        .filter_map(Value::as_str)
        .filter(|source| *source != "owner")
        .map(str::to_owned)
        .collect();
    markers.sort();
    markers.dedup();
    markers
}

pub(super) async fn packet(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let (mut tx, auth, _, _) = begin(&state, &auth, &body).await?;
    let user = auth.user_id.0;
    let now = Utc::now();
    let (today, _) = owner_local_date_in_tx(&mut tx, user, now).await?;
    let projects = sqlx::query(
        r#"
        SELECT project.slug,project.title,project.description,project.interest_override,
               project.interest_set_at,project.last_activity_at,
               project.status,project.status_reason,project.status_since,
               hub.content AS hub_content,checkpoint.metadata AS checkpoint_metadata
        FROM brunn.task_projects AS project
        LEFT JOIN LATERAL(
          SELECT version.content FROM brunn.entries AS entry
          JOIN brunn.entry_versions AS version
            ON version.user_id=entry.user_id AND version.entry_id=entry.id
           AND version.version=entry.current_version
          WHERE entry.user_id=project.user_id AND entry.path=project.hub_path
            AND entry.deleted_at IS NULL
        ) AS hub ON true
        LEFT JOIN LATERAL(
          SELECT version.metadata FROM brunn.task_checkpoint_links AS link
          JOIN brunn.entries AS entry
            ON entry.user_id=link.user_id AND entry.id=link.checkpoint_entry_id
          JOIN brunn.entry_versions AS version
            ON version.user_id=entry.user_id AND version.entry_id=entry.id
           AND version.version=entry.current_version
          WHERE link.user_id=project.user_id AND link.project_slug=project.slug
          ORDER BY version.created_at DESC,link.checkpoint_entry_id DESC LIMIT 1
        ) AS checkpoint ON true
        WHERE project.user_id=$1 AND project.archived_at IS NULL
        ORDER BY project.slug
        "#,
    )
    .bind(user)
    .fetch_all(&mut *tx)
    .await?;
    let tasks = sqlx::query(
        r#"
        SELECT project_slug,title,status,hard_due,hard_due_lead_days,soft_due,ready_at,
               cost_flag,cost_amount_cents,cost_period,cost_since,estimate_minutes,
               waiting_on,parked,provenance
        FROM brunn.task_index
        WHERE user_id=$1 AND project_slug IS NOT NULL AND status IN ('open','waiting')
        ORDER BY project_slug,hard_due ASC NULLS LAST,created_at
        "#,
    )
    .bind(user)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut tasks_by_project = std::collections::BTreeMap::<String, Vec<Value>>::new();
    for row in &tasks {
        let entry = tasks_by_project
            .entry(row.get::<String, _>("project_slug"))
            .or_default();
        if entry.len() == MAX_TASKS {
            continue;
        }
        entry.push(json!({
            "title":row.get::<String,_>("title"),"status":row.get::<String,_>("status"),
            "hard_due":row.get::<Option<DateTime<Utc>>,_>("hard_due"),
            "hard_due_lead_days":row.get::<Option<i32>,_>("hard_due_lead_days"),
            "soft_due":row.get::<Option<NaiveDate>,_>("soft_due"),
            "ready_at":row.get::<Option<DateTime<Utc>>,_>("ready_at"),
            "cost_flag":row.get::<bool,_>("cost_flag"),
            "cost_amount_cents":row.get::<Option<i64>,_>("cost_amount_cents"),
            "cost_period":row.get::<Option<String>,_>("cost_period"),
            "cost_since":row.get::<Option<NaiveDate>,_>("cost_since"),
            "estimate_minutes":row.get::<Option<i32>,_>("estimate_minutes"),
            "waiting_on":row.get::<Option<Value>,_>("waiting_on"),
            "parked":row.get::<bool,_>("parked"),
            "provenance_markers":provenance_markers(&row.get::<Value,_>("provenance")),
        }));
    }
    let projects = projects
        .iter()
        .map(|row| {
            let slug = row.get::<String, _>("slug");
            let interest = derive_project_interest(
                row.get::<Option<String>, _>("interest_override")
                    .as_deref()
                    .zip(row.get::<Option<DateTime<Utc>>, _>("interest_set_at")),
                row.get::<Option<DateTime<Utc>>, _>("last_activity_at"),
                now,
            );
            let hub_excerpt = row
                .get::<Option<String>, _>("hub_content")
                .map(|content| content.chars().take(HUB_EXCERPT_CHARS).collect::<String>());
            let checkpoint = row
                .get::<Option<Value>, _>("checkpoint_metadata")
                .and_then(|metadata| effective_metadata(&metadata).get("checkpoint_state").cloned());
            json!({
                "slug":slug,"title":row.get::<String,_>("title"),
                "description":row.get::<Option<String>,_>("description"),
                "interest":match interest { ProjectInterest::Hot=>"hot",ProjectInterest::Normal=>"normal",ProjectInterest::Parked=>"parked" },
                "hub_excerpt":hub_excerpt,"checkpoint":checkpoint,
                "tasks":tasks_by_project.remove(&slug).unwrap_or_default(),
                "current":{"status":row.get::<String,_>("status"),"reason":row.get::<String,_>("status_reason"),"since":row.get::<Option<NaiveDate>,_>("status_since")},
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({"projects":projects,"today":today})))
}

pub(super) async fn apply(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let assignments: Vec<Assignment> = serde_json::from_value(body["projects"].clone())
        .map_err(|_| ApiError::invalid("projects must be a list of {slug,status,reason}"))?;
    for assignment in &assignments {
        if !STATUSES.contains(&assignment.status.as_str()) {
            return Err(ApiError::invalid(format!(
                "unknown project status for {}",
                assignment.slug
            )));
        }
        let reason = assignment.reason.trim();
        if reason.is_empty() || reason.chars().count() > MAX_REASON_CHARS {
            return Err(ApiError::invalid(format!(
                "project status reason for {} must be 1 to {MAX_REASON_CHARS} characters",
                assignment.slug
            )));
        }
    }
    let (mut tx, auth, mut data, version) = begin(&state, &auth, &body).await?;
    let user = auth.user_id.0;
    let (today, _) = owner_local_date_in_tx(&mut tx, user, Utc::now()).await?;
    let mut transitions = Vec::new();
    for assignment in &assignments {
        let previous: String = sqlx::query_scalar(
            "SELECT status FROM brunn.task_projects WHERE user_id=$1 AND slug=$2 AND archived_at IS NULL FOR UPDATE",
        )
        .bind(user)
        .bind(&assignment.slug)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            ApiError::invalid(format!(
                "{} is not a registered active project",
                assignment.slug
            ))
        })?;
        let reason = assignment.reason.trim();
        if previous == assignment.status {
            sqlx::query("UPDATE brunn.task_projects SET status_reason=$3,status_computed_on=$4 WHERE user_id=$1 AND slug=$2")
                .bind(user).bind(&assignment.slug).bind(reason).bind(today).execute(&mut *tx).await?;
        } else {
            sqlx::query("UPDATE brunn.task_projects SET status=$3,status_previous=$4,status_since=$5,status_reason=$6,status_computed_on=$5 WHERE user_id=$1 AND slug=$2")
                .bind(user).bind(&assignment.slug).bind(&assignment.status).bind(&previous).bind(today).bind(reason).execute(&mut *tx).await?;
            transitions
                .push(json!({"slug":assignment.slug,"from":previous,"to":assignment.status}));
        }
    }
    let summary = json!({"assigned":assignments.len(),"transitions":transitions});
    data.active
        .as_mut()
        .expect("checked attempt")
        .project_status = summary.clone();
    let version = save_state(&state, &mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    let mut response = summary;
    response["state_version"] = json!(version);
    Ok(Json(response))
}
