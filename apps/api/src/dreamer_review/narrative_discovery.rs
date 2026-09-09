//! Existing workspace search supplies leads; this endpoint admits only exact
//! accessible source versions inside the active attempt's generation fence.
use super::*;
use std::collections::BTreeMap;

const MAX_CONTEXT: usize = 64;

pub(super) async fn discover(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    if body.get("subject_ref").is_some() {
        return research::discover(State(state), Extension(auth), Json(body)).await;
    }
    let auth = runner_auth(&auth)?;
    let plan = crate::dreamer::narrative::parse(
        &json!({"schema":"dream.narrative.discovery.v1","queries":body["queries"]}).to_string(),
    )
    .map_err(ApiError::invalid)?;
    let request_hash = digest(&plan.queries);
    let mut tx = begin_runner_write(&state, &auth).await?;
    if mode(&mut tx, auth.user_id.0).await?.is_none() {
        return Err(ApiError::invalid("CONTROL is paused"));
    }
    let (data, version) = load_state(&mut tx, auth.user_id.0).await?;
    let mut checked = body.clone();
    let replay = data.active.as_ref().is_some_and(|a| {
        a.narrative_discovery
            .as_ref()
            .is_some_and(|d| d["request_hash"] == request_hash)
    });
    if replay {
        checked["expected_state_version"] = json!(version);
    }
    let attempt = active(&data, &checked, &auth, version)?;
    if replay {
        let response = admission_response(&mut tx, &auth, &data, version).await?;
        return Ok(Json(json!({"data":response,"no_op":true})));
    }
    if attempt.narrative_discovery.is_some() {
        return Err(ApiError::invalid(
            "narrative discovery is already frozen for this attempt",
        ));
    }
    // Search never holds the runner's write/lease lock. Recheck the same fence
    // and source heads in a new transaction before retaining any result.
    tx.commit().await?;
    let mut leads = BTreeMap::new();
    let mut order = Vec::new();
    let mut query_results = Vec::new();
    if !plan.queries.is_empty() {
        let results = simple_core::search_headers_for_dreamer(&state, &auth, &plan.queries).await?;
        for result in &results {
            query_results.push(json!({"id":result["id"],"returned":result["candidates"].as_array().map_or(0,Vec::len),
                "query_status":result.get("query_status").cloned().unwrap_or(json!("complete"))}));
        }
        // Fairly interleave subjects and recent/relevant lanes before applying
        // the context cap. No search excerpt or generated answer enters INPUT.
        for rank in 0..8 {
            for result in &results {
                let Some(hit) = result["candidates"].get(rank) else {
                    continue;
                };
                let Some(reference) = hit["reference"].as_str() else {
                    continue;
                };
                let Some(version) = hit["version"].as_i64() else {
                    continue;
                };
                let Ok(id) = entry_id(reference) else {
                    continue;
                };
                if leads.insert(id, version).is_none() {
                    order.push(id);
                }
            }
        }
    }
    let mut tx = begin_runner_write(&state, &auth).await?;
    if mode(&mut tx, auth.user_id.0).await?.is_none() {
        return Err(ApiError::invalid("CONTROL is paused"));
    }
    let (mut data, version) = load_state(&mut tx, auth.user_id.0).await?;
    let current = active(&data, &body, &auth, version)?;
    if current.narrative_discovery.is_some() {
        return Err(conflict(
            "Narrative discovery changed; replay the original request",
            version,
        ));
    }
    let rows = sqlx::query(r#"
        SELECT e.id,e.path,e.current_version,v.metadata,v.content_sha256,c.generation
        FROM brunn.entries e
        JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version
        CROSS JOIN LATERAL (
            SELECT generation,operation FROM brunn.workspace_changes
            WHERE user_id=e.user_id AND entry_id=e.id AND entry_version=e.current_version
            ORDER BY generation DESC LIMIT 1
        ) c
        WHERE e.user_id=$1 AND e.id=ANY($2) AND e.deleted_at IS NULL AND e.kind='markdown'
            AND v.content IS NOT NULL AND v.size_bytes<=1048576
            AND c.generation<=$3 AND c.operation<>'delete'
    "#).bind(auth.user_id.0).bind(&order).bind(attempt.frozen_generation)
        .fetch_all(&mut *tx).await?;
    let mut eligible = BTreeMap::new();
    for row in rows {
        let id: Uuid = row.get("id");
        let version: i64 = row.get("current_version");
        let path: String = row.get("path");
        if leads.get(&id) != Some(&version)
            || location_discovery::excluded(&path, &row.get::<Value, _>("metadata"))
        {
            continue;
        }
        eligible.insert(
            id,
            Input {
                entry_ref: format!("entry:{id}"),
                path,
                version,
                generation: row.get("generation"),
                operation: "context".into(),
                content_hash: format!("sha256:{}", row.get::<String, _>("content_sha256")),
            },
        );
    }
    let eligible_count = eligible.len();
    let context: Vec<_> = order
        .iter()
        .filter_map(|id| eligible.remove(id))
        .take(MAX_CONTEXT)
        .collect();
    let a = data.active.as_mut().expect("active attempt checked");
    a.narrative_context = context;
    a.narrative_discovery = Some(json!({"request_hash":request_hash,"queries":plan.queries,
        "query_results":query_results,"lead_count":leads.len(),"eligible_count":eligible_count,
        "retained_count":a.narrative_context.len(),"context_cap_reached":eligible_count>MAX_CONTEXT,
        "meaning":"Bounded matching source context, not a claim of exhaustive entity coverage."}));
    let version = save_state(&state, &mut tx, &auth, &data, version).await?;
    let response = admission_response(&mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    Ok(Json(json!({"data":response})))
}
