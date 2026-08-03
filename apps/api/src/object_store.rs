use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read},
    path::{Component, Path},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{
    Client,
    config::{Credentials, SharedCredentialsProvider},
    error::ProvideErrorMetadata,
    primitives::ByteStream,
    types::{CompletedMultipartUpload, CompletedPart},
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::{
    config::Config,
    error::{ApiError, ApiResult},
    models::UserId,
};

pub mod backup;

const MAX_STAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_ARCHIVE_FILES: usize = 2_000;
const MAX_ARCHIVE_EXPANDED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARCHIVE_RATIO: u64 = 200;
const MAX_ARCHIVE_PATH_BYTES: usize = 1_024;
const MAX_ARCHIVE_INSPECTION_TIME: Duration = Duration::from_secs(5);
const PHYSICAL_USAGE_CACHE_TTL: Duration = Duration::from_secs(60);
const MAX_PHYSICAL_USAGE_CACHE_ENTRIES: usize = 128;
const MAX_PHYSICAL_USAGE_PREFIX_BYTES: usize = 1_024;
const PHYSICAL_OBJECT_COUNT_SEMANTICS: &str = "physical_object_versions";

#[derive(Clone)]
pub struct ObjectStore {
    client: Client,
    bucket: String,
    create_bucket: bool,
    physical_usage_cache: Arc<Mutex<PhysicalUsageCache>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicalUsageStatus {
    Fresh,
    Stale,
    Unavailable,
}

/// Physical storage retained by S3 under one exact prefix.
///
/// The object count is the number of non-delete object versions, including
/// current, retained, and orphaned versions. Delete markers and incomplete
/// multipart uploads do not contribute to either value.
#[derive(Clone, Debug, Serialize)]
pub struct PhysicalUsageSnapshot {
    pub status: PhysicalUsageStatus,
    pub object_count_semantics: &'static str,
    pub physical_object_versions: Option<u64>,
    pub physical_size_bytes: Option<u64>,
    pub observed_at: Option<DateTime<Utc>>,
}

impl PhysicalUsageSnapshot {
    fn available(
        status: PhysicalUsageStatus,
        physical_object_versions: u64,
        physical_size_bytes: u64,
        observed_at: DateTime<Utc>,
    ) -> Self {
        Self {
            status,
            object_count_semantics: PHYSICAL_OBJECT_COUNT_SEMANTICS,
            physical_object_versions: Some(physical_object_versions),
            physical_size_bytes: Some(physical_size_bytes),
            observed_at: Some(observed_at),
        }
    }

    fn unavailable() -> Self {
        Self {
            status: PhysicalUsageStatus::Unavailable,
            object_count_semantics: PHYSICAL_OBJECT_COUNT_SEMANTICS,
            physical_object_versions: None,
            physical_size_bytes: None,
            observed_at: None,
        }
    }

    fn with_status(&self, status: PhysicalUsageStatus) -> Self {
        let mut snapshot = self.clone();
        snapshot.status = status;
        snapshot
    }
}

#[derive(Debug)]
struct CachedPhysicalUsage {
    snapshot: PhysicalUsageSnapshot,
    refreshed_at: Instant,
    last_accessed_at: Instant,
}

#[derive(Debug, Default)]
struct PhysicalUsageCache {
    entries: HashMap<String, CachedPhysicalUsage>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StoredBlob {
    pub sha256: String,
    pub size_bytes: usize,
    pub object_key: String,
    pub object_version_id: Option<String>,
    pub created: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct StoredFile {
    pub sha256: String,
    pub size_bytes: u64,
    pub object_key: String,
    pub object_version_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PromotedUpload {
    pub file: StoredFile,
    pub temporary_object_version_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MultipartUpload {
    pub object_key: String,
    pub upload_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct UploadedPart {
    pub etag: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ObjectStoreQualification {
    pub status: &'static str,
    pub bucket_versioning: String,
    pub conditional_create: bool,
    pub content_deduplication: bool,
    pub metadata_round_trip: bool,
    pub object_version_ids: bool,
    pub delete_markers: bool,
    pub complete_version_inventory: bool,
    pub multipart_upload_inventory: bool,
    pub exact_version_purge: PurgeResult,
    pub prefix_version_purge: PurgeResult,
}

pub struct ObjectStream {
    pub body: ByteStream,
    pub content_length: Option<i64>,
    pub content_type: Option<String>,
    pub content_range: Option<String>,
    pub etag: Option<String>,
    pub object_version_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ObjectVersionCandidate {
    pub object_key: String,
    pub object_version_id: String,
    pub last_modified_unix_seconds: i64,
    pub delete_marker: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PurgeResult {
    pub versions_deleted: usize,
    pub delete_markers_deleted: usize,
}

impl PurgeResult {
    pub fn total_deleted(self) -> usize {
        self.versions_deleted + self.delete_markers_deleted
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct InventoryEntry {
    pub path: String,
    pub entry_kind: String,
    pub size_bytes: u64,
    pub compressed_size_bytes: Option<u64>,
    pub content_hash: Option<String>,
    pub readable: bool,
    pub quarantined: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ArchiveInventory {
    pub format: Option<String>,
    pub entries: Vec<InventoryEntry>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ArchiveMember {
    pub inventory_index: usize,
    pub bytes: Option<Bytes>,
}

#[derive(Clone, Debug, Default)]
pub struct ArchiveInspection {
    pub inventory: ArchiveInventory,
    pub members: Vec<ArchiveMember>,
}

impl ObjectStore {
    pub async fn new(config: &Config) -> ApiResult<Self> {
        let mut loader = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(config.s3_region.clone()));
        if let (Some(access_key), Some(secret_key)) = (&config.s3_access_key, &config.s3_secret_key)
        {
            let credentials = Credentials::new(
                access_key.clone(),
                secret_key.clone(),
                None,
                None,
                "straylight-config",
            );
            loader = loader.credentials_provider(SharedCredentialsProvider::new(credentials));
        }
        let shared = loader.load().await;
        let mut s3_config =
            aws_sdk_s3::config::Builder::from(&shared).force_path_style(config.s3_force_path_style);
        if let Some(endpoint) = &config.s3_endpoint {
            s3_config = s3_config.endpoint_url(endpoint);
        }
        Ok(Self {
            client: Client::from_conf(s3_config.build()),
            bucket: config.s3_bucket.clone(),
            create_bucket: config.s3_create_bucket,
            physical_usage_cache: Arc::new(Mutex::new(PhysicalUsageCache::default())),
        })
    }

    /// Return a cached physical object-version inventory for one exact prefix.
    ///
    /// A successful scan is reused briefly to keep dashboard reads from
    /// repeatedly walking S3. If a refresh fails, the last successful snapshot
    /// is returned as `stale`; without a prior snapshot the result is explicitly
    /// `unavailable` rather than presenting zero usage.
    pub async fn physical_usage(&self, prefix: &str) -> ApiResult<PhysicalUsageSnapshot> {
        let prefix = exact_physical_usage_prefix(prefix)?;
        if let Some(snapshot) = self.cached_physical_usage(&prefix, false) {
            metrics::counter!(
                "object_store.physical_usage",
                "result" => "cache_hit"
            )
            .increment(1);
            return Ok(snapshot);
        }

        let started = Instant::now();
        match self.scan_physical_usage(&prefix).await {
            Ok(snapshot) => {
                self.cache_physical_usage(prefix, snapshot.clone());
                metrics::counter!(
                    "object_store.physical_usage",
                    "result" => "refreshed"
                )
                .increment(1);
                metrics::histogram!(
                    "object_store.duration_ms",
                    "operation" => "physical_usage_inventory",
                    "result" => "success"
                )
                .record(started.elapsed().as_secs_f64() * 1_000.0);
                Ok(snapshot)
            }
            Err(error) => {
                metrics::counter!(
                    "object_store.physical_usage",
                    "result" => "refresh_error"
                )
                .increment(1);
                metrics::histogram!(
                    "object_store.duration_ms",
                    "operation" => "physical_usage_inventory",
                    "result" => "error"
                )
                .record(started.elapsed().as_secs_f64() * 1_000.0);
                if let Some(snapshot) = self.cached_physical_usage(&prefix, true) {
                    tracing::warn!(
                        prefix,
                        error = %error,
                        "physical object inventory refresh failed; serving the last good snapshot"
                    );
                    Ok(snapshot)
                } else {
                    tracing::warn!(
                        prefix,
                        error = %error,
                        "physical object inventory is unavailable"
                    );
                    Ok(PhysicalUsageSnapshot::unavailable())
                }
            }
        }
    }

    async fn scan_physical_usage(&self, prefix: &str) -> ApiResult<PhysicalUsageSnapshot> {
        let mut physical_object_versions = 0u64;
        let mut physical_size_bytes = 0u64;
        let mut key_marker = None;
        let mut version_id_marker = None;
        let mut seen_page_markers = HashSet::new();
        let mut seen_versions = HashSet::new();

        loop {
            let mut request = self
                .client
                .list_object_versions()
                .bucket(&self.bucket)
                .prefix(prefix);
            if let Some(marker) = key_marker.as_deref() {
                request = request.key_marker(marker);
            }
            if let Some(marker) = version_id_marker.as_deref() {
                request = request.version_id_marker(marker);
            }
            let output = request.send().await.map_err(|error| {
                ApiError::Internal(format!(
                    "physical object inventory could not list versions: {error}"
                ))
            })?;

            for version in output.versions() {
                let key = version.key().filter(|key| !key.is_empty()).ok_or_else(|| {
                    ApiError::Internal(
                        "physical object inventory found a version without a key".to_owned(),
                    )
                })?;
                if !key.starts_with(prefix) {
                    return Err(ApiError::Internal(
                        "physical object inventory returned a version outside its exact prefix"
                            .to_owned(),
                    ));
                }
                let version_id = version
                    .version_id()
                    .filter(|version_id| !version_id.is_empty())
                    .ok_or_else(|| {
                        ApiError::Internal(
                            "physical object inventory found a version without an ID".to_owned(),
                        )
                    })?;
                if !seen_versions.insert((key.to_owned(), version_id.to_owned())) {
                    return Err(ApiError::Internal(
                        "physical object inventory returned a duplicate object version".to_owned(),
                    ));
                }
                let size = version.size().ok_or_else(|| {
                    ApiError::Internal(
                        "physical object inventory found a version without a size".to_owned(),
                    )
                })?;
                let size = u64::try_from(size).map_err(|_| {
                    ApiError::Internal(
                        "physical object inventory found a negative version size".to_owned(),
                    )
                })?;
                physical_object_versions =
                    physical_object_versions.checked_add(1).ok_or_else(|| {
                        ApiError::Internal("physical object version count overflowed".to_owned())
                    })?;
                physical_size_bytes = physical_size_bytes.checked_add(size).ok_or_else(|| {
                    ApiError::Internal("physical object size total overflowed".to_owned())
                })?;
            }

            if !output.is_truncated().unwrap_or(false) {
                break;
            }
            let next = validated_next_version_markers(
                prefix,
                key_marker.as_deref(),
                version_id_marker.as_deref(),
                output.next_key_marker(),
                output.next_version_id_marker(),
            )?;
            if !seen_page_markers.insert(next.clone()) {
                return Err(ApiError::Internal(
                    "physical object inventory repeated pagination markers".to_owned(),
                ));
            }
            key_marker = Some(next.0);
            version_id_marker = next.1;
        }

        Ok(PhysicalUsageSnapshot::available(
            PhysicalUsageStatus::Fresh,
            physical_object_versions,
            physical_size_bytes,
            Utc::now(),
        ))
    }

    fn cached_physical_usage(
        &self,
        prefix: &str,
        allow_stale: bool,
    ) -> Option<PhysicalUsageSnapshot> {
        let now = Instant::now();
        let mut cache = self
            .physical_usage_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = cache.entries.get_mut(prefix)?;
        entry.last_accessed_at = now;
        if now.duration_since(entry.refreshed_at) <= PHYSICAL_USAGE_CACHE_TTL {
            Some(entry.snapshot.with_status(PhysicalUsageStatus::Fresh))
        } else if allow_stale {
            Some(entry.snapshot.with_status(PhysicalUsageStatus::Stale))
        } else {
            None
        }
    }

    fn cache_physical_usage(&self, prefix: String, snapshot: PhysicalUsageSnapshot) {
        let now = Instant::now();
        let mut cache = self
            .physical_usage_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !cache.entries.contains_key(&prefix)
            && cache.entries.len() >= MAX_PHYSICAL_USAGE_CACHE_ENTRIES
            && let Some(oldest_prefix) = cache
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_accessed_at)
                .map(|(prefix, _)| prefix.clone())
        {
            cache.entries.remove(&oldest_prefix);
        }
        cache.entries.insert(
            prefix,
            CachedPhysicalUsage {
                snapshot,
                refreshed_at: now,
                last_accessed_at: now,
            },
        );
    }

    pub async fn ensure_bucket(&self) -> ApiResult<()> {
        let started = Instant::now();
        let missing = match self.client.head_bucket().bucket(&self.bucket).send().await {
            Ok(_) => false,
            Err(error)
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 404) =>
            {
                true
            }
            Err(error) => {
                metrics::counter!(
                    "object_store.operations",
                    "operation" => "ensure_bucket",
                    "result" => "error",
                    "created" => "false"
                )
                .increment(1);
                return Err(ApiError::Internal(format!(
                    "could not inspect object bucket: {error}"
                )));
            }
        };
        if missing {
            if !self.create_bucket {
                metrics::counter!(
                    "object_store.operations",
                    "operation" => "ensure_bucket",
                    "result" => "error",
                    "created" => "false"
                )
                .increment(1);
                return Err(ApiError::configuration(
                    "object bucket is missing and STRAYLIGHT_S3_CREATE_BUCKET is false",
                ));
            }
            self.client
                .create_bucket()
                .bucket(&self.bucket)
                .send()
                .await
                .map_err(|error| {
                    ApiError::Internal(format!("could not create object bucket: {error}"))
                })?;
        }
        metrics::counter!(
            "object_store.operations",
            "operation" => "ensure_bucket",
            "result" => "success",
            "created" => if missing { "true" } else { "false" }
        )
        .increment(1);
        metrics::histogram!(
            "object_store.duration_ms",
            "operation" => "ensure_bucket",
            "result" => "success"
        )
        .record(started.elapsed().as_secs_f64() * 1_000.0);
        Ok(())
    }

    pub async fn ensure_versioned_bucket(&self) -> ApiResult<()> {
        self.ensure_bucket().await?;
        let versioning = self
            .client
            .get_bucket_versioning()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|error| {
                ApiError::configuration(format!(
                    "could not verify required object bucket versioning: {error}"
                ))
            })?;
        let status = versioning
            .status()
            .map(|value| value.as_str())
            .unwrap_or("Unset");
        if status != "Enabled" {
            return Err(ApiError::configuration(format!(
                "object bucket versioning must be Enabled; status={status}"
            )));
        }
        self.client
            .list_object_versions()
            .bucket(&self.bucket)
            .max_keys(1)
            .send()
            .await
            .map_err(|error| {
                ApiError::configuration(format!(
                    "object storage must allow complete version inventory: {error}"
                ))
            })?;
        self.client
            .list_multipart_uploads()
            .bucket(&self.bucket)
            .max_uploads(1)
            .send()
            .await
            .map_err(|error| {
                ApiError::configuration(format!(
                    "object storage must allow multipart upload inventory: {error}"
                ))
            })?;
        Ok(())
    }

    pub async fn put_user_blob(
        &self,
        user_id: UserId,
        content_type: Option<&str>,
        bytes: Bytes,
    ) -> ApiResult<StoredBlob> {
        let started = Instant::now();
        if bytes.len() > MAX_STAGE_BYTES {
            return Err(ApiError::public(
                http::StatusCode::PAYLOAD_TOO_LARGE,
                "stage_limit_exceeded",
                "staged objects are limited to 64 MiB",
            ));
        }
        let digest = hex::encode(Sha256::digest(&bytes));
        let key = format!("{}/blobs/{digest}", user_id.0);
        let mut request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(bytes.clone()));
        if let Some(content_type) = content_type {
            request = request.content_type(content_type);
        }
        let result = request
            .metadata("sha256", &digest)
            .if_none_match("*")
            .send()
            .await;
        let (created, object_version_id) = match result {
            Ok(output) => (
                true,
                exact_object_version_id(output.version_id(), "object upload")?,
            ),
            Err(error)
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 412) =>
            {
                let existing = self
                    .client
                    .head_object()
                    .bucket(&self.bucket)
                    .key(&key)
                    .send()
                    .await
                    .map_err(|head_error| {
                        ApiError::Internal(format!(
                            "could not verify conditionally retained object: {head_error}"
                        ))
                    })?;
                let expected_size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
                let stored_digest = existing
                    .metadata()
                    .and_then(|metadata| metadata.get("sha256"))
                    .map(String::as_str);
                if existing.content_length() != Some(expected_size)
                    || stored_digest != Some(digest.as_str())
                {
                    return Err(ApiError::Internal(
                        "content-addressed object metadata does not match its key".to_owned(),
                    ));
                }
                (
                    false,
                    exact_object_version_id(
                        existing.version_id(),
                        "deduplicated object verification",
                    )?,
                )
            }
            Err(error) => {
                metrics::counter!(
                    "object_store.operations",
                    "operation" => "put",
                    "result" => "error"
                )
                .increment(1);
                return Err(ApiError::Internal(format!("object upload failed: {error}")));
            }
        };
        let outcome = if created { "created" } else { "deduplicated" };
        metrics::counter!(
            "object_store.operations",
            "operation" => "put",
            "result" => outcome
        )
        .increment(1);
        metrics::histogram!(
            "object_store.duration_ms",
            "operation" => "put",
            "result" => outcome
        )
        .record(started.elapsed().as_secs_f64() * 1_000.0);
        metrics::histogram!("object_store.bytes", "operation" => "put").record(bytes.len() as f64);
        Ok(StoredBlob {
            sha256: format!("sha256:{digest}"),
            size_bytes: bytes.len(),
            object_key: key,
            object_version_id: Some(object_version_id),
            created,
        })
    }

    pub async fn put_user_file_blob(
        &self,
        user_id: UserId,
        content_type: &str,
        path: &Path,
    ) -> ApiResult<StoredFile> {
        let started = Instant::now();
        let (digest, size_bytes) = sha256_file(path).await?;
        let key = format!("{}/blobs/{digest}", user_id.0);
        let body = ByteStream::from_path(path).await.map_err(|error| {
            ApiError::Internal(format!("could not open object upload file: {error}"))
        })?;
        let result = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .content_type(content_type)
            .content_length(i64::try_from(size_bytes).unwrap_or(i64::MAX))
            .metadata("sha256", &digest)
            .if_none_match("*")
            .body(body)
            .send()
            .await;
        let (outcome, object_version_id) = match result {
            Ok(output) => (
                "created",
                exact_object_version_id(output.version_id(), "streamed object upload")?,
            ),
            Err(error)
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 412) =>
            {
                let existing = self
                    .client
                    .head_object()
                    .bucket(&self.bucket)
                    .key(&key)
                    .send()
                    .await
                    .map_err(|head_error| {
                        ApiError::Internal(format!(
                            "could not verify conditionally retained object: {head_error}"
                        ))
                    })?;
                let stored_digest = existing
                    .metadata()
                    .and_then(|metadata| metadata.get("sha256"))
                    .map(String::as_str);
                if existing.content_length() != Some(i64::try_from(size_bytes).unwrap_or(i64::MAX))
                    || stored_digest != Some(digest.as_str())
                {
                    return Err(ApiError::Internal(
                        "content-addressed object metadata does not match its key".to_owned(),
                    ));
                }
                (
                    "deduplicated",
                    exact_object_version_id(
                        existing.version_id(),
                        "streamed object deduplication",
                    )?,
                )
            }
            Err(error) => {
                metrics::counter!(
                    "object_store.operations",
                    "operation" => "put_file_blob",
                    "result" => "error"
                )
                .increment(1);
                return Err(ApiError::Internal(format!("object upload failed: {error}")));
            }
        };
        metrics::counter!(
            "object_store.operations",
            "operation" => "put_file_blob",
            "result" => outcome
        )
        .increment(1);
        metrics::histogram!(
            "object_store.duration_ms",
            "operation" => "put_file_blob",
            "result" => outcome
        )
        .record(started.elapsed().as_secs_f64() * 1_000.0);
        metrics::histogram!("object_store.bytes", "operation" => "put_file_blob")
            .record(size_bytes as f64);
        Ok(StoredFile {
            sha256: format!("sha256:{digest}"),
            size_bytes,
            object_key: key,
            object_version_id: Some(object_version_id),
        })
    }

    pub async fn create_multipart_upload(
        &self,
        user_id: UserId,
        upload_ref: uuid::Uuid,
        content_type: &str,
        expected_sha256: &str,
    ) -> ApiResult<MultipartUpload> {
        let digest = expected_sha256.trim_start_matches("sha256:");
        let object_key = format!("{}/uploads/{upload_ref}", user_id.0);
        let output = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(&object_key)
            .content_type(content_type)
            .metadata("sha256", digest)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(?error, "multipart upload creation failed");
                ApiError::Internal("could not create resumable object upload".to_owned())
            })?;
        let upload_id = output
            .upload_id()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ApiError::Internal("object storage returned no multipart upload ID".to_owned())
            })?
            .to_owned();
        metrics::counter!(
            "object_store.operations",
            "operation" => "multipart_create",
            "result" => "success"
        )
        .increment(1);
        Ok(MultipartUpload {
            object_key,
            upload_id,
        })
    }

    pub async fn upload_multipart_part(
        &self,
        object_key: &str,
        upload_id: &str,
        part_number: i32,
        bytes: Bytes,
    ) -> ApiResult<UploadedPart> {
        let size_bytes = bytes.len();
        let output = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(object_key)
            .upload_id(upload_id)
            .part_number(part_number)
            .content_length(i64::try_from(size_bytes).unwrap_or(i64::MAX))
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(?error, part_number, "multipart part upload failed");
                ApiError::Internal("could not persist resumable upload part".to_owned())
            })?;
        let etag = output
            .e_tag()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ApiError::Internal("object storage returned no multipart part ETag".to_owned())
            })?
            .to_owned();
        metrics::counter!(
            "object_store.operations",
            "operation" => "multipart_part",
            "result" => "success"
        )
        .increment(1);
        metrics::histogram!("object_store.bytes", "operation" => "multipart_part")
            .record(size_bytes as f64);
        Ok(UploadedPart { etag })
    }

    pub async fn complete_multipart_upload(
        &self,
        object_key: &str,
        upload_id: &str,
        parts: &[(i32, String)],
    ) -> ApiResult<Option<String>> {
        let completed_parts = parts
            .iter()
            .map(|(part_number, etag)| {
                CompletedPart::builder()
                    .part_number(*part_number)
                    .e_tag(etag)
                    .build()
            })
            .collect();
        let upload = CompletedMultipartUpload::builder()
            .set_parts(Some(completed_parts))
            .build();
        let output = self
            .client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(object_key)
            .upload_id(upload_id)
            .multipart_upload(upload)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(?error, "multipart upload completion failed");
                ApiError::Internal("could not complete resumable object upload".to_owned())
            })?;
        metrics::counter!(
            "object_store.operations",
            "operation" => "multipart_complete",
            "result" => "success"
        )
        .increment(1);
        Ok(Some(exact_object_version_id(
            output.version_id(),
            "multipart upload completion",
        )?))
    }

    pub async fn abort_multipart_upload(&self, object_key: &str, upload_id: &str) -> ApiResult<()> {
        let result = self
            .client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(object_key)
            .upload_id(upload_id)
            .send()
            .await;
        if let Err(error) = result {
            if error
                .as_service_error()
                .and_then(ProvideErrorMetadata::code)
                == Some("NoSuchUpload")
            {
                return Ok(());
            }
            return Err({
                tracing::warn!(?error, "multipart upload abort failed");
                ApiError::Internal("could not abort resumable object upload".to_owned())
            });
        }
        metrics::counter!(
            "object_store.operations",
            "operation" => "multipart_abort",
            "result" => "success"
        )
        .increment(1);
        Ok(())
    }

    pub async fn promote_verified_upload(
        &self,
        user_id: UserId,
        temporary_object_key: &str,
        temporary_object_version_id: Option<&str>,
        expected_sha256: &str,
        expected_size_bytes: u64,
    ) -> ApiResult<PromotedUpload> {
        let digest = expected_sha256.trim_start_matches("sha256:");
        let actual = self
            .hash_stream(temporary_object_key, temporary_object_version_id)
            .await?;
        let actual_object_version_id = exact_object_version_id(
            actual.object_version_id.as_deref(),
            "completed multipart source verification",
        )?;
        if actual.sha256 != digest || actual.size_bytes != expected_size_bytes {
            metrics::counter!(
                "object_store.integrity_failures",
                "failure" => "multipart_content_mismatch"
            )
            .increment(1);
            return Err(ApiError::conflict(
                "content_hash_mismatch",
                "uploaded bytes do not match the declared size and SHA-256",
                serde_json::json!({
                    "expected_size_bytes": expected_size_bytes,
                    "received_size_bytes": actual.size_bytes
                }),
            ));
        }
        let canonical_key = format!("{}/blobs/{digest}", user_id.0);
        let existing = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&canonical_key)
            .send()
            .await;
        let canonical_object_version_id = match existing {
            Ok(head) => {
                let stored_digest = head
                    .metadata()
                    .and_then(|metadata| metadata.get("sha256"))
                    .map(String::as_str);
                if head.content_length()
                    != Some(i64::try_from(expected_size_bytes).unwrap_or(i64::MAX))
                    || stored_digest != Some(digest)
                {
                    return Err(ApiError::Internal(
                        "content-addressed object metadata does not match its key".to_owned(),
                    ));
                }
                Some(exact_object_version_id(
                    head.version_id(),
                    "existing canonical object verification",
                )?)
            }
            Err(error)
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 404) =>
            {
                let copy_source = versioned_copy_source(
                    &self.bucket,
                    temporary_object_key,
                    Some(&actual_object_version_id),
                );
                let copied = self
                    .client
                    .copy_object()
                    .bucket(&self.bucket)
                    .key(&canonical_key)
                    .copy_source(copy_source)
                    .send()
                    .await
                    .map_err(|copy_error| {
                        tracing::warn!(?copy_error, "verified object promotion failed");
                        ApiError::Internal("could not promote verified resumable upload".to_owned())
                    })?;
                let copied_version_id =
                    exact_object_version_id(copied.version_id(), "canonical object promotion")?;
                let head = self
                    .client
                    .head_object()
                    .bucket(&self.bucket)
                    .key(&canonical_key)
                    .version_id(&copied_version_id)
                    .send()
                    .await
                    .map_err(|head_error| {
                        tracing::warn!(?head_error, "promoted object verification failed");
                        ApiError::Internal("could not verify promoted resumable upload".to_owned())
                    })?;
                let stored_digest = head
                    .metadata()
                    .and_then(|metadata| metadata.get("sha256"))
                    .map(String::as_str);
                if head.content_length()
                    != Some(i64::try_from(expected_size_bytes).unwrap_or(i64::MAX))
                    || stored_digest != Some(digest)
                {
                    return Err(ApiError::Internal(
                        "promoted object metadata failed integrity verification".to_owned(),
                    ));
                }
                ensure_requested_version(
                    Some(&copied_version_id),
                    head.version_id(),
                    "promoted object verification",
                )?;
                Some(copied_version_id)
            }
            Err(error) => {
                tracing::warn!(?error, "content-addressed object lookup failed");
                return Err(ApiError::Internal(
                    "could not check resumable upload destination".to_owned(),
                ));
            }
        };
        metrics::counter!(
            "object_store.operations",
            "operation" => "multipart_promote",
            "result" => "success"
        )
        .increment(1);
        Ok(PromotedUpload {
            temporary_object_version_id: Some(actual_object_version_id),
            file: StoredFile {
                sha256: format!("sha256:{digest}"),
                size_bytes: expected_size_bytes,
                object_key: canonical_key,
                object_version_id: canonical_object_version_id,
            },
        })
    }

    pub async fn delete_object(&self, object_key: &str) -> ApiResult<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(object_key)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(?error, "object delete failed");
                ApiError::Internal("could not remove object".to_owned())
            })?;
        Ok(())
    }

    async fn hash_stream(
        &self,
        object_key: &str,
        object_version_id: Option<&str>,
    ) -> ApiResult<StoredFile> {
        let stream = self
            .get_stream_version(object_key, object_version_id)
            .await?;
        let returned_version_id = stream.object_version_id.clone();
        let mut reader = stream.body.into_async_read();
        let mut hasher = Sha256::new();
        let mut size_bytes = 0_u64;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let count = reader.read(&mut buffer).await.map_err(|error| {
                ApiError::Internal(format!("could not verify uploaded object: {error}"))
            })?;
            if count == 0 {
                break;
            }
            size_bytes = size_bytes
                .checked_add(u64::try_from(count).unwrap_or(u64::MAX))
                .ok_or_else(|| ApiError::Internal("uploaded object size overflow".to_owned()))?;
            hasher.update(&buffer[..count]);
        }
        Ok(StoredFile {
            sha256: hex::encode(hasher.finalize()),
            size_bytes,
            object_key: object_key.to_owned(),
            object_version_id: returned_version_id,
        })
    }

    pub async fn verify_object_content(
        &self,
        object_key: &str,
        expected_sha256: &str,
        expected_size_bytes: u64,
    ) -> ApiResult<()> {
        self.verify_object_content_version(object_key, None, expected_sha256, expected_size_bytes)
            .await
    }

    pub async fn verify_object_content_version(
        &self,
        object_key: &str,
        object_version_id: Option<&str>,
        expected_sha256: &str,
        expected_size_bytes: u64,
    ) -> ApiResult<()> {
        let actual = self.hash_stream(object_key, object_version_id).await?;
        if actual.sha256 != expected_sha256.trim_start_matches("sha256:")
            || actual.size_bytes != expected_size_bytes
        {
            metrics::counter!(
                "object_store.integrity_failures",
                "failure" => "streamed_content_mismatch"
            )
            .increment(1);
            return Err(ApiError::conflict(
                "content_hash_mismatch",
                "stored object no longer matches its declared size and SHA-256",
                serde_json::json!({
                    "expected_size_bytes": expected_size_bytes,
                    "received_size_bytes": actual.size_bytes
                }),
            ));
        }
        Ok(())
    }

    pub async fn get(&self, key: &str) -> ApiResult<Bytes> {
        self.get_version(key, None).await
    }

    pub async fn get_version(
        &self,
        key: &str,
        object_version_id: Option<&str>,
    ) -> ApiResult<Bytes> {
        let started = Instant::now();
        let result = async {
            let output = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(key)
                .set_version_id(object_version_id.map(ToOwned::to_owned))
                .send()
                .await
                .map_err(|error| {
                    tracing::warn!(?error, "object-store read failed");
                    ApiError::not_found("asset_not_found", "asset")
                })?;
            ensure_requested_version(object_version_id, output.version_id(), "object-store read")?;
            output
                .body
                .collect()
                .await
                .map(|body| body.into_bytes())
                .map_err(|error| ApiError::Internal(format!("object download failed: {error}")))
        }
        .await;
        let outcome = if result.is_ok() { "success" } else { "error" };
        metrics::counter!(
            "object_store.operations",
            "operation" => "get",
            "result" => outcome
        )
        .increment(1);
        metrics::histogram!(
            "object_store.duration_ms",
            "operation" => "get",
            "result" => outcome
        )
        .record(started.elapsed().as_secs_f64() * 1_000.0);
        if let Ok(bytes) = &result {
            metrics::histogram!("object_store.bytes", "operation" => "get")
                .record(bytes.len() as f64);
        }
        result
    }

    pub async fn verify_object_identity(
        &self,
        key: &str,
        expected_sha256: &str,
        expected_size_bytes: u64,
    ) -> ApiResult<()> {
        self.verify_object_identity_version(key, None, expected_sha256, expected_size_bytes)
            .await
    }

    pub async fn resolve_object_version_id(
        &self,
        key: &str,
        object_version_id: Option<&str>,
    ) -> ApiResult<Option<String>> {
        let head = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .set_version_id(object_version_id.map(ToOwned::to_owned))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(?error, "object version lookup failed");
                ApiError::not_found("asset_not_found", "asset")
            })?;
        ensure_requested_version(
            object_version_id,
            head.version_id(),
            "object version lookup",
        )?;
        Ok(head.version_id().map(ToOwned::to_owned))
    }

    pub async fn verify_object_identity_version(
        &self,
        key: &str,
        object_version_id: Option<&str>,
        expected_sha256: &str,
        expected_size_bytes: u64,
    ) -> ApiResult<()> {
        let head = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .set_version_id(object_version_id.map(ToOwned::to_owned))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(?error, "object identity lookup failed");
                ApiError::not_found("asset_not_found", "asset")
            })?;
        ensure_requested_version(
            object_version_id,
            head.version_id(),
            "object identity lookup",
        )?;
        let stored_digest = head
            .metadata()
            .and_then(|metadata| metadata.get("sha256"))
            .map(String::as_str);
        let expected_digest = expected_sha256.trim_start_matches("sha256:");
        if head.content_length() != Some(i64::try_from(expected_size_bytes).unwrap_or(i64::MAX))
            || stored_digest != Some(expected_digest)
        {
            metrics::counter!(
                "object_store.integrity_failures",
                "failure" => "head_identity_mismatch"
            )
            .increment(1);
            return Err(ApiError::conflict(
                "content_hash_mismatch",
                "object metadata does not match the immutable asset identity",
                serde_json::json!({}),
            ));
        }
        Ok(())
    }

    pub async fn get_prefix(&self, key: &str, max_bytes: usize) -> ApiResult<Bytes> {
        self.get_prefix_version(key, None, max_bytes).await
    }

    pub async fn get_prefix_version(
        &self,
        key: &str,
        object_version_id: Option<&str>,
        max_bytes: usize,
    ) -> ApiResult<Bytes> {
        if max_bytes == 0 {
            return Ok(Bytes::new());
        }
        let end = max_bytes.saturating_sub(1);
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .range(format!("bytes=0-{end}"))
            .set_version_id(object_version_id.map(ToOwned::to_owned))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(?error, "object prefix read failed");
                ApiError::not_found("asset_not_found", "asset")
            })?;
        ensure_requested_version(object_version_id, output.version_id(), "object prefix read")?;
        let bytes = output
            .body
            .collect()
            .await
            .map_err(|error| ApiError::Internal(format!("object prefix download failed: {error}")))?
            .into_bytes();
        if bytes.len() > max_bytes {
            return Err(ApiError::Internal(
                "object storage returned more prefix bytes than requested".to_owned(),
            ));
        }
        Ok(bytes)
    }

    pub async fn get_stream(&self, key: &str) -> ApiResult<ObjectStream> {
        self.get_stream_version(key, None).await
    }

    pub async fn get_stream_version(
        &self,
        key: &str,
        object_version_id: Option<&str>,
    ) -> ApiResult<ObjectStream> {
        self.get_stream_range_version(key, object_version_id, None)
            .await
    }

    pub async fn get_stream_range(
        &self,
        key: &str,
        range: Option<&str>,
    ) -> ApiResult<ObjectStream> {
        self.get_stream_range_version(key, None, range).await
    }

    pub async fn get_stream_range_version(
        &self,
        key: &str,
        object_version_id: Option<&str>,
        range: Option<&str>,
    ) -> ApiResult<ObjectStream> {
        let mut request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .set_version_id(object_version_id.map(ToOwned::to_owned));
        if let Some(range) = range {
            request = request.range(range);
        }
        let output = request.send().await.map_err(|error| {
            tracing::warn!(?error, "object-store streaming read failed");
            ApiError::not_found("asset_not_found", "asset")
        })?;
        ensure_requested_version(
            object_version_id,
            output.version_id(),
            "object-store streaming read",
        )?;
        Ok(ObjectStream {
            body: output.body,
            content_length: output.content_length,
            content_type: output.content_type,
            content_range: output.content_range,
            etag: output.e_tag,
            object_version_id: output.version_id,
        })
    }

    pub async fn put_file(
        &self,
        key: &str,
        content_type: &str,
        path: &Path,
    ) -> ApiResult<StoredFile> {
        let (digest, size_bytes) = sha256_file(path).await?;
        let body = ByteStream::from_path(path).await.map_err(|error| {
            ApiError::Internal(format!("could not open object upload file: {error}"))
        })?;
        let output = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .content_length(i64::try_from(size_bytes).unwrap_or(i64::MAX))
            .metadata("sha256", &digest)
            .body(body)
            .send()
            .await
            .map_err(|error| ApiError::Internal(format!("object upload failed: {error}")))?;
        metrics::counter!(
            "object_store.operations",
            "operation" => "put_file",
            "result" => "success"
        )
        .increment(1);
        metrics::histogram!("object_store.bytes", "operation" => "put_file")
            .record(size_bytes as f64);
        Ok(StoredFile {
            sha256: format!("sha256:{digest}"),
            size_bytes,
            object_key: key.to_owned(),
            object_version_id: output.version_id().map(ToOwned::to_owned),
        })
    }

    pub async fn download_to_path(&self, key: &str, path: &Path) -> ApiResult<StoredFile> {
        self.download_version_to_path(key, None, path).await
    }

    pub async fn download_version_to_path(
        &self,
        key: &str,
        object_version_id: Option<&str>,
        path: &Path,
    ) -> ApiResult<StoredFile> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .set_version_id(object_version_id.map(ToOwned::to_owned))
            .send()
            .await
            .map_err(|error| ApiError::not_found("asset_not_found", &format!("{key}: {error}")))?;
        ensure_requested_version(
            object_version_id,
            output.version_id(),
            "object export download",
        )?;
        let returned_version_id = output.version_id().map(ToOwned::to_owned);
        let mut reader = output.body.into_async_read();
        let mut file = tokio::fs::File::create(path).await.map_err(|error| {
            ApiError::Internal(format!("could not create export file: {error}"))
        })?;
        tokio::io::copy(&mut reader, &mut file)
            .await
            .map_err(|error| ApiError::Internal(format!("object download failed: {error}")))?;
        file.sync_all()
            .await
            .map_err(|error| ApiError::Internal(format!("could not sync export file: {error}")))?;
        let (digest, size_bytes) = sha256_file(path).await?;
        Ok(StoredFile {
            sha256: format!("sha256:{digest}"),
            size_bytes,
            object_key: key.to_owned(),
            object_version_id: returned_version_id,
        })
    }

    pub async fn health_check(&self) -> ApiResult<()> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|error| {
                ApiError::public(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "dependency_unavailable",
                    format!("object storage is unavailable: {error}"),
                )
            })?;
        let versioning = self
            .client
            .get_bucket_versioning()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|error| {
                ApiError::public(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "dependency_unavailable",
                    format!("object storage versioning cannot be verified: {error}"),
                )
            })?;
        let status = versioning
            .status()
            .map(|value| value.as_str())
            .unwrap_or("Unset");
        if status != "Enabled" {
            return Err(ApiError::public(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "dependency_unavailable",
                format!("object storage versioning is not enabled: status={status}"),
            ));
        }
        Ok(())
    }

    pub async fn qualify(&self) -> ApiResult<ObjectStoreQualification> {
        self.ensure_bucket().await?;
        let versioning = self
            .client
            .get_bucket_versioning()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|error| {
                ApiError::Internal(format!("could not inspect bucket versioning: {error}"))
            })?;
        let versioning_status = versioning
            .status()
            .map(|status| status.as_str().to_owned())
            .unwrap_or_else(|| "Unset".to_owned());
        if versioning_status != "Enabled" {
            return Err(ApiError::Internal(format!(
                "object store qualification requires bucket versioning; status={versioning_status}"
            )));
        }
        self.client
            .list_object_versions()
            .bucket(&self.bucket)
            .max_keys(1)
            .send()
            .await
            .map_err(|error| {
                ApiError::Internal(format!(
                    "object store qualification requires complete version inventory access: {error}"
                ))
            })?;
        self.client
            .list_multipart_uploads()
            .bucket(&self.bucket)
            .max_uploads(1)
            .send()
            .await
            .map_err(|error| {
                ApiError::Internal(format!(
                    "object store qualification requires multipart inventory access: {error}"
                ))
            })?;

        let run_id = uuid::Uuid::now_v7();
        let user_id = UserId(run_id);
        let qualification_prefix = format!("qualification/{run_id}/");
        let versioned_key = format!("{qualification_prefix}versioned");
        let payload = Bytes::from(format!("straylight-object-store-qualification:{run_id}"));

        let result = async {
            let first = self
                .put_user_blob(user_id, Some("text/plain"), payload.clone())
                .await?;
            if !first.created {
                return Err(ApiError::Internal(
                    "first conditional object create was not reported as created".to_owned(),
                ));
            }
            let first_object_version_id =
                first.object_version_id.as_deref().ok_or_else(|| {
                    ApiError::Internal(
                        "conditional object create returned no object version ID".to_owned(),
                    )
                })?;
            let second = self
                .put_user_blob(user_id, Some("text/plain"), payload.clone())
                .await?;
            if second.created
                || second.object_key != first.object_key
                || second.object_version_id.as_deref() != Some(first_object_version_id)
            {
                return Err(ApiError::Internal(
                    "repeated content did not deduplicate through conditional create".to_owned(),
                ));
            }
            if self
                .get_version(&first.object_key, Some(first_object_version_id))
                .await?
                != payload
            {
                return Err(ApiError::Internal(
                    "object payload did not round-trip exactly".to_owned(),
                ));
            }

            let first_version = self
                .client
                .put_object()
                .bucket(&self.bucket)
                .key(&versioned_key)
                .metadata("qualification", "first")
                .body(ByteStream::from_static(b"first"))
                .send()
                .await
                .map_err(|error| {
                    ApiError::Internal(format!(
                        "could not write the first qualification version: {error}"
                    ))
                })?;
            let second_version = self
                .client
                .put_object()
                .bucket(&self.bucket)
                .key(&versioned_key)
                .metadata("qualification", "second")
                .body(ByteStream::from_static(b"second"))
                .send()
                .await
                .map_err(|error| {
                    ApiError::Internal(format!(
                        "could not write the second qualification version: {error}"
                    ))
                })?;
            let object_version_ids =
                first_version.version_id().is_some() && second_version.version_id().is_some();
            if !object_version_ids {
                return Err(ApiError::Internal(
                    "versioned writes did not return object version IDs".to_owned(),
                ));
            }

            let head = self
                .client
                .head_object()
                .bucket(&self.bucket)
                .key(&versioned_key)
                .send()
                .await
                .map_err(|error| {
                    ApiError::Internal(format!(
                        "could not inspect qualification metadata: {error}"
                    ))
                })?;
            let metadata_round_trip = head
                .metadata()
                .and_then(|metadata| metadata.get("qualification"))
                .is_some_and(|value| value == "second");
            if !metadata_round_trip {
                return Err(ApiError::Internal(
                    "object metadata did not round-trip exactly".to_owned(),
                ));
            }

            let deleted = self
                .client
                .delete_object()
                .bucket(&self.bucket)
                .key(&versioned_key)
                .send()
                .await
                .map_err(|error| {
                    ApiError::Internal(format!(
                        "could not create a qualification delete marker: {error}"
                    ))
                })?;
            let delete_markers =
                deleted.delete_marker().unwrap_or(false) && deleted.version_id().is_some();
            if !delete_markers {
                return Err(ApiError::Internal(
                    "versioned delete did not create an identified delete marker".to_owned(),
                ));
            }

            let listed = self
                .client
                .list_object_versions()
                .bucket(&self.bucket)
                .prefix(&versioned_key)
                .send()
                .await
                .map_err(|error| {
                    ApiError::Internal(format!(
                        "could not list qualification object versions: {error}"
                    ))
                })?;
            let versions = listed
                .versions()
                .iter()
                .filter(|version| version.key() == Some(versioned_key.as_str()))
                .count();
            let markers = listed
                .delete_markers()
                .iter()
                .filter(|marker| marker.key() == Some(versioned_key.as_str()))
                .count();
            if versions < 2 || markers < 1 {
                return Err(ApiError::Internal(format!(
                    "version listing omitted qualification history: versions={versions} markers={markers}"
                )));
            }

            Ok((
                first.object_key,
                metadata_round_trip,
                object_version_ids,
                delete_markers,
            ))
        }
        .await;

        let blob_key = format!(
            "{}/blobs/{}",
            user_id.0,
            hex::encode(Sha256::digest(&payload))
        );
        let exact_version_purge = self.purge_all_versions(&blob_key).await;
        let prefix_version_purge = self.purge_prefix(&qualification_prefix).await;

        let (object_key, metadata_round_trip, object_version_ids, delete_markers) = result?;
        if object_key != blob_key {
            return Err(ApiError::Internal(
                "content-addressed object key was not deterministic".to_owned(),
            ));
        }
        let exact_version_purge = exact_version_purge?;
        let prefix_version_purge = prefix_version_purge?;
        if exact_version_purge.versions_deleted < 1 || prefix_version_purge.versions_deleted < 2 {
            return Err(ApiError::Internal(
                "qualification cleanup did not remove all expected object versions".to_owned(),
            ));
        }

        Ok(ObjectStoreQualification {
            status: "pass",
            bucket_versioning: versioning_status,
            conditional_create: true,
            content_deduplication: true,
            metadata_round_trip,
            object_version_ids,
            delete_markers,
            complete_version_inventory: true,
            multipart_upload_inventory: true,
            exact_version_purge,
            prefix_version_purge,
        })
    }

    pub async fn purge_all_versions(&self, key: &str) -> ApiResult<PurgeResult> {
        let started = Instant::now();
        let result = self.purge_all_versions_inner(key).await;
        let outcome = if result.is_ok() { "success" } else { "error" };
        metrics::counter!(
            "object_store.operations",
            "operation" => "purge_versions",
            "result" => outcome
        )
        .increment(1);
        metrics::histogram!(
            "object_store.duration_ms",
            "operation" => "purge_versions",
            "result" => outcome
        )
        .record(started.elapsed().as_secs_f64() * 1_000.0);
        if let Ok(summary) = result {
            metrics::histogram!(
                "object_store.deleted_versions",
                "kind" => "object_version"
            )
            .record(summary.versions_deleted as f64);
            metrics::histogram!(
                "object_store.deleted_versions",
                "kind" => "delete_marker"
            )
            .record(summary.delete_markers_deleted as f64);
        }
        result
    }

    pub async fn purge_prefix(&self, prefix: &str) -> ApiResult<PurgeResult> {
        let mut versions = Vec::new();
        let mut markers = Vec::new();
        let mut key_marker = None;
        let mut version_id_marker = None;
        loop {
            let mut request = self
                .client
                .list_object_versions()
                .bucket(&self.bucket)
                .prefix(prefix);
            if let Some(marker) = key_marker.as_deref() {
                request = request.key_marker(marker);
            }
            if let Some(marker) = version_id_marker.as_deref() {
                request = request.version_id_marker(marker);
            }
            let output = request.send().await.map_err(|error| {
                ApiError::Internal(format!("object version listing failed: {error}"))
            })?;
            for version in output.versions() {
                let key = version.key().ok_or_else(|| {
                    ApiError::Internal("object version listing omitted a key".to_owned())
                })?;
                let version_id = version.version_id().ok_or_else(|| {
                    ApiError::Internal("object version listing omitted a version ID".to_owned())
                })?;
                versions.push((key.to_owned(), version_id.to_owned()));
            }
            for marker in output.delete_markers() {
                let key = marker.key().ok_or_else(|| {
                    ApiError::Internal("delete marker listing omitted a key".to_owned())
                })?;
                let version_id = marker.version_id().ok_or_else(|| {
                    ApiError::Internal("delete marker listing omitted a version ID".to_owned())
                })?;
                markers.push((key.to_owned(), version_id.to_owned()));
            }
            if !output.is_truncated().unwrap_or(false) {
                break;
            }
            key_marker = output.next_key_marker().map(ToOwned::to_owned);
            version_id_marker = output.next_version_id_marker().map(ToOwned::to_owned);
            if key_marker.is_none() {
                return Err(ApiError::Internal(
                    "truncated object version listing omitted the next key marker".to_owned(),
                ));
            }
        }

        for (key, version_id) in versions.iter().chain(&markers) {
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(key)
                .version_id(version_id)
                .send()
                .await
                .map_err(|error| {
                    ApiError::Internal(format!("object version deletion failed: {error}"))
                })?;
        }
        let remaining = self.prefix_version_count(prefix).await?;
        if remaining != 0 {
            return Err(ApiError::Internal(format!(
                "{remaining} object versions remain under the purged prefix"
            )));
        }
        Ok(PurgeResult {
            versions_deleted: versions.len(),
            delete_markers_deleted: markers.len(),
        })
    }

    pub async fn stale_object_versions(
        &self,
        before_unix_seconds: i64,
        limit: usize,
    ) -> ApiResult<Vec<ObjectVersionCandidate>> {
        let mut candidates = Vec::new();
        let mut key_marker = None;
        let mut version_id_marker = None;
        loop {
            let mut request = self.client.list_object_versions().bucket(&self.bucket);
            if let Some(marker) = key_marker.as_deref() {
                request = request.key_marker(marker);
            }
            if let Some(marker) = version_id_marker.as_deref() {
                request = request.version_id_marker(marker);
            }
            let output = request.send().await.map_err(|error| {
                ApiError::Internal(format!(
                    "object reconciliation could not list versions: {error}"
                ))
            })?;
            for version in output.versions() {
                let modified = version.last_modified().ok_or_else(|| {
                    ApiError::Internal(
                        "object reconciliation found a version without last_modified".to_owned(),
                    )
                })?;
                if modified.secs() > before_unix_seconds {
                    continue;
                }
                candidates.push(ObjectVersionCandidate {
                    object_key: version
                        .key()
                        .ok_or_else(|| {
                            ApiError::Internal(
                                "object reconciliation found a version without a key".to_owned(),
                            )
                        })?
                        .to_owned(),
                    object_version_id: version
                        .version_id()
                        .ok_or_else(|| {
                            ApiError::Internal(
                                "object reconciliation found a version without an ID".to_owned(),
                            )
                        })?
                        .to_owned(),
                    last_modified_unix_seconds: modified.secs(),
                    delete_marker: false,
                });
            }
            for marker in output.delete_markers() {
                let modified = marker.last_modified().ok_or_else(|| {
                    ApiError::Internal(
                        "object reconciliation found a delete marker without last_modified"
                            .to_owned(),
                    )
                })?;
                if modified.secs() > before_unix_seconds {
                    continue;
                }
                candidates.push(ObjectVersionCandidate {
                    object_key: marker
                        .key()
                        .ok_or_else(|| {
                            ApiError::Internal(
                                "object reconciliation found a delete marker without a key"
                                    .to_owned(),
                            )
                        })?
                        .to_owned(),
                    object_version_id: marker
                        .version_id()
                        .ok_or_else(|| {
                            ApiError::Internal(
                                "object reconciliation found a delete marker without an ID"
                                    .to_owned(),
                            )
                        })?
                        .to_owned(),
                    last_modified_unix_seconds: modified.secs(),
                    delete_marker: true,
                });
            }
            if candidates.len() >= limit || !output.is_truncated().unwrap_or(false) {
                break;
            }
            key_marker = output.next_key_marker().map(ToOwned::to_owned);
            version_id_marker = output.next_version_id_marker().map(ToOwned::to_owned);
            if key_marker.is_none() {
                return Err(ApiError::Internal(
                    "truncated reconciliation inventory omitted the next key marker".to_owned(),
                ));
            }
        }
        candidates.sort_by(|left, right| {
            left.last_modified_unix_seconds
                .cmp(&right.last_modified_unix_seconds)
                .then_with(|| left.object_key.cmp(&right.object_key))
                .then_with(|| left.object_version_id.cmp(&right.object_version_id))
        });
        candidates.truncate(limit);
        Ok(candidates)
    }

    pub async fn delete_exact_version(
        &self,
        object_key: &str,
        object_version_id: &str,
    ) -> ApiResult<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(object_key)
            .version_id(object_version_id)
            .send()
            .await
            .map_err(|error| {
                ApiError::Internal(format!(
                    "object reconciliation could not delete exact version: {error}"
                ))
            })?;
        metrics::counter!("asset.object.reconciliation", "result" => "purged").increment(1);
        Ok(())
    }

    async fn purge_all_versions_inner(&self, key: &str) -> ApiResult<PurgeResult> {
        let mut version_ids = Vec::new();
        let mut delete_marker_ids = Vec::new();
        let mut key_marker = None;
        let mut version_id_marker = None;

        loop {
            let mut request = self
                .client
                .list_object_versions()
                .bucket(&self.bucket)
                .prefix(key);
            if let Some(marker) = key_marker.as_deref() {
                request = request.key_marker(marker);
            }
            if let Some(marker) = version_id_marker.as_deref() {
                request = request.version_id_marker(marker);
            }
            let output = request.send().await.map_err(|error| {
                ApiError::Internal(format!("object version listing failed: {error}"))
            })?;

            for version in output.versions() {
                if version.key() == Some(key) {
                    let version_id = version.version_id().ok_or_else(|| {
                        ApiError::Internal(
                            "object version listing returned a version without an ID".to_owned(),
                        )
                    })?;
                    version_ids.push(version_id.to_owned());
                }
            }
            for marker in output.delete_markers() {
                if marker.key() == Some(key) {
                    let version_id = marker.version_id().ok_or_else(|| {
                        ApiError::Internal(
                            "object version listing returned a delete marker without an ID"
                                .to_owned(),
                        )
                    })?;
                    delete_marker_ids.push(version_id.to_owned());
                }
            }

            if !output.is_truncated().unwrap_or(false) {
                break;
            }
            key_marker = output.next_key_marker().map(ToOwned::to_owned);
            version_id_marker = output.next_version_id_marker().map(ToOwned::to_owned);
            if key_marker.is_none() {
                return Err(ApiError::Internal(
                    "truncated object version listing omitted the next key marker".to_owned(),
                ));
            }
        }

        for version_id in version_ids.iter().chain(&delete_marker_ids) {
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(key)
                .version_id(version_id)
                .send()
                .await
                .map_err(|error| {
                    ApiError::Internal(format!("object version deletion failed: {error}"))
                })?;
        }

        let remaining = self.exact_version_count(key).await?;
        if remaining != 0 {
            return Err(ApiError::Internal(format!(
                "{remaining} object versions remain after purge"
            )));
        }

        Ok(PurgeResult {
            versions_deleted: version_ids.len(),
            delete_markers_deleted: delete_marker_ids.len(),
        })
    }

    async fn exact_version_count(&self, key: &str) -> ApiResult<usize> {
        let mut count = 0usize;
        let mut key_marker = None;
        let mut version_id_marker = None;
        loop {
            let mut request = self
                .client
                .list_object_versions()
                .bucket(&self.bucket)
                .prefix(key);
            if let Some(marker) = key_marker.as_deref() {
                request = request.key_marker(marker);
            }
            if let Some(marker) = version_id_marker.as_deref() {
                request = request.version_id_marker(marker);
            }
            let output = request.send().await.map_err(|error| {
                ApiError::Internal(format!("object purge verification failed: {error}"))
            })?;
            count += output
                .versions()
                .iter()
                .filter(|version| version.key() == Some(key))
                .count();
            count += output
                .delete_markers()
                .iter()
                .filter(|marker| marker.key() == Some(key))
                .count();
            if !output.is_truncated().unwrap_or(false) {
                return Ok(count);
            }
            key_marker = output.next_key_marker().map(ToOwned::to_owned);
            version_id_marker = output.next_version_id_marker().map(ToOwned::to_owned);
            if key_marker.is_none() {
                return Err(ApiError::Internal(
                    "truncated object purge verification omitted the next key marker".to_owned(),
                ));
            }
        }
    }

    async fn prefix_version_count(&self, prefix: &str) -> ApiResult<usize> {
        let mut count = 0usize;
        let mut key_marker = None;
        let mut version_id_marker = None;
        loop {
            let mut request = self
                .client
                .list_object_versions()
                .bucket(&self.bucket)
                .prefix(prefix);
            if let Some(marker) = key_marker.as_deref() {
                request = request.key_marker(marker);
            }
            if let Some(marker) = version_id_marker.as_deref() {
                request = request.version_id_marker(marker);
            }
            let output = request.send().await.map_err(|error| {
                ApiError::Internal(format!("object prefix verification failed: {error}"))
            })?;
            count = count
                .saturating_add(output.versions().len())
                .saturating_add(output.delete_markers().len());
            if !output.is_truncated().unwrap_or(false) {
                return Ok(count);
            }
            key_marker = output.next_key_marker().map(ToOwned::to_owned);
            version_id_marker = output.next_version_id_marker().map(ToOwned::to_owned);
            if key_marker.is_none() {
                return Err(ApiError::Internal(
                    "truncated object prefix verification omitted the next key marker".to_owned(),
                ));
            }
        }
    }
}

