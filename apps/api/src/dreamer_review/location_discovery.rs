//! Fenced location source admission. Historical context is filtered before
//! matching/snippets; only the wrapper can retain independently fetched pages.
use super::*;
use std::sync::LazyLock;

static SENSITIVE_EXCERPT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
    r"(?i)(-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----|\b(api[_ -]?key|access[_ -]?token|refresh[_ -]?token|password)\s*[:=])|[A-Za-z0-9_-]{40,}"
).expect("fixed sensitive excerpt pattern")
});

fn query_pattern(query: &str) -> String {
    format!(
        r"(^|[^[:alnum:]_]){}([^[:alnum:]_]|$)",
        regex::escape(query.trim())
    )
}

fn excluded(path: &str, metadata: &Value) -> bool {
    input_excluded(path, metadata["kind"].as_str())
        || path.starts_with("memory/evidence/")
        || path.starts_with("artifacts/")
        || path.starts_with("Evidence/Location/")
        || crate::dreamer_summary::protected_metadata(metadata)
        || evaluation_metadata(metadata)
}

fn evaluation_metadata(value: &Value) -> bool {
    match value {
        Value::Object(values) => values.iter().any(|(key, value)| {
            ([
                "evaluation_output",
                "exclude_from_same_day_evaluation_inputs",
            ]
            .contains(&key.as_str())
                && value == true)
                || evaluation_metadata(value)
        }),
        Value::Array(values) => values.iter().any(evaluation_metadata),
        _ => false,
    }
}

fn selectors(content: &str, queries: &[String]) -> Option<(usize, usize, String)> {
    let lines: Vec<_> = content.lines().collect();
    let patterns: Vec<_> = queries
        .iter()
        .map(|q| {
            regex::RegexBuilder::new(&query_pattern(q))
                .case_insensitive(true)
                .build()
                .expect("escaped literal query")
        })
        .collect();
    let center = lines
        .iter()
        .position(|line| patterns.iter().any(|pattern| pattern.is_match(line)))?;
    let start = center.saturating_sub(2);
    let mut end = (center + 4).min(lines.len());
    while end > center + 1 && lines[start..end].join("\n").len() > 1800 {
        end -= 1;
    }
    let excerpt = lines[start..end].join("\n");
    (!excerpt.is_empty() && excerpt.len() <= 1800 && !SENSITIVE_EXCERPT.is_match(&excerpt))
        .then_some((start + 1, end, excerpt))
}

