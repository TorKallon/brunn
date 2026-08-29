//! Legacy background-job queue executor restored from the pre-simplify
//! worker (checkpoint 97f6f64). The legacy write path still enqueues
//! `index_embeddings` and `asset_description` jobs, and the upload service
//! enqueues `asset_object_cleanup`, but the simplify migration dropped the
//! consumer, leaving the queue permanently stuck. `asset_description` and
//! `asset_object_cleanup` handlers dispatch to the current implementations
//! that were written for this queue.

use pgvector::Vector;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    asset_description,
    db::AppState,
    error::{ApiError, ApiResult},
};

pub async fn process_background_job(state: &AppState, worker_id: &str) -> ApiResult<bool> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let mut transaction = pool.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT id,user_id,scope_id,job_kind,payload,attempts,max_attempts
        FROM straylight.background_jobs
        WHERE status IN ('queued','retry_wait') AND available_at <= clock_timestamp()
        ORDER BY created_at
        FOR UPDATE SKIP LOCKED LIMIT 1
        "#,
    )
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        transaction.rollback().await?;
        return Ok(false);
    };
    let job_id: Uuid = row.try_get("id")?;
    let user_id: Uuid = row.try_get("user_id")?;
    let scope_id: Uuid = row.try_get("scope_id")?;
    let job_kind: String = row.try_get("job_kind")?;
    let payload: Value = row.try_get("payload")?;
    let attempts: i32 = row.try_get("attempts")?;
    let max_attempts: i32 = row.try_get("max_attempts")?;
    sqlx::query(
        r#"
        UPDATE straylight.background_jobs
        SET status='running',attempts=attempts+1,locked_at=clock_timestamp(),locked_by=$1
        WHERE id=$2
        "#,
    )
    .bind(worker_id)
    .bind(job_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    let result = match job_kind.as_str() {
        "index_embeddings" => index_embeddings(state, user_id, scope_id, &payload).await,
        "asset_description" => {
            asset_description::process_job(state, user_id, scope_id, &payload).await
        }
        "asset_object_cleanup" => asset_object_cleanup(state, &payload).await,
        _ => Err(ApiError::invalid(format!(
            "unknown background job kind: {job_kind}"
        ))),
    };
    match result {
        Ok(result) => {
            sqlx::query(
                r#"
                UPDATE straylight.background_jobs
                SET status='succeeded',result=$1,completed_at=clock_timestamp(),
                    locked_at=NULL,locked_by=NULL
                WHERE id=$2
                "#,
            )
            .bind(result)
            .bind(job_id)
            .execute(pool)
            .await?;
        }
        Err(error) => {
            let terminal = attempts + 1 >= max_attempts;
            tracing::warn!(%job_id, %job_kind, ?error, terminal, "background job failed");
            sqlx::query(
                r#"
                UPDATE straylight.background_jobs
                SET status=$1,result=$2,available_at=clock_timestamp()+interval '5 seconds',
                    completed_at=CASE WHEN $1='failed' THEN clock_timestamp() ELSE NULL END,
                    locked_at=NULL,locked_by=NULL
                WHERE id=$3
                "#,
            )
            .bind(if terminal { "failed" } else { "retry_wait" })
            .bind(json!({"error": error.to_string()}))
            .bind(job_id)
            .execute(pool)
            .await?;
        }
    }
    Ok(true)
}

async fn asset_object_cleanup(state: &AppState, payload: &Value) -> ApiResult<Value> {
    let object_key = payload
        .get("object_key")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("asset_object_cleanup requires object_key"))?;
    state.object_store.delete_object(object_key).await?;
    Ok(json!({"removed_object_key": object_key}))
}

async fn index_embeddings(
    state: &AppState,
    user_id: Uuid,
    scope_id: Uuid,
    payload: &Value,
) -> ApiResult<Value> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let chunk_ids: Vec<Uuid> = payload
        .get("chunk_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().and_then(|value| Uuid::parse_str(value).ok()))
        .collect();
    if chunk_ids.is_empty() {
        return Ok(json!({"indexed": 0}));
    }
    let rows = sqlx::query(
        r#"
        SELECT id,content,content_hash FROM straylight.chunks
        WHERE user_id=$1 AND scope_id=$2 AND id=ANY($3)
        ORDER BY id
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(&chunk_ids)
    .fetch_all(pool)
    .await?;
    let contents: Vec<String> = rows
        .iter()
        .map(|row| row.try_get("content"))
        .collect::<Result<_, _>>()?;
    let vectors = state.embedder.embed(&contents).await?;
    if rows.len() != vectors.len() {
        return Err(ApiError::Internal(
            "embedding row count mismatch".to_owned(),
        ));
    }
    for (row, vector) in rows.iter().zip(vectors) {
        let chunk_id: Uuid = row.try_get("id")?;
        let content_hash: String = row.try_get("content_hash")?;
        sqlx::query(
            r#"
            INSERT INTO straylight.embeddings (
              id,user_id,scope_id,target_record_id,granularity,model,
              dimensions,embedding,source_content_hash
            ) VALUES ($1,$2,$3,$4,'chunk',$5,$6,$7,$8)
            ON CONFLICT (
              user_id,target_record_id,granularity,model,source_content_hash
            ) DO NOTHING
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(scope_id)
        .bind(chunk_id)
        .bind(state.embedder.model())
        .bind(state.embedder.dimensions() as i32)
        .bind(Vector::from(vector))
        .bind(content_hash)
        .execute(pool)
        .await?;
    }
    Ok(json!({
        "indexed": rows.len(),
        "provider": state.embedder.provider(),
        "model": state.embedder.model(),
        "degraded": state.embedder.is_degraded()
    }))
}