fn exact_physical_usage_prefix(prefix: &str) -> ApiResult<String> {
    if prefix.is_empty()
        || prefix.len() > MAX_PHYSICAL_USAGE_PREFIX_BYTES
        || prefix.trim() != prefix
    {
        return Err(ApiError::invalid(
            "physical object inventory requires a non-empty exact prefix",
        ));
    }
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        return Err(ApiError::invalid(
            "physical object inventory requires a non-empty exact prefix",
        ));
    }
    Ok(format!("{prefix}/"))
}

fn validated_next_version_markers(
    prefix: &str,
    current_key_marker: Option<&str>,
    current_version_id_marker: Option<&str>,
    next_key_marker: Option<&str>,
    next_version_id_marker: Option<&str>,
) -> ApiResult<(String, Option<String>)> {
    let next_key_marker = next_key_marker
        .filter(|marker| !marker.is_empty())
        .ok_or_else(|| {
            ApiError::Internal(
                "truncated physical object inventory omitted the next key marker".to_owned(),
            )
        })?;
    if !next_key_marker.starts_with(prefix) {
        return Err(ApiError::Internal(
            "physical object inventory returned a pagination marker outside its exact prefix"
                .to_owned(),
        ));
    }
    let next_version_id_marker = match next_version_id_marker {
        Some("") => {
            return Err(ApiError::Internal(
                "physical object inventory returned an empty version pagination marker".to_owned(),
            ));
        }
        marker => marker.map(ToOwned::to_owned),
    };
    if current_key_marker == Some(next_key_marker)
        && current_version_id_marker == next_version_id_marker.as_deref()
    {
        return Err(ApiError::Internal(
            "physical object inventory pagination did not advance".to_owned(),
        ));
    }
    Ok((next_key_marker.to_owned(), next_version_id_marker))
}

