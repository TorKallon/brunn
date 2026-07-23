use std::str::FromStr;

use sha2::Digest;
use sqlx::{
    PgPool, Postgres, Row, Transaction,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use uuid::Uuid;

use crate::{
    auth::{AuthContext, hash_token},
    config::Config,
    embeddings::{SharedEmbedder, from_config as embedder_from_config},
    error::{ApiError, ApiResult},
    object_store::ObjectStore,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub auth_pool: PgPool,
    pub rw_pool: PgPool,
    pub ro_pool: PgPool,
    pub admin_pool: Option<PgPool>,
    pub embedder: SharedEmbedder,
    pub object_store: ObjectStore,
}

impl AppState {
    pub async fn connect(config: Config) -> ApiResult<Self> {
        let auth_pool = pool(
            &config.database_url_rw,
            config.database_max_connections,
            "straylight-auth",
        )
        .await?;
        let rw_pool = pool(
            &config.database_url_rw,
            config.database_max_connections,
            "straylight-rw",
        )
        .await?;
        let ro_pool = pool(
            &config.database_url_ro,
            config.database_max_connections,
            "straylight-ro",
        )
        .await?;
        let admin_pool = match &config.database_url_admin {
            Some(url) => Some(pool(url, 4, "straylight-worker-admin").await?),
            None => None,
        };
        let embedder = embedder_from_config(&config)?;
        let object_store = ObjectStore::new(&config).await?;
        object_store.ensure_bucket().await?;
        Ok(Self {
            config,
            auth_pool,
            rw_pool,
            ro_pool,
            admin_pool,
            embedder,
            object_store,
        })
    }

    pub async fn begin_read(&self, auth: &AuthContext) -> ApiResult<Transaction<'_, Postgres>> {
        let mut transaction = self.ro_pool.begin().await?;
        set_context(&mut transaction, auth).await?;
        Ok(transaction)
    }

    pub async fn begin_write(&self, auth: &AuthContext) -> ApiResult<Transaction<'_, Postgres>> {
        let mut transaction = self.rw_pool.begin().await?;
        set_context(&mut transaction, auth).await?;
        Ok(transaction)
    }
}

async fn pool(url: &str, max: u32, application_name: &str) -> ApiResult<PgPool> {
    let options = PgConnectOptions::from_str(url)
        .map_err(|error| ApiError::configuration(format!("invalid database URL: {error}")))?
        .application_name(application_name);
    Ok(PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(std::time::Duration::from_secs(15))
        .connect_with(options)
        .await?)
}

