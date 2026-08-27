use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

use straylight::{
    auth::AuthContext,
    db::set_context,
    models::{CredentialId, UserId},
};

const TASK_COUNT: usize = 2_000;
const TIMED_SAMPLE_COUNT: usize = 500;
const WARMUP_SAMPLE_COUNT: usize = 10;
const CANDIDATE_P95_LIMIT: Duration = Duration::from_millis(50);
const PROJECT_STATE_P95_LIMIT: Duration = Duration::from_millis(100);

const CANDIDATE_QUERY: &str = r#"
SELECT
  task_id,entry_version,title,status,ready_at,soft_due,hard_due,
  hard_due_lead_days,cost_amount_cents,cost_period,cost_flag,cost_since,
  required_contexts,project_slug,estimate_minutes,waiting_on,snooze_count,
  parked,today_pin,triaged_at,done_at,dropped_at,provenance,created_at,updated_at
FROM straylight.task_index
WHERE user_id=$1
  AND status='open'
  AND NOT parked
  AND (ready_at IS NULL OR ready_at <= $2)
  AND required_contexts <@ $3::text[]
ORDER BY ready_at NULLS FIRST,created_at,task_id
LIMIT 25
"#;

const PROJECT_STATE_QUERY: &str = r#"
SELECT
  task_id,entry_version,title,status,ready_at,soft_due,hard_due,
  hard_due_lead_days,cost_amount_cents,cost_period,cost_flag,cost_since,
  required_contexts,project_slug,estimate_minutes,waiting_on,snooze_count,
  parked,today_pin,triaged_at,done_at,dropped_at,provenance,created_at,updated_at
FROM straylight.task_index
WHERE user_id=$1
  AND project_slug=$2
  AND status IN ('open','waiting')
ORDER BY updated_at DESC,task_id
LIMIT 100
"#;

const CANDIDATE_EXPLAIN_QUERY: &str = r#"
EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
SELECT
  task_id,entry_version,title,status,ready_at,soft_due,hard_due,
  hard_due_lead_days,cost_amount_cents,cost_period,cost_flag,cost_since,
  required_contexts,project_slug,estimate_minutes,waiting_on,snooze_count,
  parked,today_pin,triaged_at,done_at,dropped_at,provenance,created_at,updated_at
FROM straylight.task_index
WHERE user_id=$1
  AND status='open'
  AND NOT parked
  AND (ready_at IS NULL OR ready_at <= $2)
  AND required_contexts <@ $3::text[]
ORDER BY ready_at NULLS FIRST,created_at,task_id
LIMIT 25
"#;

const PROJECT_STATE_EXPLAIN_QUERY: &str = r#"
EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
SELECT
  task_id,entry_version,title,status,ready_at,soft_due,hard_due,
  hard_due_lead_days,cost_amount_cents,cost_period,cost_flag,cost_since,
  required_contexts,project_slug,estimate_minutes,waiting_on,snooze_count,
  parked,today_pin,triaged_at,done_at,dropped_at,provenance,created_at,updated_at
FROM straylight.task_index
WHERE user_id=$1
  AND project_slug=$2
  AND status IN ('open','waiting')
ORDER BY updated_at DESC,task_id
LIMIT 100
"#;

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping task latency database test");
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
        .expect("apply Straylight migrations");
    Some(pool)
}