fn ensure_requested_version(
    requested: Option<&str>,
    returned: Option<&str>,
    operation: &str,
) -> ApiResult<()> {
    if let Some(requested) = requested
        && returned != Some(requested)
    {
        return Err(ApiError::Internal(format!(
            "{operation} did not return the requested object version"
        )));
    }
    Ok(())
}

fn exact_object_version_id(version_id: Option<&str>, operation: &str) -> ApiResult<String> {
    let version_id = version_id
        .filter(|value| !value.trim().is_empty() && *value != "null")
        .ok_or_else(|| {
            ApiError::Internal(format!(
                "{operation} returned no exact object version ID; bucket versioning is required"
            ))
        })?;
    Ok(version_id.to_owned())
}

fn versioned_copy_source(bucket: &str, key: &str, object_version_id: Option<&str>) -> String {
    let encoded_bucket = utf8_percent_encode(bucket, NON_ALPHANUMERIC);
    let encoded_key = key
        .split('/')
        .map(|segment| utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string())
        .collect::<Vec<_>>()
        .join("/");
    match object_version_id {
        Some(version_id) => format!(
            "{encoded_bucket}/{encoded_key}?versionId={}",
            utf8_percent_encode(version_id, NON_ALPHANUMERIC)
        ),
        None => format!("{encoded_bucket}/{encoded_key}"),
    }
}