pub async fn set_context(
    transaction: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
) -> ApiResult<()> {
    let mut capabilities: Vec<_> = auth.capabilities.iter().cloned().collect();
    capabilities.sort();
    let row = sqlx::query(
        r#"
        SELECT valid, scope_ids
        FROM straylight_auth.validate_transaction_context($1, $2, $3, $4)
        "#,
    )
    .bind(auth.user_id.0)
    .bind(auth.credential_id.0)
    .bind(&capabilities)
    .bind(&auth.scope_refs)
    .fetch_one(&mut **transaction)
    .await?;
    if !row.try_get::<bool, _>("valid")? {
        return Err(ApiError::unauthenticated());
    }
    let scope_ids: Vec<Uuid> = row.try_get("scope_ids")?;
    sqlx::query(
        r#"
        SELECT
          set_config('app.current_user_id', $1, true),
          set_config('app.current_credential_id', $2, true),
          set_config('app.capabilities', $3, true),
          set_config('app.scope_refs', $4, true),
          set_config('app.scope_ids', $5, true),
          set_config('app.context_valid', 'true', true)
        "#,
    )
    .bind(auth.user_id.0.to_string())
    .bind(auth.credential_id.0.to_string())
    .bind(auth.capability_guc())
    .bind(auth.scope_guc())
    .bind(
        scope_ids
            .iter()
            .map(Uuid::to_string)
            .collect::<Vec<_>>()
            .join(","),
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub async fn migrate_and_bootstrap(config: &Config) -> ApiResult<()> {
    let admin_url = config
        .database_url_admin
        .as_deref()
        .ok_or_else(|| ApiError::configuration("DATABASE_URL_ADMIN is required for migrations"))?;
    let admin = pool(admin_url, 2, "straylight-migrate").await?;
    sqlx::migrate!("./migrations").run(&admin).await?;
    bootstrap_dev_identity(&admin, config).await?;
    Ok(())
}

async fn bootstrap_dev_identity(pool: &PgPool, config: &Config) -> ApiResult<()> {
    let Some(read_write_token) = &config.dev_read_write_token else {
        tracing::warn!(
            "STRAYLIGHT_DEV_READ_WRITE_TOKEN is unset; no local credential was bootstrapped"
        );
        return Ok(());
    };

    let write_capabilities = vec![
        "open",
        "query",
        "read",
        "compute",
        "verify",
        "status",
        "checkpoint",
        "save",
        "stage",
        "correct",
        "delete",
        "dream",
        "credential:manage",
    ];
    let (user_id, _, scope_id, _): (Uuid, Uuid, Uuid, Uuid) =
        sqlx::query_as("SELECT * FROM straylight_auth.bootstrap_user($1, $2, $3, $4, $5)")
            .bind(&config.dev_user_ref)
            .bind(&config.dev_user_name)
            .bind("Local read/write")
            .bind(hash_token(read_write_token))
            .bind(&write_capabilities)
            .fetch_one(pool)
            .await?;

    if let Some(read_only_token) = &config.dev_read_only_token {
        let read_capabilities = vec!["open", "query", "read", "compute", "verify", "status"];
        let _: (Uuid, Uuid, Uuid, Uuid) =
            sqlx::query_as("SELECT * FROM straylight_auth.bootstrap_user($1, $2, $3, $4, $5)")
                .bind(&config.dev_user_ref)
                .bind(&config.dev_user_name)
                .bind("Local read-only")
                .bind(hash_token(read_only_token))
                .bind(&read_capabilities)
                .fetch_one(pool)
                .await?;
    }

    let empty_manifest_hash = hex::encode(sha2::Sha256::digest(b"straylight:empty-corpus@v1"));
    let initial_revision_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.corpus_revisions (
          id, user_id, scope_id, parent_revision_id, revision_number, manifest_hash
        )
        SELECT $1, $2, $3, NULL, 1, $4
        WHERE NOT EXISTS (
          SELECT 1
          FROM straylight.corpus_revisions
          WHERE user_id=$2 AND scope_id=$3 AND revision_number=1
        )
        ON CONFLICT (user_id, scope_id, revision_number) DO NOTHING
        "#,
    )
    .bind(initial_revision_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(&empty_manifest_hash)
    .execute(pool)
    .await?;
    let initial_revision_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.corpus_revisions WHERE user_id = $1 AND scope_id = $2 AND revision_number = 1",
    )
    .bind(user_id)
    .bind(scope_id)
    .fetch_one(pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.active_manifests (
          id, user_id, scope_id, active_corpus_revision_id, manifest_hash, generation
        ) VALUES ($1, $2, $3, $4, $5, 1)
        ON CONFLICT (user_id, scope_id) DO NOTHING
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(scope_id)
    .bind(initial_revision_id)
    .bind(&empty_manifest_hash)
    .execute(pool)
    .await?;
    let manifest_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.active_manifests WHERE user_id = $1 AND scope_id = $2",
    )
    .bind(user_id)
    .bind(scope_id)
    .fetch_one(pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.active_manifest_history (
          id, user_id, scope_id, manifest_id, generation, corpus_revision_id,
          manifest_hash, change_kind
        ) VALUES ($1, $2, $3, $4, 1, $5, $6, 'initial')
        ON CONFLICT (user_id, manifest_id, generation) DO NOTHING
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(scope_id)
    .bind(manifest_id)
    .bind(initial_revision_id)
    .bind(empty_manifest_hash)
    .execute(pool)
    .await?;
    Ok(())
}