async fn context(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    work: &Value,
    frozen: i64,
    queries: &[String],
) -> ApiResult<Vec<Value>> {
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    // The start of the day is deliberately conservative: same-day dialogue
    // and all later reconstructions are not an answer key. Select the latest
    // historical version first; do not search today's index then hide hits.
    let cutoff = DateTime::parse_from_rfc3339(string(work, "from")?)
        .map_err(|_| ApiError::invalid("invalid historical context boundary"))?
        .to_utc();
    let patterns: Vec<_> = queries.iter().map(|q| query_pattern(q)).collect();
    let rows = sqlx::query(r#"
        WITH historical AS MATERIALIZED (
            SELECT e.id,e.path,e.current_version,v.version,v.content,v.content_sha256,v.metadata,
                   v.created_at,current_v.metadata AS current_metadata
            FROM brunn.entries e
            JOIN brunn.entry_versions current_v ON current_v.user_id=e.user_id
                AND current_v.entry_id=e.id AND current_v.version=e.current_version
            CROSS JOIN LATERAL (
                SELECT v.* FROM brunn.entry_versions v
                WHERE v.user_id=e.user_id AND v.entry_id=e.id AND v.created_at<$2
                  AND EXISTS(SELECT 1 FROM brunn.workspace_changes c WHERE c.user_id=v.user_id
                    AND c.entry_id=v.entry_id AND c.entry_version=v.version AND c.generation<=$4)
                ORDER BY v.version DESC LIMIT 1
            ) v
            WHERE e.user_id=$1 AND e.deleted_at IS NULL AND e.kind='markdown'
                AND e.path NOT LIKE 'dreams/%' AND e.path NOT LIKE 'derived/%'
                AND e.path NOT LIKE '.brunn/%' AND e.path NOT LIKE 'agent-memory/%'
                AND e.path NOT LIKE 'memory/evidence/%' AND e.path NOT LIKE 'artifacts/%'
                AND e.path NOT LIKE 'Location/%' AND e.path NOT LIKE 'Evidence/Location/%'
                AND e.path <> 'private/dreamer.md'
                AND e.path !~* $5
                AND v.content IS NOT NULL AND v.size_bytes<=1048576
                AND NOT (v.metadata ?| ARRAY['dreamer_summary','dreamer_run','dreamer_review','dreamer_state','dreamer_receipt'])
                AND NOT (current_v.metadata ?| ARRAY['dreamer_summary','dreamer_run','dreamer_review','dreamer_state','dreamer_receipt'])
                AND v.metadata::text NOT LIKE '%"evaluation_output": true%'
                AND current_v.metadata::text NOT LIKE '%"evaluation_output": true%'
                AND v.metadata::text NOT LIKE '%"exclude_from_same_day_evaluation_inputs": true%'
                AND current_v.metadata::text NOT LIKE '%"exclude_from_same_day_evaluation_inputs": true%'
        ), matched AS MATERIALIZED (
            SELECT h.*, ARRAY(SELECT q FROM unnest($3::text[]) q
                WHERE h.content ~* q) AS matches
            FROM historical h
        ) SELECT * FROM matched WHERE cardinality(matches)>0
          ORDER BY cardinality(matches) DESC,created_at DESC,id LIMIT 32
    "#).bind(auth.user_id.0).bind(cutoff).bind(patterns).bind(frozen).bind(SENSITIVE_INPUT_PATH)
        .fetch_all(&mut **tx).await?;
    let mut sources = Vec::new();
    for row in rows {
        let path: String = row.get("path");
        if excluded(&path, &row.get::<Value, _>("metadata"))
            || excluded(&path, &row.get::<Value, _>("current_metadata"))
        {
            continue;
        }
        let content: String = row.get("content");
        let Some((start, end, excerpt)) = selectors(&content, queries) else {
            continue;
        };
        sources.push(json!({"entry_ref":format!("entry:{}",row.get::<Uuid,_>("id")),
            "version":row.get::<i64,_>("version"),"current_version":row.get::<i64,_>("current_version"),
            "path":path,"start_line":start,"end_line":end,"excerpt":excerpt,
            "content_hash":format!("sha256:{}",row.get::<String,_>("content_sha256")),
            "discovery_origin":"historical_context","context_before":cutoff}));
        if sources.len() == 4 {
            break;
        }
    }
    Ok(sources)
}

pub(super) async fn discover(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let auth = runner_auth(&auth)?;
    let queries: Vec<String> =
        serde_json::from_value(body.get("context_queries").cloned().unwrap_or(json!([])))?;
    let web: Vec<Value> =
        serde_json::from_value(body.get("web_sources").cloned().unwrap_or(json!([])))?;
    if queries.len() > 8
        || queries.iter().any(|q| q.trim().len() < 2 || q.len() > 160)
        || web.len() > 8
    {
        return Err(ApiError::invalid("location discovery bounds exceeded"));
    }
    let mut tx = begin_runner_write(&state, &auth).await?;
    if mode(&mut tx, auth.user_id.0).await?.is_none() {
        return Err(ApiError::invalid("CONTROL is paused"));
    }
    let (mut data, version) = load_state(&mut tx, auth.user_id.0).await?;
    let a = data
        .active
        .clone()
        .ok_or_else(|| ApiError::invalid("no active attempt"))?;
    let mut work = a
        .location_work
        .clone()
        .ok_or_else(|| ApiError::invalid("no historical day admitted"))?;
    let request_hash = digest(&json!({"queries":queries,"web_sources":web}));
    let replay = work["discovery"]["request_hash"] == request_hash;
    let mut checked = body.clone();
    if replay {
        checked["expected_state_version"] = json!(version);
    }
    active(&data, &checked, &auth, version)?;
    if replay {
        let response = admission_response(&mut tx, &auth, &data, version).await?;
        return Ok(Json(json!({"data":response,"no_op":true})));
    }
    if work.get("discovery").is_some() {
        return Err(ApiError::invalid(
            "discovery already sealed for this attempt",
        ));
    }
    let mut sources = context(&mut tx, &auth, &work, a.frozen_generation, &queries).await?;
    crate::db::lock_workspace_commit(&mut tx, auth.user_id.0).await?;
    let mut seen = std::collections::BTreeSet::new();
    for page in &web {
        let lookup = crate::dreamer::discovery::Lookup {
            url: string(page, "url")?.into(),
            quote: string(page, "quote")?.into(),
        };
        crate::dreamer::discovery::validate_lookup(&lookup).map_err(ApiError::invalid)?;
        let fetched = DateTime::parse_from_rfc3339(string(page, "fetched_at")?)
            .map_err(|_| ApiError::invalid("invalid lookup fetch time"))?
            .to_utc();
        let hash = string(page, "body_sha256")?;
        if page["verification"] != "fetched_exact_quote"
            || fetched < a.started_at
            || fetched > Utc::now() + Duration::seconds(30)
            || !hash
                .strip_prefix("sha256:")
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            || !seen.insert(lookup.url.clone())
        {
            return Err(ApiError::invalid(
                "lookup verification receipt is invalid or duplicated",
            ));
        }
        let path = format!(
            "Evidence/Location/{}/{}.md",
            work["date"].as_str().unwrap_or(""),
            digest(&lookup)
        );
        let content = format!(
            "# Public place source\n\nURL: {}\nFetched: {}\nVerification: fetched exact quotation\n\n> {}\n",
            lookup.url,
            fetched.to_rfc3339(),
            lookup.quote
        );
        let old = load_entry(&mut tx, auth.user_id.0, &path).await?;
        let receipt = put_entry(&state,&mut tx,&auth,&path,content.clone(),
            json!({"kind":"location_lookup","location_lookup":{"schema":"location.lookup.v1","attempt_id":a.attempt_id,"producer_credential_id":auth.credential_id.0,"evidence_fingerprint":work["fingerprint"],"source":page}}),
            old.as_ref().map_or(0,|e|e.version)).await?;
        let mut source = receipt;
        source["start_line"] = json!(1);
        source["end_line"] = json!(content.lines().count());
        source["excerpt"] = json!(content.trim_end());
        source["discovery_origin"] = json!("verified_web");
        source["expires_at"] = json!(fetched + Duration::days(30));
        sources.push(source);
    }
    let total: usize = sources
        .iter()
        .map(|s| s["excerpt"].as_str().map_or(0, str::len))
        .sum();
    if total > 12_000 {
        return Err(ApiError::invalid(
            "location discovery excerpts exceed bound",
        ));
    }
    work["context_sources"] = json!(sources);
    work["discovery"] = json!({"schema":"location.discovery.v1","request_hash":request_hash,
        "context_before":work["from"],"queries":queries,"verified_web_count":web.len(),"context_count":sources.len()-web.len(),"completed_at":Utc::now()});
    let generation: i64 = sqlx::query_scalar(
        "SELECT coalesce(max(generation),0) FROM brunn.workspace_changes WHERE user_id=$1",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?;
    data.active.as_mut().expect("active").frozen_generation = generation.max(a.frozen_generation);
    data.active.as_mut().expect("active").location_work = Some(work);
    let version = save_state(&state, &mut tx, &auth, &data, version).await?;
    let response = admission_response(&mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    Ok(Json(json!({"data":response})))
}

/// Queue the latest closed local day automatically. Explicit historical work,
/// retained proposals and owner dispositions take precedence.
pub(super) async fn queue_latest(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    data: &mut RunState,
    date: &str,
) -> ApiResult<()> {
    if !data.location_work.is_empty() {
        return Ok(());
    }
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| ApiError::invalid("invalid run date"))?
        .pred_opt()
        .ok_or_else(|| ApiError::invalid("invalid prior day"))?;
    if data
        .location_scopes
        .iter()
        .any(|w| w["date"] == date.to_string())
    {
        return Ok(());
    }
    let from = Los_Angeles
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .to_utc();
    let to = Los_Angeles
        .from_local_datetime(&date.succ_opt().unwrap().and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .to_utc();
    if to > Utc::now() || from < Utc::now() - Duration::days(30) {
        return Ok(());
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM brunn.location_reports WHERE user_id=$1 AND at>=$2 AND at<$3)",
    )
    .bind(user)
    .bind(from)
    .bind(to)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        data.location_work.push(
            json!({"date":date.to_string(),"timezone":"America/Los_Angeles","from":from,"to":to}),
        );
    }
    Ok(())
}