async fn sha256_file(path: &Path) -> ApiResult<(String, u64)> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| ApiError::Internal(format!("could not open file for hashing: {error}")))?;
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| ApiError::Internal(format!("could not hash file: {error}")))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        size_bytes = size_bytes.saturating_add(read as u64);
    }
    Ok((hex::encode(digest.finalize()), size_bytes))
}

pub fn inspect_archive(name: &str, bytes: &[u8]) -> ApiResult<ArchiveInventory> {
    Ok(extract_archive(name, bytes)?.inventory)
}

pub fn extract_archive(name: &str, bytes: &[u8]) -> ApiResult<ArchiveInspection> {
    let started = Instant::now();
    let lower = name.to_ascii_lowercase();
    let format = if lower.ends_with(".zip") {
        "zip"
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        "tar_gzip"
    } else if lower.ends_with(".tar") {
        "tar"
    } else {
        "not_archive"
    };
    let result = if lower.ends_with(".zip") {
        extract_zip(bytes)
    } else if lower.ends_with(".tar") || lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        extract_tar(bytes, lower.ends_with(".gz") || lower.ends_with(".tgz"))
    } else {
        Ok(ArchiveInspection::default())
    };
    let outcome = if result.is_ok() { "success" } else { "error" };
    metrics::counter!(
        "archive.inspections",
        "format" => format,
        "result" => outcome
    )
    .increment(1);
    metrics::histogram!(
        "archive.duration_ms",
        "format" => format,
        "result" => outcome
    )
    .record(started.elapsed().as_secs_f64() * 1_000.0);
    metrics::histogram!("archive.input_bytes", "format" => format).record(bytes.len() as f64);
    if let Ok(inspection) = &result {
        metrics::histogram!("archive.entries", "format" => format)
            .record(inspection.inventory.entries.len() as f64);
        metrics::histogram!("archive.expanded_bytes", "format" => format).record(
            inspection
                .inventory
                .entries
                .iter()
                .map(|entry| entry.size_bytes)
                .sum::<u64>() as f64,
        );
        metrics::histogram!("archive.quarantined_entries", "format" => format).record(
            inspection
                .inventory
                .entries
                .iter()
                .filter(|entry| entry.quarantined)
                .count() as f64,
        );
    }
    result
}

