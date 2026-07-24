use std::{
    collections::HashSet,
    io::{Cursor, Read},
    path::{Component, Path},
    time::{Duration, Instant},
};

use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{
    Client,
    config::{Credentials, SharedCredentialsProvider},
    primitives::ByteStream,
};
use bytes::Bytes;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::{
    config::Config,
    error::{ApiError, ApiResult},
    models::UserId,
};

const MAX_STAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_ARCHIVE_FILES: usize = 2_000;
const MAX_ARCHIVE_EXPANDED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARCHIVE_RATIO: u64 = 200;
const MAX_ARCHIVE_PATH_BYTES: usize = 1_024;
const MAX_ARCHIVE_INSPECTION_TIME: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct ObjectStore {
    client: Client,
    bucket: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct StoredBlob {
    pub sha256: String,
    pub size_bytes: usize,
    pub object_key: String,
    pub created: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct StoredFile {
    pub sha256: String,
    pub size_bytes: u64,
    pub object_key: String,
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
    pub exact_version_purge: PurgeResult,
    pub prefix_version_purge: PurgeResult,
}

pub struct ObjectStream {
    pub body: ByteStream,
    pub content_length: Option<i64>,
    pub content_type: Option<String>,
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
        let credentials = Credentials::new(
            config.s3_access_key.clone(),
            config.s3_secret_key.clone(),
            None,
            None,
            "straylight-config",
        );
        let shared = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(config.s3_region.clone()))
            .credentials_provider(SharedCredentialsProvider::new(credentials))
            .load()
            .await;
        let s3_config = aws_sdk_s3::config::Builder::from(&shared)
            .endpoint_url(&config.s3_endpoint)
            .force_path_style(true)
            .build();
        Ok(Self {
            client: Client::from_conf(s3_config),
            bucket: config.s3_bucket.clone(),
        })
    }

    pub async fn ensure_bucket(&self) -> ApiResult<()> {
        let started = Instant::now();
        let missing = self
            .client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .is_err();
        if missing {
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
        let created = match result {
            Ok(_) => true,
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
                false
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
            created,
        })
    }

    pub async fn get(&self, key: &str) -> ApiResult<Bytes> {
        let started = Instant::now();
        let result = async {
            let output = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .map_err(|error| {
                    ApiError::not_found("asset_not_found", &format!("{key}: {error}"))
                })?;
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

    pub async fn get_stream(&self, key: &str) -> ApiResult<ObjectStream> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| ApiError::not_found("asset_not_found", &format!("{key}: {error}")))?;
        Ok(ObjectStream {
            body: output.body,
            content_length: output.content_length,
            content_type: output.content_type,
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
        self.client
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
        })
    }

    pub async fn download_to_path(&self, key: &str, path: &Path) -> ApiResult<StoredFile> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| ApiError::not_found("asset_not_found", &format!("{key}: {error}")))?;
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
            let second = self
                .put_user_blob(user_id, Some("text/plain"), payload.clone())
                .await?;
            if second.created || second.object_key != first.object_key {
                return Err(ApiError::Internal(
                    "repeated content did not deduplicate through conditional create".to_owned(),
                ));
            }
            if self.get(&first.object_key).await? != payload {
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
}
