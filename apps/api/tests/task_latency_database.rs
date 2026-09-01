use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use sqlx::{AssertSqlSafe, PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

use brunn::{
    auth::{AuthContext, hash_token},
    db::set_context,
    models::{CredentialId, UserId},
    task_service::{TASK_CANDIDATE_PROJECTION_SQL, TASK_PROJECT_STATE_PROJECTION_SQL},
};

const TASK_COUNT: usize = 2_000;
const TIMED_SAMPLE_COUNT: usize = 500;
const WARMUP_SAMPLE_COUNT: usize = 10;
const PLANNER_NOISE_USERS: usize = 9;
const CANDIDATE_P95_LIMIT: Duration = Duration::from_millis(50);
const PROJECT_STATE_P95_LIMIT: Duration = Duration::from_millis(100);

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping task latency database test");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to disposable PostgreSQL");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply Brunn migrations");
    Some(pool)
}

async fn insert_principal(
    tx: &mut Transaction<'_, Postgres>,
) -> (AuthContext, Uuid, Uuid, Uuid, String) {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:task-latency-{scope_id}");
    let token = format!("task-latency-test-token-{credential_id}-secret");
    let capabilities = vec!["task.read".to_owned()];

    sqlx::query("INSERT INTO brunn.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("task-latency-test:{user_id}"))
        .bind("Task latency database test")
        .execute(&mut **tx)
        .await
        .expect("insert task latency test user");
    sqlx::query("INSERT INTO brunn.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind("Task latency database test")
        .execute(&mut **tx)
        .await
        .expect("insert task latency test scope");
    sqlx::query(
        r#"
        INSERT INTO brunn.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind("Task latency database test")
    .bind(hash_token(&token))
    .bind(&capabilities)
    .execute(&mut **tx)
    .await
    .expect("insert task latency test credential");
    sqlx::query(
        r#"
        INSERT INTO brunn.credential_scope_grants (
          credential_id,user_id,scope_id
        ) VALUES ($1,$2,$3)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(&mut **tx)
    .await
    .expect("grant task latency test scope");

    (
        AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(user_id),
            capabilities: capabilities.into_iter().collect::<HashSet<_>>(),
            scope_refs: vec![scope_ref],
            read_only: true,
        },
        user_id,
        credential_id,
        scope_id,
        token,
    )
}

async fn insert_reader_credential(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    label: &str,
) -> String {
    let credential_id = Uuid::now_v7();
    let token = format!("task-latency-{label}-{credential_id}-secret");
    sqlx::query(
        "INSERT INTO brunn.api_credentials(id,user_id,label,token_hash,capabilities) VALUES($1,$2,$3,$4,ARRAY['task.read']::text[])",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(label)
    .bind(hash_token(&token))
    .execute(&mut **tx)
    .await
    .expect("insert HTTP latency reader credential");
    sqlx::query(
        "INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id) VALUES($1,$2,$3)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(&mut **tx)
    .await
    .expect("grant HTTP latency reader scope");
    token
}

async fn delete_fixture_task_rows(pool: &PgPool, user_ids: &[Uuid]) {
    if user_ids.is_empty() {
        return;
    }
    let mut tx = pool.begin().await.expect("begin task latency cleanup");
    // Leave principals in the disposable database until teardown. The live
    // API buffers credential-activity writes, so deleting credentials here
    // can race its asynchronous usage flush after the benchmark completes.
    for statement in [
        "DELETE FROM brunn.task_index WHERE user_id=ANY($1)",
        "DELETE FROM brunn.entry_versions WHERE user_id=ANY($1)",
        "DELETE FROM brunn.entries WHERE user_id=ANY($1)",
    ] {
        sqlx::query(statement)
            .bind(user_ids)
            .execute(&mut *tx)
            .await
            .unwrap_or_else(|error| panic!("task latency cleanup failed for {statement}: {error}"));
    }
    tx.commit().await.expect("commit task latency cleanup");
}

async fn insert_task_fixture(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    as_of: DateTime<Utc>,
) -> Vec<Uuid> {
    let task_ids = (0..TASK_COUNT).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let entry_ids = (0..TASK_COUNT).map(|_| Uuid::now_v7()).collect::<Vec<_>>();

    sqlx::query(
        r#"
        INSERT INTO brunn.task_projects (
          user_id,slug,title,created_by,last_activity_at
        )
        SELECT
          $1,
          format('project-%s',lpad(project_number::text,2,'0')),
          format('Synthetic project %s',project_number),
          'owner',
          $2::timestamptz
        FROM generate_series(0,39) AS projects(project_number)
        "#,
    )
    .bind(user_id)
    .bind(as_of)
    .execute(&mut **tx)
    .await
    .expect("insert synthetic task projects");

    let inserted_entries = sqlx::query(
        r#"
        INSERT INTO brunn.entries (
          id,user_id,path,title,kind,media_type,current_version
        )
        SELECT
          fixture.entry_id,
          $1,
          '.brunn/tasks/' || fixture.task_id::text || '.md',
          format('Synthetic task %s',fixture.ordinal),
          'markdown',
          'text/markdown',
          0
        FROM unnest($2::uuid[],$3::uuid[]) WITH ORDINALITY
          AS fixture(task_id,entry_id,ordinal)
        "#,
    )
    .bind(user_id)
    .bind(&task_ids)
    .bind(&entry_ids)
    .execute(&mut **tx)
    .await
    .expect("insert synthetic task entries");
    assert_eq!(inserted_entries.rows_affected(), TASK_COUNT as u64);

    let inserted_versions = sqlx::query(
        r#"
        WITH fixture AS (
          SELECT task_id,entry_id,ordinal
          FROM unnest($2::uuid[],$3::uuid[]) WITH ORDINALITY
            AS rows(task_id,entry_id,ordinal)
        ), projected AS (
          SELECT
            task_id,
            entry_id,
            ordinal,
            format('Synthetic task %s',ordinal) AS title,
            CASE ordinal % 5
              WHEN 0 THEN 'open'
              WHEN 1 THEN 'waiting'
              WHEN 2 THEN 'done'
              ELSE 'dropped'
            END AS status
          FROM fixture
        )
        INSERT INTO brunn.entry_versions (
          user_id,entry_id,version,content_sha256,content,size_bytes,metadata
        )
        SELECT
          $1,
          entry_id,
          1,
          repeat('0',64),
          '# ' || title || E'\n',
          octet_length('# ' || title || E'\n'),
          jsonb_build_object(
            'kind','task',
            'schema','task.v1',
            'task',jsonb_build_object('id',task_id,'title',title,'status',status)
          )
        FROM projected
        "#,
    )
    .bind(user_id)
    .bind(&task_ids)
    .bind(&entry_ids)
    .execute(&mut **tx)
    .await
    .expect("insert synthetic task entry versions");
    assert_eq!(inserted_versions.rows_affected(), TASK_COUNT as u64);

    sqlx::query("UPDATE brunn.entries SET current_version=1 WHERE user_id=$1 AND id=ANY($2)")
        .bind(user_id)
        .bind(&entry_ids)
        .execute(&mut **tx)
        .await
        .expect("advance synthetic task entry heads");

    let inserted_tasks = sqlx::query(
        r#"
        WITH fixture AS (
          SELECT task_id,entry_id,ordinal
          FROM unnest($2::uuid[],$3::uuid[]) WITH ORDINALITY
            AS rows(task_id,entry_id,ordinal)
        ), projected AS (
          SELECT
            task_id,
            entry_id,
            ordinal,
            format('Synthetic task %s',ordinal) AS title,
            'open'::text AS status,
            false AS parked,
            'project-00'::text AS project_slug,
            '{}'::text[] AS required_contexts
          FROM fixture
        )
        INSERT INTO brunn.task_index (
          user_id,task_id,entry_id,entry_version,title,status,ready_at,
          soft_due,hard_due,hard_due_lead_days,cost_amount_cents,cost_period,
          cost_flag,cost_since,required_contexts,project_slug,estimate_minutes,
          waiting_on,snooze_count,parked,today_pin,triaged_at,done_at,dropped_at,
          provenance,source_timestamps,task,created_at,updated_at
        )
        SELECT
          $1,
          task_id,
          entry_id,
          1,
          title,
          status,
          NULL,
          NULL,
          NULL,
          NULL,
          NULL,
          NULL,
          false,
          NULL,
          required_contexts,
          project_slug,
          NULL,
          NULL,
          0,
          parked,
          NULL,
          $4::timestamptz,
          NULL,
          NULL,
          jsonb_build_object('project','owner'),
          jsonb_build_object('project',$4::timestamptz),
          jsonb_build_object('id',task_id,'title',title,'status',status),
          $4::timestamptz - ordinal * interval '1 minute',
          $4::timestamptz - (ordinal % 120) * interval '1 second'
        FROM projected
        "#,
    )
    .bind(user_id)
    .bind(&task_ids)
    .bind(&entry_ids)
    .bind(as_of)
    .execute(&mut **tx)
    .await
    .expect("insert synthetic task projections");
    assert_eq!(inserted_tasks.rows_affected(), TASK_COUNT as u64);

    let stored_task_count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.task_index WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await
            .expect("count synthetic task projections");
    assert_eq!(stored_task_count, TASK_COUNT as i64);

    task_ids
}

async fn fetch_candidates(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    as_of: DateTime<Utc>,
    available_contexts: &[String],
) -> usize {
    sqlx::query(TASK_CANDIDATE_PROJECTION_SQL)
        .bind(user_id)
        .bind(false)
        .bind(false)
        .bind(as_of)
        .bind(Option::<String>::None)
        .bind(available_contexts)
        .fetch_all(&mut **tx)
        .await
        .expect("fetch task candidate projection")
        .len()
}

async fn fetch_project_state(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    project_slug: &str,
    as_of: DateTime<Utc>,
) -> usize {
    sqlx::query(TASK_PROJECT_STATE_PROJECTION_SQL)
        .bind(user_id)
        .bind(project_slug)
        .bind(as_of)
        .fetch_all(&mut **tx)
        .await
        .expect("fetch project task projection")
        .len()
}

fn p95(samples: &mut [Duration]) -> Duration {
    assert_eq!(samples.len(), TIMED_SAMPLE_COUNT);
    samples.sort_unstable();
    let rank = (samples.len() * 95).div_ceil(100);
    samples[rank - 1]
}

fn live_api_url() -> Option<String> {
    std::env::var("BRUNN_TEST_API_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim_end_matches('/').to_owned())
}

async fn timed_http_get(client: &Client, url: &str, token: &str) -> (Duration, Value) {
    let started = Instant::now();
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .expect("send live task latency request");
    let status = response.status();
    let body = response
        .json::<Value>()
        .await
        .expect("decode live task latency response");
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::OK, "live task request failed: {body}");
    (elapsed, body)
}

async fn measure_http_endpoint(
    client: &Client,
    url: &str,
    token: &str,
    validate: impl Fn(&Value),
) -> Duration {
    for _ in 0..WARMUP_SAMPLE_COUNT {
        let (_, body) = timed_http_get(client, url, token).await;
        validate(&body);
    }
    let mut samples = Vec::with_capacity(TIMED_SAMPLE_COUNT);
    for _ in 0..TIMED_SAMPLE_COUNT {
        let (elapsed, body) = timed_http_get(client, url, token).await;
        validate(&body);
        samples.push(elapsed);
    }
    p95(&mut samples)
}

#[derive(Default)]
struct TaskIndexPlan {
    has_sequential_scan: bool,
    index_names: HashSet<String>,
}

fn inspect_task_index_plan(value: &Value, inspection: &mut TaskIndexPlan) {
    match value {
        Value::Array(values) => {
            for value in values {
                inspect_task_index_plan(value, inspection);
            }
        }
        Value::Object(object) => {
            let relation = object.get("Relation Name").and_then(Value::as_str);
            let node_type = object.get("Node Type").and_then(Value::as_str);
            if relation == Some("task_index")
                && node_type.is_some_and(|value| value.contains("Seq Scan"))
            {
                inspection.has_sequential_scan = true;
            }
            if let Some(index_name) = object.get("Index Name").and_then(Value::as_str)
                && index_name.starts_with("task_index_")
            {
                inspection.index_names.insert(index_name.to_owned());
            }
            for value in object.values() {
                inspect_task_index_plan(value, inspection);
            }
        }
        _ => {}
    }
}

async fn candidate_plan(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    as_of: DateTime<Utc>,
    available_contexts: &[String],
) -> Value {
    let explain =
        format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {TASK_CANDIDATE_PROJECTION_SQL}");
    // The dynamic prefix composes only this crate-owned static deployed SQL.
    sqlx::query_scalar::<_, Value>(AssertSqlSafe(explain.as_str()))
        .bind(user_id)
        .bind(false)
        .bind(false)
        .bind(as_of)
        .bind(Option::<String>::None)
        .bind(available_contexts)
        .fetch_one(&mut **tx)
        .await
        .expect("explain task candidate projection")
}

async fn project_state_plan(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    project_slug: &str,
    as_of: DateTime<Utc>,
) -> Value {
    let explain =
        format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {TASK_PROJECT_STATE_PROJECTION_SQL}");
    // The dynamic prefix composes only this crate-owned static deployed SQL.
    sqlx::query_scalar::<_, Value>(AssertSqlSafe(explain.as_str()))
        .bind(user_id)
        .bind(project_slug)
        .bind(as_of)
        .fetch_one(&mut **tx)
        .await
        .expect("explain project task projection")
}

#[tokio::test]
async fn task_projection_meets_candidate_and_project_state_latency_gates() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let interrupted_fixture_users = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM brunn.users WHERE external_ref LIKE 'task-latency-test:%'",
    )
    .fetch_all(&pool)
    .await
    .expect("find task latency fixtures left by an interrupted test");
    delete_fixture_task_rows(&pool, &interrupted_fixture_users).await;
    let mut setup = pool.begin().await.expect("begin task latency fixture");
    let (auth, user_id, _credential_id, scope_id, first_page_token) =
        insert_principal(&mut setup).await;
    let as_of = Utc::now();
    let task_ids = insert_task_fixture(&mut setup, user_id, as_of).await;
    let late_page_token =
        insert_reader_credential(&mut setup, user_id, scope_id, "late-page").await;
    let project_state_token =
        insert_reader_credential(&mut setup, user_id, scope_id, "project-state").await;
    let mut fixture_user_ids = vec![user_id];
    for _ in 0..PLANNER_NOISE_USERS {
        let (_, noise_user_id, _, _, _) = insert_principal(&mut setup).await;
        insert_task_fixture(&mut setup, noise_user_id, as_of).await;
        fixture_user_ids.push(noise_user_id);
    }
    sqlx::query("ANALYZE brunn.task_index")
        .execute(&mut *setup)
        .await
        .expect("analyze exact deployed task projections");
    setup.commit().await.expect("commit task latency fixture");

    let mut tx = pool.begin().await.expect("begin task latency samples");
    sqlx::query("SET LOCAL ROLE app_ro")
        .execute(&mut *tx)
        .await
        .expect("assume app_ro for latency samples");
    set_context(&mut tx, &auth)
        .await
        .expect("install task.read context for latency samples");

    let available_contexts = vec!["home".to_owned(), "online".to_owned()];
    let project_slug = "project-00";

    for _ in 0..WARMUP_SAMPLE_COUNT {
        assert_eq!(
            fetch_candidates(&mut tx, user_id, as_of, &available_contexts).await,
            TASK_COUNT
        );
        assert_eq!(
            fetch_project_state(&mut tx, user_id, project_slug, as_of).await,
            TASK_COUNT
        );
    }

    let mut candidate_samples = Vec::with_capacity(TIMED_SAMPLE_COUNT);
    for _ in 0..TIMED_SAMPLE_COUNT {
        let started = Instant::now();
        let row_count = fetch_candidates(&mut tx, user_id, as_of, &available_contexts).await;
        candidate_samples.push(started.elapsed());
        assert_eq!(row_count, TASK_COUNT);
    }

    let mut project_state_samples = Vec::with_capacity(TIMED_SAMPLE_COUNT);
    for _ in 0..TIMED_SAMPLE_COUNT {
        let started = Instant::now();
        let row_count = fetch_project_state(&mut tx, user_id, project_slug, as_of).await;
        project_state_samples.push(started.elapsed());
        assert_eq!(row_count, TASK_COUNT);
    }

    let candidate_p95 = p95(&mut candidate_samples);
    let project_state_p95 = p95(&mut project_state_samples);
    eprintln!(
        "task_latency tasks={TASK_COUNT} candidate_samples={TIMED_SAMPLE_COUNT} candidate_p95_ms={:.3} project_state_samples={TIMED_SAMPLE_COUNT} project_state_p95_ms={:.3}",
        candidate_p95.as_secs_f64() * 1_000.0,
        project_state_p95.as_secs_f64() * 1_000.0,
    );
    assert!(
        candidate_p95 <= CANDIDATE_P95_LIMIT,
        "task candidate p95 exceeded 50 ms: {candidate_p95:?}"
    );
    assert!(
        project_state_p95 <= PROJECT_STATE_P95_LIMIT,
        "project state p95 exceeded 100 ms: {project_state_p95:?}"
    );

    let candidate_plan = candidate_plan(&mut tx, user_id, as_of, &available_contexts).await;
    let mut candidate_inspection = TaskIndexPlan::default();
    inspect_task_index_plan(&candidate_plan, &mut candidate_inspection);
    eprintln!(
        "task_candidate_explain seq_scan={} indexes={:?}",
        candidate_inspection.has_sequential_scan, candidate_inspection.index_names
    );
    assert!(
        !candidate_inspection.has_sequential_scan,
        "task candidate query performed a sequential scan on task_index"
    );
    assert!(
        !candidate_inspection.index_names.is_empty(),
        "task candidate query did not use a task_index index"
    );

    let project_plan = project_state_plan(&mut tx, user_id, project_slug, as_of).await;
    let mut project_inspection = TaskIndexPlan::default();
    inspect_task_index_plan(&project_plan, &mut project_inspection);
    eprintln!(
        "task_project_state_explain seq_scan={} indexes={:?}",
        project_inspection.has_sequential_scan, project_inspection.index_names
    );
    assert!(
        !project_inspection.has_sequential_scan,
        "project state query performed a sequential scan on task_index"
    );
    assert!(
        !project_inspection.index_names.is_empty(),
        "project state query did not use a task_index index"
    );

    tx.rollback().await.expect("roll back task latency samples");

    if let Some(base_url) = live_api_url() {
        let client = Client::new();
        let as_of = as_of.to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let first_url = format!(
            "{base_url}/v1/workspace/tasks/candidates?view=all&deliberate_all=true&limit=25&as_of={as_of}"
        );
        // Fallback ordering is oldest first; ordinal 26 is the cursor directly
        // before the final 25-item page in this deliberately skewed fixture.
        let late_cursor = task_ids[25];
        let late_url = format!(
            "{base_url}/v1/workspace/tasks/candidates?view=all&deliberate_all=true&limit=25&cursor={late_cursor}&as_of={as_of}"
        );
        let project_url =
            format!("{base_url}/v1/workspace/projects/project-00/state?as_of={as_of}");
        let first_p95 = measure_http_endpoint(&client, &first_url, &first_page_token, |body| {
            assert_eq!(body["data"]["items"].as_array().map(Vec::len), Some(25));
            assert_eq!(body["data"]["next_remaining"], TASK_COUNT - 25);
            assert!(body["data"]["next_cursor"].is_string());
        })
        .await;
        let late_p95 = measure_http_endpoint(&client, &late_url, &late_page_token, |body| {
            assert_eq!(body["data"]["items"].as_array().map(Vec::len), Some(25));
            assert_eq!(body["data"]["next_remaining"], 0);
            assert!(body["data"]["next_cursor"].is_null());
        })
        .await;
        let project_p95 =
            measure_http_endpoint(&client, &project_url, &project_state_token, |body| {
                assert_eq!(body["data"]["next"].as_array().map(Vec::len), Some(3));
                assert_eq!(body["data"]["rollups"]["open"], TASK_COUNT);
                assert_eq!(body["data"]["waiting_total"], 0);
            })
            .await;
        eprintln!(
            "task_http_latency tasks={TASK_COUNT} samples={TIMED_SAMPLE_COUNT} first_all_p95_ms={:.3} late_all_p95_ms={:.3} project_state_p95_ms={:.3}",
            first_p95.as_secs_f64() * 1_000.0,
            late_p95.as_secs_f64() * 1_000.0,
            project_p95.as_secs_f64() * 1_000.0,
        );
        assert!(
            first_p95 <= CANDIDATE_P95_LIMIT,
            "first deliberate-all HTTP p95 exceeded 50 ms: {first_p95:?}"
        );
        assert!(
            late_p95 <= CANDIDATE_P95_LIMIT,
            "late deliberate-all HTTP p95 exceeded 50 ms: {late_p95:?}"
        );
        assert!(
            project_p95 <= PROJECT_STATE_P95_LIMIT,
            "project-state HTTP p95 exceeded 100 ms: {project_p95:?}"
        );
    } else {
        eprintln!("BRUNN_TEST_API_URL is unset; skipping live task handler latency samples");
    }

    delete_fixture_task_rows(&pool, &fixture_user_ids).await;
}