fn extract_zip(bytes: &[u8]) -> ApiResult<ArchiveInspection> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| ApiError::invalid(format!("invalid zip archive: {error}")))?;
    if archive.len() > MAX_ARCHIVE_FILES {
        return Err(ApiError::invalid(
            "archive contains more than 2,000 entries",
        ));
    }
    let mut inventory = ArchiveInventory {
        format: Some("zip".to_owned()),
        ..ArchiveInventory::default()
    };
    let mut members = Vec::with_capacity(archive.len());
    let mut seen_paths = HashSet::new();
    let mut expanded = 0u64;
    let started = Instant::now();
    for index in 0..archive.len() {
        enforce_inspection_time(started)?;
        let mut file = archive
            .by_index(index)
            .map_err(|error| ApiError::invalid(format!("could not inspect zip entry: {error}")))?;
        expanded = expanded.saturating_add(file.size());
        if expanded > MAX_ARCHIVE_EXPANDED_BYTES {
            return Err(ApiError::invalid(
                "archive expands beyond the 256 MiB safety limit",
            ));
        }
        let raw_path = file.name().to_owned();
        let enclosed = file.enclosed_name().as_deref().and_then(safe_relative_path);
        let ratio_bad = file.compressed_size() > 0
            && file.size() > file.compressed_size().saturating_mul(MAX_ARCHIVE_RATIO);
        let unsafe_path = enclosed.is_none();
        let duplicate_path = enclosed
            .as_ref()
            .is_some_and(|path| !seen_paths.insert(path.clone()));
        let member_too_large = file.size() > MAX_STAGE_BYTES as u64;
        let quarantined =
            unsafe_path || duplicate_path || ratio_bad || file.is_symlink() || member_too_large;
        let path = enclosed.unwrap_or(raw_path);
        let entry_kind = archive_entry_kind(&path, file.is_dir());
        let mut member_bytes = None;
        if !quarantined && !file.is_dir() {
            let mut extracted = Vec::with_capacity(
                usize::try_from(file.size())
                    .unwrap_or_default()
                    .min(MAX_STAGE_BYTES),
            );
            (&mut file)
                .take(MAX_STAGE_BYTES as u64 + 1)
                .read_to_end(&mut extracted)
                .map_err(|error| {
                    ApiError::invalid(format!("could not extract zip entry {path}: {error}"))
                })?;
            if extracted.len() > MAX_STAGE_BYTES {
                return Err(ApiError::invalid(
                    "archive member exceeds the 512 MiB staged-object limit",
                ));
            }
            member_bytes = Some(Bytes::from(extracted));
        }
        let content_hash = member_bytes
            .as_ref()
            .map(|bytes| format!("sha256:{}", hex::encode(Sha256::digest(bytes))));
        let inventory_index = inventory.entries.len();
        inventory.entries.push(InventoryEntry {
            path,
            entry_kind,
            size_bytes: file.size(),
            compressed_size_bytes: Some(file.compressed_size()),
            content_hash,
            readable: !file.is_dir() && !quarantined,
            quarantined,
            reason: if unsafe_path {
                Some("path_traversal".to_owned())
            } else if duplicate_path {
                Some("duplicate_path".to_owned())
            } else if file.is_symlink() {
                Some("symlink".to_owned())
            } else if ratio_bad {
                Some("compression_ratio".to_owned())
            } else if member_too_large {
                Some("member_size".to_owned())
            } else {
                None
            },
        });
        members.push(ArchiveMember {
            inventory_index,
            bytes: member_bytes,
        });
    }
    add_nesting_warning(&mut inventory);
    Ok(ArchiveInspection { inventory, members })
}