async fn insert_principal(tx: &mut Transaction<'_, Postgres>) -> (AuthContext, Uuid, Uuid, Uuid) {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:task-latency-{scope_id}");
    let capabilities = vec!["task.read".to_owned()];

    sqlx::query("INSERT INTO straylight.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("task-latency-test:{user_id}"))
        .bind("Task latency database test")
        .execute(&mut **tx)
        .await
        .expect("insert task latency test user");
    sqlx::query("INSERT INTO straylight.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind("Task latency database test")
        .execute(&mut **tx)
        .await
        .expect("insert task latency test scope");
    sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind("Task latency database test")
    .bind(format!("task-latency-test-token-{credential_id}"))
    .bind(&capabilities)
    .execute(&mut **tx)
    .await
    .expect("insert task latency test credential");
    sqlx::query(
        r#"
        INSERT INTO straylight.credential_scope_grants (
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
    )
}

async fn insert_task_fixture(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    as_of: DateTime<Utc>,
) {
    let task_ids = (0..TASK_COUNT).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let entry_ids = (0..TASK_COUNT).map(|_| Uuid::now_v7()).collect::<Vec<_>>();

    sqlx::query(
        r#"
        INSERT INTO straylight.task_projects (
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
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        )
        SELECT
          fixture.entry_id,
          $1,
          '.straylight/tasks/' || fixture.task_id::text || '.md',
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
        INSERT INTO straylight.entry_versions (
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

    sqlx::query("UPDATE straylight.entries SET current_version=1 WHERE user_id=$1 AND id=ANY($2)")
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
            CASE ordinal % 5
              WHEN 0 THEN 'open'
              WHEN 1 THEN 'waiting'
              WHEN 2 THEN 'done'
              ELSE 'dropped'
            END AS status,
            ordinal % 10 = 5 AS parked,
            format('project-%s',lpad((ordinal % 40)::text,2,'0')) AS project_slug,
            CASE ordinal % 4
              WHEN 0 THEN ARRAY['online']::text[]
              WHEN 1 THEN ARRAY['phone','online']::text[]
              WHEN 2 THEN ARRAY['home']::text[]
              ELSE '{}'::text[]
            END AS required_contexts
          FROM fixture
        )
        INSERT INTO straylight.task_index (
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
          CASE
            WHEN ordinal % 20 = 0 THEN NULL
            WHEN ordinal % 10 = 0 THEN $4::timestamptz - interval '1 day'
            ELSE $4::timestamptz + interval '1 day'
          END,
          CASE
            WHEN ordinal % 7 = 0 THEN ($4::timestamptz AT TIME ZONE 'UTC')::date + 3
            ELSE NULL
          END,
          CASE WHEN ordinal % 11 = 0 THEN $4::timestamptz + interval '2 days' ELSE NULL END,
          CASE WHEN ordinal % 11 = 0 THEN 7 ELSE NULL END,
          CASE WHEN ordinal % 13 = 0 THEN 100 ELSE NULL END,
          CASE WHEN ordinal % 13 = 0 THEN 'day' ELSE NULL END,
          ordinal % 17 = 0,
          CASE
            WHEN ordinal % 13 = 0 THEN ($4::timestamptz AT TIME ZONE 'UTC')::date - 3
            ELSE NULL
          END,
          required_contexts,
          project_slug,
          15 + (ordinal % 90)::integer,
          CASE WHEN status = 'waiting' THEN jsonb_build_object('kind','external') ELSE NULL END,
          (ordinal % 3)::integer,
          parked,
          CASE
            WHEN ordinal % 37 = 0 THEN ($4::timestamptz AT TIME ZONE 'UTC')::date
            ELSE NULL
          END,
          CASE WHEN ordinal % 29 = 0 THEN $4::timestamptz - interval '14 days' ELSE NULL END,
          CASE WHEN status = 'done' THEN $4::timestamptz - interval '1 hour' ELSE NULL END,
          CASE WHEN status = 'dropped' THEN $4::timestamptz - interval '1 hour' ELSE NULL END,
          jsonb_build_object('synthetic',true),
          '{}'::jsonb,
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
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM straylight.task_index WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await
            .expect("count synthetic task projections");
    assert_eq!(stored_task_count, TASK_COUNT as i64);

    sqlx::query("ANALYZE straylight.task_index")
        .execute(&mut **tx)
        .await
        .expect("analyze synthetic task projection");
}

async fn fetch_candidates(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    as_of: DateTime<Utc>,
    available_contexts: &[String],
) -> usize {
    sqlx::query(CANDIDATE_QUERY)
        .bind(user_id)
        .bind(as_of)
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
) -> usize {
    sqlx::query(PROJECT_STATE_QUERY)
        .bind(user_id)
        .bind(project_slug)
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
    sqlx::query_scalar::<_, Value>(CANDIDATE_EXPLAIN_QUERY)
        .bind(user_id)
        .bind(as_of)
        .bind(available_contexts)
        .fetch_one(&mut **tx)
        .await
        .expect("explain task candidate projection")
}

async fn project_state_plan(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    project_slug: &str,
) -> Value {
    sqlx::query_scalar::<_, Value>(PROJECT_STATE_EXPLAIN_QUERY)
        .bind(user_id)
        .bind(project_slug)
        .fetch_one(&mut **tx)
        .await
        .expect("explain project task projection")
}

#[tokio::test]
async fn task_projection_meets_candidate_and_project_state_latency_gates() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let mut tx = pool.begin().await.expect("begin task latency fixture");
    let (auth, user_id, _credential_id, _scope_id) = insert_principal(&mut tx).await;
    let as_of = Utc::now();
    insert_task_fixture(&mut tx, user_id, as_of).await;

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
            25
        );
        assert_eq!(
            fetch_project_state(&mut tx, user_id, project_slug).await,
            50
        );
    }

    let mut candidate_samples = Vec::with_capacity(TIMED_SAMPLE_COUNT);
    for _ in 0..TIMED_SAMPLE_COUNT {
        let started = Instant::now();
        let row_count = fetch_candidates(&mut tx, user_id, as_of, &available_contexts).await;
        candidate_samples.push(started.elapsed());
        assert_eq!(row_count, 25);
    }

    let mut project_state_samples = Vec::with_capacity(TIMED_SAMPLE_COUNT);
    for _ in 0..TIMED_SAMPLE_COUNT {
        let started = Instant::now();
        let row_count = fetch_project_state(&mut tx, user_id, project_slug).await;
        project_state_samples.push(started.elapsed());
        assert_eq!(row_count, 50);
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
    assert!(
        !candidate_inspection.has_sequential_scan,
        "task candidate query performed a sequential scan on task_index"
    );
    assert!(
        candidate_inspection
            .index_names
            .contains("task_index_candidates_idx"),
        "task candidate query omitted task_index_candidates_idx; task indexes used: {:?}",
        candidate_inspection.index_names
    );

    let project_plan = project_state_plan(&mut tx, user_id, project_slug).await;
    let mut project_inspection = TaskIndexPlan::default();
    inspect_task_index_plan(&project_plan, &mut project_inspection);
    assert!(
        !project_inspection.has_sequential_scan,
        "project state query performed a sequential scan on task_index"
    );
    assert!(
        project_inspection
            .index_names
            .contains("task_index_project_idx"),
        "project state query omitted task_index_project_idx; task indexes used: {:?}",
        project_inspection.index_names
    );

    tx.rollback().await.expect("roll back task latency fixture");
}
