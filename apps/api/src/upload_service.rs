//! Multipart-upload hygiene for account lifecycle flows. Account exports
//! stream through multipart uploads and account deletion must abort any
//! in-flight multipart under the purged user prefix before the object-store
//! purge; export compensation itself lives in account_worker.

use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{
    Client,
    config::{Credentials, SharedCredentialsProvider},
};
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::{
    config::Config,
    db::AppState,
    error::{ApiError, ApiResult},
};

static MULTIPART_INVENTORY_CLIENT: OnceCell<Client> = OnceCell::const_new();

#[derive(Clone, Debug)]
struct MultipartInventoryEntry {
    object_key: String,
    upload_id: String,
}

pub async fn abort_multipart_prefix(state: &AppState, prefix: &str) -> ApiResult<usize> {
    validate_user_prefix(prefix)?;
    let uploads = list_multipart_uploads(state, Some(prefix)).await?;
    for upload in &uploads {
        state
            .object_store
            .abort_multipart_upload(&upload.object_key, &upload.upload_id)
            .await?;
    }
    let remaining = list_multipart_uploads(state, Some(prefix)).await?;
    if !remaining.is_empty() {
        return Err(ApiError::Internal(format!(
            "{} multipart uploads remain under the purged account prefix",
            remaining.len()
        )));
    }
    Ok(uploads.len())
}

async fn list_multipart_uploads(
    state: &AppState,
    prefix: Option<&str>,
) -> ApiResult<Vec<MultipartInventoryEntry>> {
    let client = multipart_inventory_client(&state.config).await;
    let mut uploads = Vec::new();
    let mut key_marker = None;
    let mut upload_id_marker = None;
    loop {
        let mut request = client
            .list_multipart_uploads()
            .bucket(&state.config.s3_bucket);
        if let Some(prefix) = prefix {
            request = request.prefix(prefix);
        }
        if let Some(marker) = key_marker.as_deref() {
            request = request.key_marker(marker);
        }
        if let Some(marker) = upload_id_marker.as_deref() {
            request = request.upload_id_marker(marker);
        }
        let output = request.send().await.map_err(|error| {
            ApiError::Internal(format!("multipart upload inventory failed: {error}"))
        })?;
        for upload in output.uploads() {
            let object_key = upload.key().ok_or_else(|| {
                ApiError::Internal("multipart upload inventory omitted an object key".to_owned())
            })?;
            let upload_id = upload.upload_id().ok_or_else(|| {
                ApiError::Internal("multipart upload inventory omitted an upload ID".to_owned())
            })?;
            uploads.push(MultipartInventoryEntry {
                object_key: object_key.to_owned(),
                upload_id: upload_id.to_owned(),
            });
        }
        if !output.is_truncated().unwrap_or(false) {
            break;
        }
        key_marker = output.next_key_marker().map(ToOwned::to_owned);
        upload_id_marker = output.next_upload_id_marker().map(ToOwned::to_owned);
        if key_marker.is_none() {
            return Err(ApiError::Internal(
                "truncated multipart upload inventory omitted its next key marker".to_owned(),
            ));
        }
    }
    Ok(uploads)
}

async fn multipart_inventory_client(config: &Config) -> &'static Client {
    MULTIPART_INVENTORY_CLIENT
        .get_or_init(|| async {
            let mut loader = aws_config::defaults(BehaviorVersion::latest())
                .region(Region::new(config.s3_region.clone()));
            if let (Some(access_key), Some(secret_key)) =
                (&config.s3_access_key, &config.s3_secret_key)
            {
                let credentials = Credentials::new(
                    access_key.clone(),
                    secret_key.clone(),
                    None,
                    None,
                    "brunn-multipart-reconciliation",
                );
                loader = loader.credentials_provider(SharedCredentialsProvider::new(credentials));
            }
            let shared = loader.load().await;
            let mut s3_config = aws_sdk_s3::config::Builder::from(&shared)
                .force_path_style(config.s3_force_path_style);
            if let Some(endpoint) = &config.s3_endpoint {
                s3_config = s3_config.endpoint_url(endpoint);
            }
            Client::from_conf(s3_config.build())
        })
        .await
}

fn validate_user_prefix(prefix: &str) -> ApiResult<Uuid> {
    let user = prefix
        .strip_suffix('/')
        .ok_or_else(|| ApiError::invalid("account object prefix must end in a slash"))?;
    Uuid::parse_str(user)
        .map_err(|_| ApiError::invalid("account object prefix must contain one user UUID"))
}