fn extract_tar(bytes: &[u8], compressed: bool) -> ApiResult<ArchiveInspection> {
    let reader: Box<dyn Read> = if compressed {
        Box::new(flate2::read::GzDecoder::new(Cursor::new(bytes)))
    } else {
        Box::new(Cursor::new(bytes))
    };
    let mut archive = tar::Archive::new(reader);
    let mut inventory = ArchiveInventory {
        format: Some(if compressed { "tar.gz" } else { "tar" }.to_owned()),
        ..ArchiveInventory::default()
    };
    let mut members = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut expanded = 0u64;
    let started = Instant::now();
    for (index, entry) in archive
        .entries()
        .map_err(|error| ApiError::invalid(format!("invalid tar archive: {error}")))?
        .enumerate()
    {
        enforce_inspection_time(started)?;
        if index >= MAX_ARCHIVE_FILES {
            return Err(ApiError::invalid(
                "archive contains more than 2,000 entries",
            ));
        }
        let mut entry =
            entry.map_err(|error| ApiError::invalid(format!("invalid tar entry: {error}")))?;
        let size = entry.size();
        expanded = expanded.saturating_add(size);
        if expanded > MAX_ARCHIVE_EXPANDED_BYTES {
            return Err(ApiError::invalid(
                "archive expands beyond the 256 MiB safety limit",
            ));
        }
        if compressed && expanded > (bytes.len() as u64).saturating_mul(MAX_ARCHIVE_RATIO) {
            return Err(ApiError::invalid(
                "compressed tar exceeds the 200:1 expansion-ratio safety limit",
            ));
        }
        let raw_path = entry
            .path()
            .ok()
            .map(|path| path.to_string_lossy().into_owned());
        let path = raw_path
            .as_deref()
            .and_then(|path| safe_relative_path(Path::new(path)));
        let entry_type = entry.header().entry_type();
        let unsafe_path = path.is_none();
        let duplicate_path = path
            .as_ref()
            .is_some_and(|path| !seen_paths.insert(path.clone()));
        let unsafe_type = entry_type.is_symlink()
            || entry_type.is_hard_link()
            || entry_type.is_block_special()
            || entry_type.is_character_special()
            || entry_type.is_fifo();
        let member_too_large = size > MAX_STAGE_BYTES as u64;
        let quarantined = unsafe_path || duplicate_path || unsafe_type || member_too_large;
        let path = path
            .or(raw_path)
            .unwrap_or_else(|| "(invalid path)".to_owned());
        let entry_kind = archive_entry_kind(&path, entry_type.is_dir());
        let mut member_bytes = None;
        if !quarantined && entry_type.is_file() {
            let mut extracted = Vec::with_capacity(
                usize::try_from(size)
                    .unwrap_or_default()
                    .min(MAX_STAGE_BYTES),
            );
            (&mut entry)
                .take(MAX_STAGE_BYTES as u64 + 1)
                .read_to_end(&mut extracted)
                .map_err(|error| {
                    ApiError::invalid(format!("could not extract tar entry {path}: {error}"))
                })?;
            if extracted.len() > MAX_STAGE_BYTES {
                return Err(ApiError::invalid(
                    "archive member exceeds the 512 MiB staged-object limit",
                ));
            }
            member_bytes = Some(Bytes::from(extracted));
        }
        let content_hash = member_bytes
            .as_ref()
            .map(|bytes| format!("sha256:{}", hex::encode(Sha256::digest(bytes))));
        let inventory_index = inventory.entries.len();
        inventory.entries.push(InventoryEntry {
            path,
            entry_kind,
            size_bytes: size,
            compressed_size_bytes: None,
            content_hash,
            readable: entry_type.is_file() && !quarantined,
            quarantined,
            reason: if unsafe_path {
                Some("path_traversal".to_owned())
            } else if duplicate_path {
                Some("duplicate_path".to_owned())
            } else if unsafe_type {
                Some("special_file".to_owned())
            } else if member_too_large {
                Some("member_size".to_owned())
            } else {
                None
            },
        });
        members.push(ArchiveMember {
            inventory_index,
            bytes: member_bytes,
        });
    }
    add_nesting_warning(&mut inventory);
    Ok(ArchiveInspection { inventory, members })
}

fn safe_relative_path(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str()?;
                if part.is_empty() || part.contains('\\') || part.contains('\0') {
                    return None;
                }
                parts.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    let normalized = parts.join("/");
    (!normalized.is_empty() && normalized.len() <= MAX_ARCHIVE_PATH_BYTES).then_some(normalized)
}

fn archive_entry_kind(path: &str, directory: bool) -> String {
    if directory {
        "directory".to_owned()
    } else if is_archive_path(path) {
        "archive".to_owned()
    } else {
        "file".to_owned()
    }
}

fn is_archive_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".tar")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
}

fn add_nesting_warning(inventory: &mut ArchiveInventory) {
    if inventory
        .entries
        .iter()
        .any(|entry| entry.entry_kind == "archive" && !entry.quarantined)
    {
        inventory
            .warnings
            .push("nested archives are retained as opaque members and are not expanded".to_owned());
    }
}

fn enforce_inspection_time(started: Instant) -> ApiResult<()> {
    if started.elapsed() > MAX_ARCHIVE_INSPECTION_TIME {
        Err(ApiError::invalid(
            "archive inspection exceeded the five-second safety budget",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn physical_usage_prefix_is_an_exact_namespace_boundary() {
        assert_eq!(exact_physical_usage_prefix("user-id").unwrap(), "user-id/");
        assert_eq!(
            exact_physical_usage_prefix("user-id///").unwrap(),
            "user-id/"
        );
        assert!(exact_physical_usage_prefix("").is_err());
        assert!(exact_physical_usage_prefix("/").is_err());
        assert!(exact_physical_usage_prefix(" user-id").is_err());
        assert!(exact_physical_usage_prefix("user-id ").is_err());
    }

    #[test]
    fn physical_usage_pagination_requires_prefix_scoped_progress() {
        assert_eq!(
            validated_next_version_markers(
                "user/",
                Some("user/blob"),
                Some("v2"),
                Some("user/blob"),
                Some("v1"),
            )
            .unwrap(),
            ("user/blob".to_owned(), Some("v1".to_owned()))
        );
        assert!(
            validated_next_version_markers(
                "user/",
                Some("user/blob"),
                Some("v1"),
                Some("user/blob"),
                Some("v1"),
            )
            .is_err()
        );
        assert!(validated_next_version_markers("user/", None, None, None, None).is_err());
        assert!(
            validated_next_version_markers("user/", None, None, Some("another-user/blob"), None,)
                .is_err()
        );
        assert!(
            validated_next_version_markers("user/", None, None, Some("user/blob"), Some(""),)
                .is_err()
        );
    }

    #[test]
    fn unavailable_physical_usage_never_looks_like_zero_usage() {
        let snapshot = PhysicalUsageSnapshot::unavailable();
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(json["status"], "unavailable");
        assert_eq!(json["object_count_semantics"], "physical_object_versions");
        assert!(json["physical_object_versions"].is_null());
        assert!(json["physical_size_bytes"].is_null());
        assert!(json["observed_at"].is_null());
    }

    #[test]
    fn zip_inventory_quarantines_traversal() {
        let mut bytes = Vec::new();
        {
            let cursor = Cursor::new(&mut bytes);
            let mut writer = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("../escape.txt", options).unwrap();
            writer.write_all(b"nope").unwrap();
            writer.finish().unwrap();
        }
        let inspection = extract_zip(&bytes).unwrap();
        assert!(inspection.inventory.entries[0].quarantined);
        assert!(inspection.members[0].bytes.is_none());
    }

    #[test]
    fn zip_extraction_returns_safe_members_without_expanding_nested_archives() {
        let mut bytes = Vec::new();
        {
            let cursor = Cursor::new(&mut bytes);
            let mut writer = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("docs/readme.txt", options).unwrap();
            writer.write_all(b"immutable staged text").unwrap();
            writer.start_file("nested.zip", options).unwrap();
            writer.write_all(b"opaque nested bytes").unwrap();
            writer.finish().unwrap();
        }

        let inspection = extract_archive("pack.zip", &bytes).unwrap();
        assert_eq!(inspection.inventory.entries.len(), 2);
        assert_eq!(
            inspection.members[0].bytes.as_deref(),
            Some(&b"immutable staged text"[..])
        );
        assert_eq!(inspection.inventory.entries[1].entry_kind, "archive");
        assert_eq!(inspection.members.len(), 2);
        assert_eq!(inspection.inventory.warnings.len(), 1);
    }

    #[test]
    fn tar_extraction_keeps_safe_file_bytes_and_path_validation_rejects_traversal() {
        let mut bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(4);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "safe.txt", &b"safe"[..])
                .unwrap();
            builder.finish().unwrap();
        }

        let inspection = extract_archive("pack.tar", &bytes).unwrap();
        assert_eq!(inspection.members[0].bytes.as_deref(), Some(&b"safe"[..]));
        assert!(safe_relative_path(Path::new("../escape.txt")).is_none());
        assert!(safe_relative_path(Path::new("safe/../escape.txt")).is_none());
    }

    #[test]
    fn copy_source_preserves_key_structure_and_encodes_opaque_version_ids() {
        assert_eq!(
            versioned_copy_source(
                "carry-state",
                "user/uploads/file name",
                Some("opaque+/= value")
            ),
            "carry%2Dstate/user/uploads/file%20name?versionId=opaque%2B%2F%3D%20value"
        );
        assert_eq!(
            versioned_copy_source("bucket", "user/blobs/hash", None),
            "bucket/user/blobs/hash"
        );
    }

    #[test]
    fn requested_version_must_be_echoed_exactly() {
        assert!(ensure_requested_version(None, None, "read").is_ok());
        assert!(ensure_requested_version(Some("v1"), Some("v1"), "read").is_ok());
        assert!(ensure_requested_version(Some("v1"), Some("v2"), "read").is_err());
        assert!(ensure_requested_version(Some("v1"), None, "read").is_err());
    }

    #[test]
    fn canonical_writes_require_real_provider_version_ids() {
        assert_eq!(
            exact_object_version_id(Some("opaque-version"), "write").unwrap(),
            "opaque-version"
        );
        for invalid in [None, Some(""), Some("   "), Some("null")] {
            assert!(exact_object_version_id(invalid, "write").is_err());
        }
    }

    #[tokio::test]
    async fn live_reads_stay_on_the_pinned_version_after_latest_changes() {
        if std::env::var("STRAYLIGHT_LIVE_S3_VERSION_TEST").as_deref() != Ok("1") {
            eprintln!("STRAYLIGHT_LIVE_S3_VERSION_TEST is unset; skipping live S3 version test");
            return;
        }
        let config = Config::from_env().expect("load live S3 test configuration");
        let store = ObjectStore::new(&config)
            .await
            .expect("create live S3 client");
        store.ensure_bucket().await.expect("ensure test bucket");

        let user_id = UserId(uuid::Uuid::now_v7());
        let original = Bytes::from_static(b"original pinned bytes");
        let first = store
            .put_user_blob(user_id, Some("application/octet-stream"), original.clone())
            .await
            .expect("write original object");
        let pinned_version = first
            .object_version_id
            .as_deref()
            .expect("versioned write returned an object version ID");

        let mut replacement = tempfile::NamedTempFile::new().expect("create replacement file");
        replacement
            .write_all(b"replacement latest bytes")
            .expect("write replacement file");
        let latest = store
            .put_file(
                &first.object_key,
                "application/octet-stream",
                replacement.path(),
            )
            .await
            .expect("write a newer version at the same key");
        assert_ne!(latest.object_version_id.as_deref(), Some(pinned_version));
        assert_eq!(
            store.get(&first.object_key).await.expect("read latest key"),
            Bytes::from_static(b"replacement latest bytes")
        );
        assert_eq!(
            store
                .get_version(&first.object_key, Some(pinned_version))
                .await
                .expect("read pinned object version"),
            original
        );
        store
            .verify_object_content_version(
                &first.object_key,
                Some(pinned_version),
                &first.sha256,
                u64::try_from(first.size_bytes).unwrap(),
            )
            .await
            .expect("verify pinned object bytes");
        assert_eq!(
            store
                .get_prefix_version(&first.object_key, Some(pinned_version), 8)
                .await
                .expect("read pinned prefix"),
            Bytes::from_static(b"original")
        );

        store
            .purge_all_versions(&first.object_key)
            .await
            .expect("purge live test versions");
    }
}
