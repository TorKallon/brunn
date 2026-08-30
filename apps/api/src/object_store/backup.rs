use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use aws_sdk_s3::{
    primitives::{ByteStream, Length},
    types::{CompletedMultipartUpload, CompletedPart},
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, QueryBuilder, Row, postgres::PgPoolOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::ObjectStore;

const BACKUP_FORMAT: &str = "straylight-object-version-archive@v1";
const RESTORE_MAP_FORMAT: &str = "straylight-object-version-restore-map@v2";
const LEGACY_RESTORE_MAP_FORMAT: &str = "straylight-object-version-restore-map@v1";
const MAX_PORTABLE_OBJECT_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const MULTIPART_RESTORE_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;
const MULTIPART_RESTORE_PART_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ObjectBackupManifest {
    pub format: String,
    pub created_at: String,
    pub source_bucket: String,
    pub bucket_versioning: String,
    pub entries: Vec<ObjectBackupEntry>,
    pub summary: ObjectBackupSummary,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ObjectBackupEntry {
    pub kind: String,
    pub key: String,
    pub source_version_id: String,
    pub is_latest: bool,
    pub last_modified_unix_seconds: i64,
    pub last_modified_subsecond_nanos: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_disposition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_language: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ObjectBackupSummary {
    pub object_versions: usize,
    pub delete_markers: usize,
    pub unique_blobs: usize,
    pub logical_bytes: u64,
    pub stored_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ObjectRestoreMap {
    pub format: String,
    pub restored_at: String,
    pub source_bucket: String,
    pub target_bucket: String,
    pub source_manifest_sha256: String,
    #[serde(default)]
    pub target_was_empty: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<ObjectRestorePending>,
    #[serde(default = "default_restore_complete")]
    pub complete: bool,
    pub entries: Vec<ObjectRestoreEntry>,
    pub summary: ObjectRestoreSummary,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ObjectRestorePending {
    pub kind: String,
    pub key: String,
    pub source_version_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ObjectRestoreEntry {
    pub kind: String,
    pub key: String,
    pub source_version_id: String,
    pub restored_version_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ObjectRestoreSummary {
    pub object_versions: usize,
    pub delete_markers: usize,
    pub logical_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DatabaseRemapSummary {
    pub mapping_entries: usize,
    pub asset_versions_updated: u64,
    pub workspace_entry_versions_updated: u64,
    pub upload_temporary_versions_updated: u64,
    pub upload_canonical_versions_updated: u64,
    pub account_exports_updated: u64,
    pub cleaned_upload_versions_cleared: u64,
    pub database_references_verified: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DatabasePinSummary {
    pub asset_versions_pinned: u64,
    pub upload_canonical_versions_pinned: u64,
    pub account_export_versions_pinned: u64,
    pub objects_stream_verified: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DatabaseReferenceSummary {
    pub references_verified: u64,
    pub unique_object_versions_verified: u64,
    pub logical_bytes_verified: u64,
}

#[derive(Clone, Debug)]
struct ListedEvent {
    kind: &'static str,
    key: String,
    version_id: String,
    is_latest: bool,
    last_modified_unix_seconds: i64,
    last_modified_subsecond_nanos: u32,
    size_bytes: Option<u64>,
    etag: Option<String>,
}

pub async fn export(store: &ObjectStore, output: &Path) -> Result<ObjectBackupManifest> {
    let versioning = require_enabled_versioning(store).await?;
    ensure_no_multipart_uploads(store).await?;
    std::fs::create_dir(output).with_context(|| {
        format!(
            "could not create object backup directory {}",
            output.display()
        )
    })?;
    let objects_dir = output.join("objects");
    std::fs::create_dir(&objects_dir).with_context(|| {
        format!(
            "could not create object backup data directory {}",
            objects_dir.display()
        )
    })?;

    let result = export_inner(store, output, &objects_dir, versioning).await;
    if result.is_err() {
        let _ = std::fs::remove_dir_all(output);
    }
    result
}

async fn export_inner(
    store: &ObjectStore,
    output: &Path,
    objects_dir: &Path,
    versioning: String,
) -> Result<ObjectBackupManifest> {
    let listed = list_events(store).await?;
    validate_latest_events(&listed)?;
    let mut entries = Vec::with_capacity(listed.len());
    let mut unique_blobs = BTreeSet::new();
    let mut logical_bytes = 0u64;

    for event in listed {
        if event.kind == "delete_marker" {
            entries.push(ObjectBackupEntry {
                kind: event.kind.to_owned(),
                key: event.key,
                source_version_id: event.version_id,
                is_latest: event.is_latest,
                last_modified_unix_seconds: event.last_modified_unix_seconds,
                last_modified_subsecond_nanos: event.last_modified_subsecond_nanos,
                size_bytes: None,
                sha256: None,
                etag: None,
                content_type: None,
                cache_control: None,
                content_disposition: None,
                content_encoding: None,
                content_language: None,
                metadata: BTreeMap::new(),
            });
            continue;
        }

        let listed_size = event
            .size_bytes
            .context("object version inventory omitted its size")?;
        ensure!(
            listed_size <= MAX_PORTABLE_OBJECT_BYTES,
            "object {} version {} is {} bytes; the portable recovery format currently supports at most {} bytes per object",
            event.key,
            event.version_id,
            listed_size,
            MAX_PORTABLE_OBJECT_BYTES
        );
        let mut request = store
            .client
            .get_object()
            .bucket(&store.bucket)
            .key(&event.key)
            .version_id(&event.version_id);
        request = request.checksum_mode(aws_sdk_s3::types::ChecksumMode::Enabled);
        let response = request.send().await.with_context(|| {
            format!(
                "could not read object {} version {}",
                event.key, event.version_id
            )
        })?;
        ensure!(
            response.version_id() == Some(event.version_id.as_str()),
            "object read returned the wrong version for {}",
            event.key
        );
        let content_type = response.content_type().map(ToOwned::to_owned);
        let cache_control = response.cache_control().map(ToOwned::to_owned);
        let content_disposition = response.content_disposition().map(ToOwned::to_owned);
        let content_encoding = response.content_encoding().map(ToOwned::to_owned);
        let content_language = response.content_language().map(ToOwned::to_owned);
        let metadata = response
            .metadata()
            .map(|values| {
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let temporary_path = objects_dir.join(format!(".partial-{}", uuid::Uuid::now_v7()));
        let (sha256, downloaded_size) = download_body(response.body, &temporary_path)
            .await
            .with_context(|| {
                format!(
                    "could not archive object {} version {}",
                    event.key, event.version_id
                )
            })?;
        ensure!(
            downloaded_size == listed_size,
            "object {} version {} changed size during inventory: listed={} downloaded={}",
            event.key,
            event.version_id,
            listed_size,
            downloaded_size
        );
        let blob_path = objects_dir.join(&sha256);
        if blob_path.exists() {
            verify_blob(&blob_path, &sha256, downloaded_size).await?;
            std::fs::remove_file(&temporary_path).with_context(|| {
                format!(
                    "could not remove duplicate temporary object {}",
                    temporary_path.display()
                )
            })?;
        } else {
            std::fs::rename(&temporary_path, &blob_path).with_context(|| {
                format!("could not finalize archived object {}", blob_path.display())
            })?;
        }
        unique_blobs.insert(sha256.clone());
        logical_bytes = logical_bytes
            .checked_add(downloaded_size)
            .context("object backup logical byte count overflowed")?;
        entries.push(ObjectBackupEntry {
            kind: event.kind.to_owned(),
            key: event.key,
            source_version_id: event.version_id,
            is_latest: event.is_latest,
            last_modified_unix_seconds: event.last_modified_unix_seconds,
            last_modified_subsecond_nanos: event.last_modified_subsecond_nanos,
            size_bytes: Some(downloaded_size),
            sha256: Some(format!("sha256:{sha256}")),
            etag: event.etag,
            content_type,
            cache_control,
            content_disposition,
            content_encoding,
            content_language,
            metadata,
        });
    }

    entries.sort_by(entry_sort_key);
    let stored_bytes = unique_blobs.iter().try_fold(0u64, |total, digest| {
        let size = std::fs::metadata(objects_dir.join(digest))
            .with_context(|| format!("could not inspect archived blob {digest}"))?
            .len();
        total
            .checked_add(size)
            .context("object backup stored byte count overflowed")
    })?;
    let object_versions = entries
        .iter()
        .filter(|entry| entry.kind == "object_version")
        .count();
    let delete_markers = entries.len().saturating_sub(object_versions);
    let manifest = ObjectBackupManifest {
        format: BACKUP_FORMAT.to_owned(),
        created_at: now(),
        source_bucket: store.bucket.clone(),
        bucket_versioning: versioning,
        entries,
        summary: ObjectBackupSummary {
            object_versions,
            delete_markers,
            unique_blobs: unique_blobs.len(),
            logical_bytes,
            stored_bytes,
        },
    };
    validate_manifest(&manifest)?;
    write_json_sync(&output.join("manifest.json"), &manifest)?;
    Ok(manifest)
}

pub async fn verify(input: &Path) -> Result<ObjectBackupManifest> {
    let manifest = load_manifest(input)?;
    validate_manifest(&manifest)?;
    let objects_dir = input.join("objects");
    let mut expected = BTreeMap::new();
    for entry in &manifest.entries {
        if entry.kind != "object_version" {
            continue;
        }
        let digest = digest_from_entry(entry)?;
        let size = entry.size_bytes.context("object version omitted size")?;
        if let Some(existing) = expected.insert(digest.clone(), size) {
            ensure!(
                existing == size,
                "digest {digest} is associated with conflicting sizes"
            );
        }
    }
    for (digest, size) in &expected {
        verify_blob(&objects_dir.join(digest), digest, *size).await?;
    }
    let actual = std::fs::read_dir(&objects_dir)
        .with_context(|| {
            format!(
                "could not inspect object backup directory {}",
                objects_dir.display()
            )
        })?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().len() == 64)
        .count();
    ensure!(
        actual == expected.len(),
        "object backup contains {} data files but manifest names {}",
        actual,
        expected.len()
    );
    Ok(manifest)
}

pub async fn restore(
    store: &ObjectStore,
    input: &Path,
    mapping_path: &Path,
) -> Result<ObjectRestoreMap> {
    let manifest = verify(input).await?;
    let manifest_hash = hash_file(&input.join("manifest.json")).await?.0;
    let source_manifest_sha256 = format!("sha256:{manifest_hash}");
    require_enabled_versioning(store).await?;

    let mut mapping = if mapping_path.exists() {
        let mapping = load_restore_map(mapping_path)?;
        validate_restore_map(&mapping)?;
        ensure!(
            mapping.target_bucket == store.bucket,
            "restore journal targets bucket {}, not configured bucket {}",
            mapping.target_bucket,
            store.bucket
        );
        ensure!(
            mapping.source_bucket == manifest.source_bucket
                && mapping.source_manifest_sha256 == source_manifest_sha256,
            "restore journal does not belong to this source manifest"
        );
        ensure!(
            mapping.format == RESTORE_MAP_FORMAT || mapping.complete,
            "legacy restore maps cannot resume an incomplete restore"
        );
        if !mapping.complete {
            ensure!(
                mapping.target_was_empty,
                "incomplete restore journal does not prove the target started empty"
            );
            abort_all_multipart_uploads(store).await?;
        } else {
            ensure_no_multipart_uploads(store).await?;
        }
        mapping
    } else {
        ensure_no_multipart_uploads(store).await?;
        let existing = list_events(store).await?;
        ensure!(
            existing.is_empty(),
            "restore target bucket {} is not empty; use a dedicated empty drill or recovery bucket",
            store.bucket
        );
        let mapping = ObjectRestoreMap {
            format: RESTORE_MAP_FORMAT.to_owned(),
            restored_at: now(),
            source_bucket: manifest.source_bucket.clone(),
            target_bucket: store.bucket.clone(),
            source_manifest_sha256: source_manifest_sha256.clone(),
            target_was_empty: true,
            pending: None,
            complete: false,
            entries: Vec::with_capacity(manifest.entries.len()),
            summary: ObjectRestoreSummary {
                object_versions: 0,
                delete_markers: 0,
                logical_bytes: 0,
            },
        };
        write_json_atomic(mapping_path, &mapping)?;
        mapping
    };

    validate_restore_map_against_manifest(&mapping, &manifest, false)?;
    reconcile_restore_journal(store, &manifest, mapping_path, &mut mapping).await?;

    for entry in restore_order(&manifest.entries) {
        if mapping.entries.iter().any(|restored| {
            restored.kind == entry.kind
                && restored.key == entry.key
                && restored.source_version_id == entry.source_version_id
        }) {
            continue;
        }

        let pending = pending_from_entry(entry);
        match mapping.pending.as_ref() {
            Some(existing) => ensure!(
                same_pending_identity(existing, &pending),
                "restore journal pending entry is not the next source event"
            ),
            None => {
                mapping.pending = Some(pending);
                mapping.restored_at = now();
                write_json_atomic(mapping_path, &mapping)?;
            }
        }

        let restored = restore_one(store, input, entry).await?;
        mapping.entries.push(restored);
        mapping.pending = None;
        mapping.restored_at = now();
        refresh_restore_summary(&mut mapping)?;
        validate_restore_map(&mapping)?;
        write_json_atomic(mapping_path, &mapping)?;
    }

    mapping.complete = true;
    mapping.restored_at = now();
    refresh_restore_summary(&mut mapping)?;
    validate_restore_map(&mapping)?;
    validate_restore_map_against_manifest(&mapping, &manifest, true)?;
    verify_restored_inventory(store, &manifest, &mapping).await?;
    write_json_atomic(mapping_path, &mapping)?;
    Ok(mapping)
}

async fn restore_one(
    store: &ObjectStore,
    input: &Path,
    entry: &ObjectBackupEntry,
) -> Result<ObjectRestoreEntry> {
    if entry.kind == "delete_marker" {
        let output = store
            .client
            .delete_object()
            .bucket(&store.bucket)
            .key(&entry.key)
            .send()
            .await
            .with_context(|| format!("could not recreate delete marker for {}", entry.key))?;
        ensure!(
            output.delete_marker().unwrap_or(false),
            "restored delete did not create a marker for {}",
            entry.key
        );
        let version_id = output
            .version_id()
            .context("restored delete marker omitted its version ID")?
            .to_owned();
        return Ok(ObjectRestoreEntry {
            kind: entry.kind.clone(),
            key: entry.key.clone(),
            source_version_id: entry.source_version_id.clone(),
            restored_version_id: version_id,
            sha256: None,
            size_bytes: None,
        });
    }

    let digest = digest_from_entry(entry)?;
    let size = entry.size_bytes.context("object version omitted size")?;
    let blob_path = input.join("objects").join(&digest);
    let restored_version_id = if should_use_multipart_restore(size) {
        restore_object_multipart(store, entry, &blob_path, size).await?
    } else {
        restore_object_single_put(store, entry, &blob_path, size).await?
    };
    verify_remote_object(store, entry, &restored_version_id).await?;
    Ok(ObjectRestoreEntry {
        kind: entry.kind.clone(),
        key: entry.key.clone(),
        source_version_id: entry.source_version_id.clone(),
        restored_version_id,
        sha256: entry.sha256.clone(),
        size_bytes: entry.size_bytes,
    })
}

async fn restore_object_single_put(
    store: &ObjectStore,
    entry: &ObjectBackupEntry,
    blob_path: &Path,
    size: u64,
) -> Result<String> {
    let body = ByteStream::from_path(blob_path)
        .await
        .with_context(|| format!("could not open archived blob {}", blob_path.display()))?;
    let mut request = store
        .client
        .put_object()
        .bucket(&store.bucket)
        .key(&entry.key)
        .content_length(i64::try_from(size).context("object size exceeds i64")?)
        .set_content_type(entry.content_type.clone())
        .set_cache_control(entry.cache_control.clone())
        .set_content_disposition(entry.content_disposition.clone())
        .set_content_encoding(entry.content_encoding.clone())
        .set_content_language(entry.content_language.clone())
        .set_metadata(Some(entry.metadata.clone().into_iter().collect()))
        .body(body);
    if entry.metadata.is_empty() {
        request = request.set_metadata(None);
    }
    let output = request.send().await.with_context(|| {
        format!(
            "could not restore object {} source version {}",
            entry.key, entry.source_version_id
        )
    })?;
    output
        .version_id()
        .filter(|value| !value.is_empty() && *value != "null")
        .map(ToOwned::to_owned)
        .context("restored object write omitted its exact version ID")
}

async fn restore_object_multipart(
    store: &ObjectStore,
    entry: &ObjectBackupEntry,
    blob_path: &Path,
    size: u64,
) -> Result<String> {
    ensure!(
        multipart_restore_part_count(size) <= 10_000,
        "multipart restore exceeds the provider part-count limit"
    );
    let actual_size = tokio::fs::metadata(blob_path)
        .await
        .with_context(|| format!("could not inspect archived blob {}", blob_path.display()))?
        .len();
    ensure!(
        actual_size == size,
        "archived blob size changed before multipart restore"
    );

    let mut create = store
        .client
        .create_multipart_upload()
        .bucket(&store.bucket)
        .key(&entry.key)
        .set_content_type(entry.content_type.clone())
        .set_cache_control(entry.cache_control.clone())
        .set_content_disposition(entry.content_disposition.clone())
        .set_content_encoding(entry.content_encoding.clone())
        .set_content_language(entry.content_language.clone())
        .set_metadata(Some(entry.metadata.clone().into_iter().collect()));
    if entry.metadata.is_empty() {
        create = create.set_metadata(None);
    }
    let created = create.send().await.with_context(|| {
        format!(
            "could not begin multipart restore for {} source version {}",
            entry.key, entry.source_version_id
        )
    })?;
    let upload_id = created
        .upload_id()
        .filter(|value| !value.is_empty())
        .context("multipart restore omitted its upload ID")?
        .to_owned();

    let result: Result<String> = async {
        let mut completed_parts = Vec::new();
        let mut offset = 0u64;
        let mut part_number = 1i32;
        while offset < size {
            let part_size = (size - offset).min(MULTIPART_RESTORE_PART_BYTES);
            let body = ByteStream::read_from()
                .path(blob_path)
                .offset(offset)
                .length(Length::Exact(part_size))
                .build()
                .await
                .with_context(|| {
                    format!(
                        "could not stream multipart restore part {part_number} for {}",
                        entry.key
                    )
                })?;
            let uploaded = store
                .client
                .upload_part()
                .bucket(&store.bucket)
                .key(&entry.key)
                .upload_id(&upload_id)
                .part_number(part_number)
                .content_length(i64::try_from(part_size).context("part size exceeds i64")?)
                .body(body)
                .send()
                .await
                .with_context(|| {
                    format!(
                        "could not upload multipart restore part {part_number} for {}",
                        entry.key
                    )
                })?;
            let etag = uploaded
                .e_tag()
                .filter(|value| !value.is_empty())
                .context("multipart restore part omitted its ETag")?;
            completed_parts.push(
                CompletedPart::builder()
                    .part_number(part_number)
                    .e_tag(etag)
                    .build(),
            );
            offset = offset
                .checked_add(part_size)
                .context("multipart restore offset overflowed")?;
            part_number = part_number
                .checked_add(1)
                .context("multipart restore exceeded its part count")?;
        }

        let completed = store
            .client
            .complete_multipart_upload()
            .bucket(&store.bucket)
            .key(&entry.key)
            .upload_id(&upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(completed_parts))
                    .build(),
            )
            .send()
            .await
            .with_context(|| format!("could not complete multipart restore for {}", entry.key))?;
        completed
            .version_id()
            .filter(|value| !value.is_empty() && *value != "null")
            .map(ToOwned::to_owned)
            .context("multipart restore omitted its exact version ID")
    }
    .await;

    if result.is_err() {
        let _ = store
            .client
            .abort_multipart_upload()
            .bucket(&store.bucket)
            .key(&entry.key)
            .upload_id(&upload_id)
            .send()
            .await;
    }
    result
}

fn should_use_multipart_restore(size: u64) -> bool {
    size >= MULTIPART_RESTORE_THRESHOLD_BYTES
}

fn multipart_restore_part_count(size: u64) -> u64 {
    size.div_ceil(MULTIPART_RESTORE_PART_BYTES)
}

async fn reconcile_restore_journal(
    store: &ObjectStore,
    manifest: &ObjectBackupManifest,
    mapping_path: &Path,
    mapping: &mut ObjectRestoreMap,
) -> Result<()> {
    let listed = list_events(store).await?;
    let completed: HashSet<_> = mapping
        .entries
        .iter()
        .map(|entry| {
            (
                entry.kind.as_str(),
                entry.key.as_str(),
                entry.restored_version_id.as_str(),
            )
        })
        .collect();
    for restored in &mapping.entries {
        ensure!(
            listed.iter().any(|event| {
                event.kind == restored.kind
                    && event.key == restored.key
                    && event.version_id == restored.restored_version_id
            }),
            "restore journal names a target version that no longer exists: {}",
            restored.key
        );
    }
    let extras = listed
        .iter()
        .filter(|event| {
            !completed.contains(&(event.kind, event.key.as_str(), event.version_id.as_str()))
        })
        .collect::<Vec<_>>();

    if let Some(pending) = mapping.pending.clone() {
        ensure!(
            extras.len() <= 1,
            "restore target contains multiple unjournaled versions while one source event is pending"
        );
        if let Some(extra) = extras.first() {
            ensure!(
                extra.kind == pending.kind && extra.key == pending.key,
                "unjournaled target version does not match the pending restore event"
            );
            let source = manifest
                .entries
                .iter()
                .find(|entry| {
                    entry.kind == pending.kind
                        && entry.key == pending.key
                        && entry.source_version_id == pending.source_version_id
                })
                .context("pending restore event is absent from the source manifest")?;
            if source.kind == "object_version" {
                verify_remote_object(store, source, &extra.version_id).await?;
            }
            mapping.entries.push(ObjectRestoreEntry {
                kind: pending.kind,
                key: pending.key,
                source_version_id: pending.source_version_id,
                restored_version_id: extra.version_id.clone(),
                sha256: pending.sha256,
                size_bytes: pending.size_bytes,
            });
            mapping.pending = None;
            mapping.restored_at = now();
            refresh_restore_summary(mapping)?;
            validate_restore_map(mapping)?;
            write_json_atomic(mapping_path, mapping)?;
        }
    } else {
        ensure!(
            extras.is_empty(),
            "restore target contains an unjournaled object version"
        );
    }
    Ok(())
}

pub async fn cleanup_restore(store: &ObjectStore, mapping_path: &Path) -> Result<()> {
    let mapping = load_restore_map(mapping_path)?;
    validate_restore_map(&mapping)?;
    ensure!(
        mapping.target_bucket == store.bucket,
        "restore mapping targets bucket {}, not configured bucket {}",
        mapping.target_bucket,
        store.bucket
    );
    abort_all_multipart_uploads(store).await?;
    if mapping.format == RESTORE_MAP_FORMAT && mapping.target_was_empty {
        let listed = list_events(store).await?;
        cleanup_listed_entries(store, &listed).await?;
    } else {
        cleanup_entries(store, &mapping.entries).await?;
    }
    let remaining = list_events(store).await?;
    ensure!(
        remaining.is_empty(),
        "restore cleanup left {} unexpected object versions in target bucket {}; they were not deleted",
        remaining.len(),
        store.bucket
    );
    Ok(())
}

async fn cleanup_listed_entries(store: &ObjectStore, entries: &[ListedEvent]) -> Result<()> {
    for entry in entries.iter().rev() {
        store
            .client
            .delete_object()
            .bucket(&store.bucket)
            .key(&entry.key)
            .version_id(&entry.version_id)
            .send()
            .await
            .with_context(|| {
                format!(
                    "could not delete restored object {} version {}",
                    entry.key, entry.version_id
                )
            })?;
    }
    Ok(())
}

async fn cleanup_entries(store: &ObjectStore, entries: &[ObjectRestoreEntry]) -> Result<()> {
    for entry in entries.iter().rev() {
        store
            .client
            .delete_object()
            .bucket(&store.bucket)
            .key(&entry.key)
            .version_id(&entry.restored_version_id)
            .send()
            .await
            .with_context(|| {
                format!(
                    "could not delete restored object {} version {}",
                    entry.key, entry.restored_version_id
                )
            })?;
    }
    Ok(())
}

pub async fn verify_restore_for_database_remap(
    store: &ObjectStore,
    input: &Path,
    mapping_path: &Path,
) -> Result<ObjectRestoreMap> {
    let manifest = verify(input).await?;
    let manifest_hash = hash_file(&input.join("manifest.json")).await?.0;
    let source_manifest_sha256 = format!("sha256:{manifest_hash}");
    require_enabled_versioning(store).await?;
    ensure_no_multipart_uploads(store).await?;
    let mapping = load_restore_map(mapping_path)?;
    validate_restore_map(&mapping)?;
    ensure!(
        mapping.complete && mapping.pending.is_none(),
        "database remap requires a complete object restore journal"
    );
    ensure!(
        mapping.target_bucket == store.bucket,
        "restore journal targets bucket {}, not configured bucket {}",
        mapping.target_bucket,
        store.bucket
    );
    ensure!(
        mapping.source_bucket == manifest.source_bucket
            && mapping.source_manifest_sha256 == source_manifest_sha256,
        "restore journal does not belong to this source manifest"
    );
    validate_restore_map_against_manifest(&mapping, &manifest, true)?;
    verify_restored_inventory(store, &manifest, &mapping).await?;
    Ok(mapping)
}

pub async fn pin_legacy_database_references(
    database_url: &str,
    store: &ObjectStore,
) -> Result<DatabasePinSummary> {
    require_enabled_versioning(store).await?;
    ensure_no_multipart_uploads(store).await?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
        .context("could not connect to PostgreSQL for legacy object pinning")?;
    let mut transaction = pool
        .begin()
        .await
        .context("could not begin legacy object pinning transaction")?;

    let user_ids = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT user_id
         FROM (
           SELECT user_id FROM straylight.asset_versions
           WHERE object_version_id IS NULL
             AND NOT (
               object_key::text = user_id::text || '/deleted-assets/'
                 || asset_id::text || '/' || version::text
               AND content_hash::text =
                 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
               AND size_bytes=0
               AND media_type::text='application/x-straylight-deleted'
               AND metadata ? 'redacted_by_deletion_job'
             )
           UNION
           SELECT user_id FROM straylight.account_exports
           WHERE status='ready' AND object_key IS NOT NULL
             AND object_version_id IS NULL
         ) AS affected
         ORDER BY user_id",
    )
    .fetch_all(&mut *transaction)
    .await
    .context("could not enumerate users with legacy object locators")?;
    for user_id in &user_ids {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('storage:' || $1::text,0))")
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .context("could not lock a user for legacy object pinning")?;
    }

    let asset_rows = sqlx::query(
        "SELECT user_id,asset_id,version,bucket,object_key,
                content_hash::text AS content_hash,size_bytes
         FROM straylight.asset_versions
         WHERE object_version_id IS NULL
           AND NOT (
             object_key::text = user_id::text || '/deleted-assets/'
               || asset_id::text || '/' || version::text
             AND content_hash::text =
               'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
             AND size_bytes=0
             AND media_type::text='application/x-straylight-deleted'
             AND metadata ? 'redacted_by_deletion_job'
           )
         ORDER BY user_id,asset_id,version
         FOR UPDATE",
    )
    .fetch_all(&mut *transaction)
    .await
    .context("could not lock legacy immutable asset rows")?;
    let export_rows = sqlx::query(
        "SELECT user_id,id,object_key,content_hash::text AS content_hash,size_bytes
         FROM straylight.account_exports
         WHERE status='ready' AND object_key IS NOT NULL
           AND object_version_id IS NULL
         ORDER BY user_id,id
         FOR UPDATE",
    )
    .fetch_all(&mut *transaction)
    .await
    .context("could not lock legacy account-export rows")?;

    let mut asset_mapping = Vec::with_capacity(asset_rows.len());
    let mut verified_objects = HashMap::<String, (String, i64, super::StoredFile)>::new();
    for row in &asset_rows {
        let bucket: String = row.try_get("bucket")?;
        ensure!(
            bucket == store.bucket,
            "legacy asset references bucket {bucket}, not configured bucket {}",
            store.bucket
        );
        let object_key: String = row.try_get("object_key")?;
        let content_hash: String = row.try_get("content_hash")?;
        let size_bytes: i64 = row.try_get("size_bytes")?;
        let actual = verified_latest_object_cached(
            store,
            &mut verified_objects,
            &object_key,
            &content_hash,
            size_bytes,
        )
        .await?;
        let object_version_id = actual
            .object_version_id
            .context("object store returned no exact version ID while pinning a legacy asset")?;
        asset_mapping.push(serde_json::json!({
            "user_id": row.try_get::<uuid::Uuid,_>("user_id")?,
            "asset_id": row.try_get::<uuid::Uuid,_>("asset_id")?,
            "asset_version": row.try_get::<i32,_>("version")?,
            "object_key": object_key,
            "object_version_id": object_version_id,
            "content_hash": content_hash,
            "size_bytes": size_bytes
        }));
    }
    let asset_versions_pinned = sqlx::query_scalar::<_, i64>(
        "SELECT straylight.pin_legacy_asset_object_versions($1::jsonb)",
    )
    .bind(serde_json::Value::Array(asset_mapping))
    .fetch_one(&mut *transaction)
    .await
    .context("could not pin legacy immutable asset object versions")?;

    let upload_canonical_versions_pinned = 0_u64;

    let mut account_export_versions_pinned = 0_u64;
    for row in &export_rows {
        let object_key: String = row.try_get("object_key")?;
        let content_hash: Option<String> = row.try_get("content_hash")?;
        let size_bytes: Option<i64> = row.try_get("size_bytes")?;
        let content_hash = content_hash.context("ready legacy account export has no SHA-256")?;
        let size_bytes = size_bytes.context("ready legacy account export has no size")?;
        let actual = verified_latest_object_cached(
            store,
            &mut verified_objects,
            &object_key,
            &content_hash,
            size_bytes,
        )
        .await?;
        let object_version_id = actual
            .object_version_id
            .context("object store returned no exact version ID while pinning an account export")?;
        let updated = sqlx::query(
            "UPDATE straylight.account_exports
             SET object_version_id=$1
             WHERE user_id=$2 AND id=$3 AND object_key=$4
               AND content_hash=$5 AND size_bytes=$6
               AND object_version_id IS NULL AND status='ready'",
        )
        .bind(object_version_id)
        .bind(row.try_get::<uuid::Uuid, _>("user_id")?)
        .bind(row.try_get::<uuid::Uuid, _>("id")?)
        .bind(&object_key)
        .bind(&content_hash)
        .bind(size_bytes)
        .execute(&mut *transaction)
        .await
        .context("could not pin an account-export object version")?
        .rows_affected();
        ensure!(
            updated == 1,
            "account export changed while it was being pinned"
        );
        account_export_versions_pinned += updated;
    }

    transaction
        .commit()
        .await
        .context("could not commit legacy object-version pinning")?;
    pool.close().await;
    Ok(DatabasePinSummary {
        asset_versions_pinned: u64::try_from(asset_versions_pinned)
            .context("asset pin count was negative")?,
        upload_canonical_versions_pinned,
        account_export_versions_pinned,
        objects_stream_verified: u64::try_from(verified_objects.len())
            .context("verified object count overflowed")?,
    })
}

pub async fn verify_database_references(
    database_url: &str,
    store: &ObjectStore,
) -> Result<DatabaseReferenceSummary> {
    require_enabled_versioning(store).await?;
    ensure_no_multipart_uploads(store).await?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
        .context("could not connect to PostgreSQL for object-reference verification")?;
    let rows = sqlx::query(
        "SELECT relation,object_bucket,object_key,object_version_id,
                content_hash,size_bytes
         FROM (
	           SELECT 'asset_versions'::text AS relation,bucket AS object_bucket,
	                  object_key,object_version_id,
	                  content_hash::text AS content_hash,size_bytes
	           FROM straylight.asset_versions
	           WHERE NOT (
	             object_key::text = user_id::text || '/deleted-assets/'
	               || asset_id::text || '/' || version::text
	             AND content_hash::text =
	               'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
	             AND size_bytes=0
	             AND media_type::text='application/x-straylight-deleted'
	             AND metadata ? 'redacted_by_deletion_job'
	           )
           UNION ALL
           SELECT 'account_exports',$1::text,object_key,object_version_id,
                  content_hash::text,size_bytes
           FROM straylight.account_exports
           WHERE status='ready' AND object_key IS NOT NULL
           UNION ALL
           SELECT 'entry_versions',$1::text,object_key,object_version_id,
                  content_sha256::text,size_bytes
           FROM straylight.entry_versions
           WHERE object_key IS NOT NULL
         ) AS authoritative
         ORDER BY relation,object_key,object_version_id NULLS FIRST",
    )
    .bind(&store.bucket)
    .fetch_all(&pool)
    .await
    .context("could not enumerate authoritative database object references")?;

    let mut unique = BTreeMap::<(String, String), (String, u64)>::new();
    let mut references_verified = 0_u64;
    let mut logical_bytes_verified = 0_u64;
    for row in rows {
        let relation: String = row.try_get("relation")?;
        let bucket: String = row.try_get("object_bucket")?;
        let object_key: String = row.try_get("object_key")?;
        let object_version_id: Option<String> = row.try_get("object_version_id")?;
        let content_hash: Option<String> = row.try_get("content_hash")?;
        let size_bytes: Option<i64> = row.try_get("size_bytes")?;
        ensure!(
            bucket == store.bucket,
            "{relation} references bucket {bucket}, not configured bucket {}",
            store.bucket
        );
        let object_version_id = object_version_id.with_context(|| {
            format!("{relation}:{object_key} is not pinned to an exact object version")
        })?;
        ensure!(
            !object_version_id.trim().is_empty() && object_version_id != "null",
            "{relation}:{object_key} has an invalid object version ID"
        );
        let content_hash =
            content_hash.with_context(|| format!("{relation}:{object_key} has no SHA-256"))?;
        let size_bytes =
            size_bytes.with_context(|| format!("{relation}:{object_key} has no size"))?;
        let size_bytes = u64::try_from(size_bytes)
            .with_context(|| format!("{relation}:{object_key} has a negative size"))?;
        let identity = (content_hash.clone(), size_bytes);
        if let Some(existing) = unique.insert(
            (object_key.clone(), object_version_id.clone()),
            identity.clone(),
        ) {
            ensure!(
                existing == identity,
                "database references disagree about {object_key} version {object_version_id}"
            );
        }
        references_verified += 1;
        logical_bytes_verified = logical_bytes_verified
            .checked_add(size_bytes)
            .context("verified logical byte count overflowed")?;
    }

    for ((object_key, object_version_id), (content_hash, size_bytes)) in &unique {
        store
            .verify_object_content_version(
                object_key,
                Some(object_version_id),
                content_hash,
                *size_bytes,
            )
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "database reference {object_key} version {object_version_id} failed streamed integrity verification: {error}"
                )
            })?;
    }
    pool.close().await;
    Ok(DatabaseReferenceSummary {
        references_verified,
        unique_object_versions_verified: u64::try_from(unique.len())
            .context("unique object-version count overflowed")?,
        logical_bytes_verified,
    })
}

async fn verified_latest_object(
    store: &ObjectStore,
    object_key: &str,
    expected_hash: &str,
    expected_size: i64,
) -> Result<super::StoredFile> {
    let expected_size =
        u64::try_from(expected_size).context("database object size was negative")?;
    let mut actual = store
        .hash_stream(object_key, None)
        .await
        .map_err(|error| anyhow::anyhow!("could not stream {object_key}: {error}"))?;
    ensure!(
        actual.sha256 == expected_hash.trim_start_matches("sha256:")
            && actual.size_bytes == expected_size,
        "latest object {object_key} does not match its database SHA-256 and size"
    );
    if !actual
        .object_version_id
        .as_deref()
        .is_some_and(|version| !version.trim().is_empty() && version != "null")
    {
        let copied = store
            .client
            .copy_object()
            .bucket(&store.bucket)
            .key(object_key)
            .copy_source(super::versioned_copy_source(
                &store.bucket,
                object_key,
                None,
            ))
            .send()
            .await
            .with_context(|| {
                format!("could not create a versioned copy of legacy object {object_key}")
            })?;
        let version_id = copied
            .version_id()
            .filter(|value| !value.trim().is_empty() && *value != "null")
            .context("versioned legacy object copy returned no exact provider version ID")?
            .to_owned();
        actual = store
            .hash_stream(object_key, Some(&version_id))
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "could not verify newly versioned legacy object {object_key}: {error}"
                )
            })?;
        ensure!(
            actual.sha256 == expected_hash.trim_start_matches("sha256:")
                && actual.size_bytes == expected_size
                && actual.object_version_id.as_deref() == Some(version_id.as_str()),
            "newly versioned legacy object {object_key} changed identity"
        );
    }
    Ok(actual)
}

async fn verified_latest_object_cached(
    store: &ObjectStore,
    cache: &mut HashMap<String, (String, i64, super::StoredFile)>,
    object_key: &str,
    expected_hash: &str,
    expected_size: i64,
) -> Result<super::StoredFile> {
    if let Some((content_hash, size_bytes, stored)) = cache.get(object_key) {
        ensure!(
            content_hash == expected_hash && *size_bytes == expected_size,
            "database rows disagree about legacy object {object_key}"
        );
        return Ok(stored.clone());
    }
    let stored = verified_latest_object(store, object_key, expected_hash, expected_size).await?;
    cache.insert(
        object_key.to_owned(),
        (expected_hash.to_owned(), expected_size, stored.clone()),
    );
    Ok(stored)
}

pub async fn remap_database(
    database_url: &str,
    mapping: &ObjectRestoreMap,
) -> Result<DatabaseRemapSummary> {
    validate_restore_map(mapping)?;
    ensure!(
        mapping.complete && mapping.pending.is_none(),
        "database remap requires a complete object restore journal"
    );
    let object_entries: Vec<_> = mapping
        .entries
        .iter()
        .filter(|entry| entry.kind == "object_version")
        .collect();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
        .context("could not connect to the restored PostgreSQL database")?;
    let mut transaction = pool
        .begin()
        .await
        .context("could not begin object-version remap transaction")?;
    sqlx::query(
        "CREATE TEMP TABLE object_version_restore_map (
           object_key text NOT NULL,
           source_version_id text NOT NULL,
           restored_version_id text NOT NULL,
           source_bucket text NOT NULL,
           restored_bucket text NOT NULL,
           content_hash text NOT NULL,
           size_bytes bigint NOT NULL,
           PRIMARY KEY (object_key,source_version_id)
         ) ON COMMIT DROP",
    )
    .execute(&mut *transaction)
    .await
    .context("could not create temporary object-version remap table")?;

    for chunk in object_entries.chunks(500) {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO object_version_restore_map (
               object_key,source_version_id,restored_version_id,
               source_bucket,restored_bucket,content_hash,size_bytes
             ) ",
        );
        query.push_values(chunk, |mut row, entry| {
            row.push_bind(&entry.key)
                .push_bind(&entry.source_version_id)
                .push_bind(&entry.restored_version_id)
                .push_bind(&mapping.source_bucket)
                .push_bind(&mapping.target_bucket)
                .push_bind(
                    entry
                        .sha256
                        .as_deref()
                        .and_then(|value| value.strip_prefix("sha256:"))
                        .expect("validated object restore entry has a SHA-256"),
                )
                .push_bind(
                    i64::try_from(
                        entry
                            .size_bytes
                            .expect("validated object restore entry has a size"),
                    )
                    .expect("portable object size fits i64"),
                );
        });
        query
            .build()
            .execute(&mut *transaction)
            .await
            .context("could not load object-version remap entries")?;
    }

    let missing = sqlx::query(
        "WITH referenced AS (
           SELECT 'asset_versions' AS relation,object_key,object_version_id,
                  false AS may_be_absent
	           FROM straylight.asset_versions
	           WHERE object_version_id IS NOT NULL
	             AND NOT (
	               object_key::text = user_id::text || '/deleted-assets/'
	                 || asset_id::text || '/' || version::text
	               AND content_hash::text =
	                 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
	               AND size_bytes=0
	               AND media_type::text='application/x-straylight-deleted'
	               AND metadata ? 'redacted_by_deletion_job'
	             )
           UNION ALL
           SELECT 'account_exports',object_key,object_version_id,false
           FROM straylight.account_exports
           WHERE object_key IS NOT NULL AND object_version_id IS NOT NULL
           UNION ALL
           SELECT 'entry_versions',object_key,object_version_id,false
           FROM straylight.entry_versions
           WHERE object_key IS NOT NULL AND object_version_id IS NOT NULL
         )
         SELECT referenced.relation,
                referenced.object_key,
                referenced.object_version_id
         FROM referenced
         LEFT JOIN object_version_restore_map mapping
           ON mapping.object_key=referenced.object_key
          AND referenced.object_version_id IN (
            mapping.source_version_id,
            mapping.restored_version_id
          )
         WHERE mapping.restored_version_id IS NULL
           AND NOT referenced.may_be_absent
         ORDER BY referenced.relation,referenced.object_key
         LIMIT 20",
    )
    .fetch_all(&mut *transaction)
    .await
    .context("could not validate restored database object references")?;
    if !missing.is_empty() {
        let examples = missing
            .iter()
            .map(|row| {
                format!(
                    "{}:{}:{}",
                    row.get::<String, _>("relation"),
                    row.get::<String, _>("object_key"),
                    row.get::<String, _>("object_version_id")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        bail!("restore map is missing authoritative PostgreSQL object references: {examples}");
    }

    let cleaned_upload_versions_cleared = 0_u64;
    let asset_mapping = object_entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "object_key": entry.key,
                "source_version_id": entry.source_version_id,
                "restored_version_id": entry.restored_version_id,
                "source_bucket": mapping.source_bucket,
                "restored_bucket": mapping.target_bucket,
                "content_hash": entry.sha256.as_deref()
                    .and_then(|value| value.strip_prefix("sha256:")),
                "size_bytes": entry.size_bytes,
            })
        })
        .collect::<Vec<_>>();
    let asset_versions_updated =
        sqlx::query_scalar::<_, i64>("SELECT straylight.remap_asset_object_versions($1::jsonb)")
            .bind(serde_json::Value::Array(asset_mapping))
            .fetch_one(&mut *transaction)
            .await
            .context("could not remap immutable asset-version object locators")?;
    let asset_versions_updated =
        u64::try_from(asset_versions_updated).context("asset remap returned a negative count")?;
    let workspace_entry_versions_updated = sqlx::query(
        "UPDATE straylight.entry_versions version
         SET object_version_id=mapping.restored_version_id
         FROM object_version_restore_map mapping
         WHERE mapping.object_key=version.object_key
           AND mapping.source_version_id=version.object_version_id",
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    let upload_temporary_versions_updated = 0_u64;
    let upload_canonical_versions_updated = 0_u64;
    let account_exports_updated = sqlx::query(
        "UPDATE straylight.account_exports export
         SET object_version_id=mapping.restored_version_id
         FROM object_version_restore_map mapping
         WHERE mapping.object_key=export.object_key
           AND mapping.source_version_id=export.object_version_id",
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    let database_reference_issues = sqlx::query(
        "WITH referenced AS (
           SELECT 'asset_versions' AS relation,
                  bucket AS object_bucket,
                  object_key,
                  object_version_id,
                  content_hash::text AS content_hash,
                  size_bytes
	           FROM straylight.asset_versions
	           WHERE object_version_id IS NOT NULL
	             AND NOT (
	               object_key::text = user_id::text || '/deleted-assets/'
	                 || asset_id::text || '/' || version::text
	               AND content_hash::text =
	                 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
	               AND size_bytes=0
	               AND media_type::text='application/x-straylight-deleted'
	               AND metadata ? 'redacted_by_deletion_job'
	             )
           UNION ALL
           SELECT 'account_exports',
                  NULL::text,
                  object_key,
                  object_version_id,
                  content_hash::text,
                  size_bytes
           FROM straylight.account_exports
           WHERE object_key IS NOT NULL
             AND object_version_id IS NOT NULL
           UNION ALL
           SELECT 'entry_versions',
                  NULL::text,
                  object_key,
                  object_version_id,
                  content_sha256::text,
                  size_bytes
           FROM straylight.entry_versions
           WHERE object_key IS NOT NULL
             AND object_version_id IS NOT NULL
         )
         SELECT referenced.relation,
                referenced.object_key,
                referenced.object_version_id
         FROM referenced
         LEFT JOIN object_version_restore_map AS mapping
           ON mapping.object_key=referenced.object_key
          AND mapping.restored_version_id=referenced.object_version_id
          AND (
            referenced.object_bucket IS NULL
            OR referenced.object_bucket=mapping.restored_bucket
          )
         WHERE mapping.restored_version_id IS NULL
            OR referenced.content_hash IS NULL
            OR referenced.size_bytes IS NULL
            OR mapping.content_hash<>referenced.content_hash
            OR mapping.size_bytes<>referenced.size_bytes
         ORDER BY referenced.relation,referenced.object_key
         LIMIT 20",
    )
    .fetch_all(&mut *transaction)
    .await
    .context("could not verify remapped PostgreSQL object references")?;
    if !database_reference_issues.is_empty() {
        let examples = database_reference_issues
            .iter()
            .map(|row| {
                format!(
                    "{}:{}:{}",
                    row.get::<String, _>("relation"),
                    row.get::<String, _>("object_key"),
                    row.get::<String, _>("object_version_id")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        bail!("remapped PostgreSQL object references failed identity checks: {examples}");
    }
    let database_references_verified = sqlx::query_scalar::<_, i64>(
        "SELECT
	           (SELECT count(*) FROM straylight.asset_versions
	            WHERE object_version_id IS NOT NULL
	              AND NOT (
	                object_key::text = user_id::text || '/deleted-assets/'
	                  || asset_id::text || '/' || version::text
	                AND content_hash::text =
	                  'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
	                AND size_bytes=0
	                AND media_type::text='application/x-straylight-deleted'
	                AND metadata ? 'redacted_by_deletion_job'
	              ))
           + (SELECT count(*) FROM straylight.account_exports
              WHERE object_version_id IS NOT NULL)
           + (SELECT count(*) FROM straylight.entry_versions
              WHERE object_version_id IS NOT NULL)",
    )
    .fetch_one(&mut *transaction)
    .await
    .context("could not count verified PostgreSQL object references")?;
    let database_references_verified = u64::try_from(database_references_verified)
        .context("verified database reference count was negative")?;

    transaction
        .commit()
        .await
        .context("could not commit object-version remap")?;
    pool.close().await;
    Ok(DatabaseRemapSummary {
        mapping_entries: object_entries.len(),
        asset_versions_updated,
        workspace_entry_versions_updated,
        upload_temporary_versions_updated,
        upload_canonical_versions_updated,
        account_exports_updated,
        cleaned_upload_versions_cleared,
        database_references_verified,
    })
}

async fn require_enabled_versioning(store: &ObjectStore) -> Result<String> {
    store.ensure_bucket().await?;
    let output = store
        .client
        .get_bucket_versioning()
        .bucket(&store.bucket)
        .send()
        .await
        .context("could not inspect object backup bucket versioning")?;
    let status = output
        .status()
        .map(|value| value.as_str().to_owned())
        .unwrap_or_else(|| "Unset".to_owned());
    ensure!(
        status == "Enabled",
        "portable object recovery requires bucket versioning; status={status}"
    );
    Ok(status)
}

async fn ensure_no_multipart_uploads(store: &ObjectStore) -> Result<()> {
    let output = store
        .client
        .list_multipart_uploads()
        .bucket(&store.bucket)
        .max_uploads(1)
        .send()
        .await
        .context(
            "could not inventory multipart uploads; recovery refuses to continue without this permission",
        )?;
    ensure!(
        output.uploads().is_empty(),
        "object bucket contains an incomplete multipart upload; finish or abort it before backup or restore"
    );
    Ok(())
}

async fn abort_all_multipart_uploads(store: &ObjectStore) -> Result<()> {
    let mut key_marker = None;
    let mut upload_id_marker = None;
    let mut uploads = Vec::new();
    loop {
        let mut request = store.client.list_multipart_uploads().bucket(&store.bucket);
        if let Some(marker) = key_marker.as_deref() {
            request = request.key_marker(marker);
        }
        if let Some(marker) = upload_id_marker.as_deref() {
            request = request.upload_id_marker(marker);
        }
        let output = request.send().await.context(
            "could not inventory interrupted multipart restores in the dedicated target bucket",
        )?;
        for upload in output.uploads() {
            let key = upload
                .key()
                .context("multipart restore inventory omitted its key")?
                .to_owned();
            let upload_id = upload
                .upload_id()
                .context("multipart restore inventory omitted its upload ID")?
                .to_owned();
            uploads.push((key, upload_id));
        }
        if !output.is_truncated().unwrap_or(false) {
            break;
        }
        key_marker = output.next_key_marker().map(ToOwned::to_owned);
        upload_id_marker = output.next_upload_id_marker().map(ToOwned::to_owned);
        ensure!(
            key_marker.is_some(),
            "truncated multipart restore inventory omitted next_key_marker"
        );
    }
    for (key, upload_id) in uploads {
        store
            .client
            .abort_multipart_upload()
            .bucket(&store.bucket)
            .key(&key)
            .upload_id(&upload_id)
            .send()
            .await
            .with_context(|| {
                format!("could not abort interrupted multipart restore {upload_id} for {key}")
            })?;
    }
    ensure_no_multipart_uploads(store).await
}

async fn list_events(store: &ObjectStore) -> Result<Vec<ListedEvent>> {
    let mut events = Vec::new();
    let mut key_marker = None;
    let mut version_id_marker = None;
    loop {
        let mut request = store.client.list_object_versions().bucket(&store.bucket);
        if let Some(marker) = key_marker.as_deref() {
            request = request.key_marker(marker);
        }
        if let Some(marker) = version_id_marker.as_deref() {
            request = request.version_id_marker(marker);
        }
        let output = request
            .send()
            .await
            .context("could not list complete object-version inventory")?;
        for version in output.versions() {
            let modified = version
                .last_modified()
                .context("object version inventory omitted last_modified")?;
            let size = version
                .size()
                .context("object version inventory omitted size")?;
            events.push(ListedEvent {
                kind: "object_version",
                key: version
                    .key()
                    .context("object version inventory omitted key")?
                    .to_owned(),
                version_id: version
                    .version_id()
                    .context("object version inventory omitted version ID")?
                    .to_owned(),
                is_latest: version.is_latest().unwrap_or(false),
                last_modified_unix_seconds: modified.secs(),
                last_modified_subsecond_nanos: modified.subsec_nanos(),
                size_bytes: Some(u64::try_from(size).context("object version size is negative")?),
                etag: version.e_tag().map(ToOwned::to_owned),
            });
        }
        for marker in output.delete_markers() {
            let modified = marker
                .last_modified()
                .context("delete marker inventory omitted last_modified")?;
            events.push(ListedEvent {
                kind: "delete_marker",
                key: marker
                    .key()
                    .context("delete marker inventory omitted key")?
                    .to_owned(),
                version_id: marker
                    .version_id()
                    .context("delete marker inventory omitted version ID")?
                    .to_owned(),
                is_latest: marker.is_latest().unwrap_or(false),
                last_modified_unix_seconds: modified.secs(),
                last_modified_subsecond_nanos: modified.subsec_nanos(),
                size_bytes: None,
                etag: None,
            });
        }
        if !output.is_truncated().unwrap_or(false) {
            break;
        }
        key_marker = output.next_key_marker().map(ToOwned::to_owned);
        version_id_marker = output.next_version_id_marker().map(ToOwned::to_owned);
        ensure!(
            key_marker.is_some(),
            "truncated object inventory omitted next_key_marker"
        );
    }
    events.sort_by(|left, right| {
        (
            &left.key,
            left.last_modified_unix_seconds,
            left.last_modified_subsecond_nanos,
            left.kind,
            &left.version_id,
        )
            .cmp(&(
                &right.key,
                right.last_modified_unix_seconds,
                right.last_modified_subsecond_nanos,
                right.kind,
                &right.version_id,
            ))
    });
    Ok(events)
}

fn validate_latest_events(events: &[ListedEvent]) -> Result<()> {
    let mut latest_per_key: HashMap<&str, usize> = HashMap::new();
    for event in events {
        if event.is_latest {
            *latest_per_key.entry(&event.key).or_default() += 1;
        }
    }
    for key in events
        .iter()
        .map(|event| event.key.as_str())
        .collect::<HashSet<_>>()
    {
        ensure!(
            latest_per_key.get(key) == Some(&1),
            "object inventory must identify exactly one latest event for key {key}"
        );
    }
    Ok(())
}

fn validate_manifest(manifest: &ObjectBackupManifest) -> Result<()> {
    ensure!(
        manifest.format == BACKUP_FORMAT,
        "unsupported object backup format: {}",
        manifest.format
    );
    ensure!(
        manifest.bucket_versioning == "Enabled",
        "object backup was not captured from a versioned bucket"
    );
    ensure!(
        !manifest.source_bucket.trim().is_empty(),
        "object backup omitted source bucket"
    );
    let mut identities = HashSet::new();
    let mut latest = HashMap::<&str, usize>::new();
    let mut object_versions = 0usize;
    let mut delete_markers = 0usize;
    let mut unique_blobs = HashSet::new();
    let mut logical_bytes = 0u64;
    for entry in &manifest.entries {
        ensure!(
            !entry.key.is_empty() && !entry.key.chars().any(char::is_control),
            "object backup contains an invalid key"
        );
        ensure!(
            !entry.source_version_id.is_empty()
                && !entry.source_version_id.chars().any(char::is_control),
            "object backup contains an invalid source version ID"
        );
        ensure!(
            identities.insert((
                entry.kind.as_str(),
                entry.key.as_str(),
                entry.source_version_id.as_str()
            )),
            "object backup contains a duplicate version identity"
        );
        if entry.is_latest {
            *latest.entry(&entry.key).or_default() += 1;
        }
        match entry.kind.as_str() {
            "object_version" => {
                object_versions += 1;
                let size = entry.size_bytes.context("object version omitted size")?;
                let digest = digest_from_entry(entry)?;
                ensure!(
                    size <= MAX_PORTABLE_OBJECT_BYTES,
                    "object version exceeds portable restore limit"
                );
                unique_blobs.insert(digest);
                logical_bytes = logical_bytes
                    .checked_add(size)
                    .context("object backup logical byte count overflowed")?;
            }
            "delete_marker" => {
                delete_markers += 1;
                ensure!(
                    entry.size_bytes.is_none()
                        && entry.sha256.is_none()
                        && entry.content_type.is_none()
                        && entry.metadata.is_empty(),
                    "delete marker contains object payload fields"
                );
            }
            other => bail!("unknown object backup entry kind: {other}"),
        }
    }
    for key in manifest
        .entries
        .iter()
        .map(|entry| entry.key.as_str())
        .collect::<HashSet<_>>()
    {
        ensure!(
            latest.get(key) == Some(&1),
            "object backup must identify exactly one latest event for key {key}"
        );
    }
    ensure!(
        manifest.summary.object_versions == object_versions
            && manifest.summary.delete_markers == delete_markers
            && manifest.summary.unique_blobs == unique_blobs.len()
            && manifest.summary.logical_bytes == logical_bytes,
        "object backup summary does not match its entries"
    );
    Ok(())
}

fn validate_restore_map(mapping: &ObjectRestoreMap) -> Result<()> {
    ensure!(
        matches!(
            mapping.format.as_str(),
            RESTORE_MAP_FORMAT | LEGACY_RESTORE_MAP_FORMAT
        ),
        "unsupported object restore map format: {}",
        mapping.format
    );
    ensure!(
        !mapping.source_bucket.is_empty() && !mapping.target_bucket.is_empty(),
        "object restore map omitted a bucket"
    );
    ensure!(
        mapping.format != RESTORE_MAP_FORMAT || mapping.target_was_empty,
        "resumable restore journal does not prove that its target started empty"
    );
    ensure!(
        !mapping.complete || mapping.pending.is_none(),
        "complete restore map still contains a pending event"
    );
    ensure!(
        mapping
            .source_manifest_sha256
            .strip_prefix("sha256:")
            .is_some_and(valid_digest),
        "object restore map contains an invalid source manifest hash"
    );
    let mut source = HashSet::new();
    let mut target = HashSet::new();
    let mut objects = 0usize;
    let mut markers = 0usize;
    let mut bytes = 0u64;
    for entry in &mapping.entries {
        ensure!(
            matches!(entry.kind.as_str(), "object_version" | "delete_marker"),
            "object restore map contains an invalid kind"
        );
        ensure!(
            source.insert((&entry.kind, &entry.key, &entry.source_version_id)),
            "object restore map contains a duplicate source identity"
        );
        ensure!(
            target.insert((&entry.kind, &entry.key, &entry.restored_version_id)),
            "object restore map contains a duplicate target identity"
        );
        if entry.kind == "object_version" {
            objects += 1;
            ensure!(
                entry
                    .sha256
                    .as_deref()
                    .and_then(|value| value.strip_prefix("sha256:"))
                    .is_some_and(valid_digest),
                "object restore map contains an invalid object hash"
            );
            bytes = bytes
                .checked_add(entry.size_bytes.context("restored object omitted size")?)
                .context("restore map byte count overflowed")?;
        } else {
            markers += 1;
            ensure!(
                entry.sha256.is_none() && entry.size_bytes.is_none(),
                "restored delete marker contains payload fields"
            );
        }
    }
    if let Some(pending) = &mapping.pending {
        ensure!(
            matches!(pending.kind.as_str(), "object_version" | "delete_marker"),
            "restore journal contains an invalid pending kind"
        );
        ensure!(
            !pending.key.is_empty()
                && !pending.key.chars().any(char::is_control)
                && !pending.source_version_id.is_empty()
                && !pending.source_version_id.chars().any(char::is_control),
            "restore journal contains an invalid pending identity"
        );
        ensure!(
            source.insert((&pending.kind, &pending.key, &pending.source_version_id)),
            "restore journal repeats a completed source identity as pending"
        );
        if pending.kind == "object_version" {
            ensure!(
                pending
                    .sha256
                    .as_deref()
                    .and_then(|value| value.strip_prefix("sha256:"))
                    .is_some_and(valid_digest)
                    && pending.size_bytes.is_some(),
                "pending object restore omitted its exact hash or size"
            );
        } else {
            ensure!(
                pending.sha256.is_none() && pending.size_bytes.is_none(),
                "pending delete marker contains payload fields"
            );
        }
    }
    ensure!(
        objects == mapping.summary.object_versions
            && markers == mapping.summary.delete_markers
            && bytes == mapping.summary.logical_bytes,
        "object restore map summary does not match its entries"
    );
    Ok(())
}

fn validate_restore_map_against_manifest(
    mapping: &ObjectRestoreMap,
    manifest: &ObjectBackupManifest,
    require_complete: bool,
) -> Result<()> {
    let source: HashMap<_, _> = manifest
        .entries
        .iter()
        .map(|entry| {
            (
                (
                    entry.kind.as_str(),
                    entry.key.as_str(),
                    entry.source_version_id.as_str(),
                ),
                entry,
            )
        })
        .collect();
    for restored in &mapping.entries {
        let original = source
            .get(&(
                restored.kind.as_str(),
                restored.key.as_str(),
                restored.source_version_id.as_str(),
            ))
            .context("restore map contains an entry absent from the source manifest")?;
        ensure!(
            restored.sha256 == original.sha256 && restored.size_bytes == original.size_bytes,
            "restore map changed source hash or size identity for {}",
            restored.key
        );
    }
    if let Some(pending) = &mapping.pending {
        let original = source
            .get(&(
                pending.kind.as_str(),
                pending.key.as_str(),
                pending.source_version_id.as_str(),
            ))
            .context("restore journal pending entry is absent from the source manifest")?;
        ensure!(
            pending.sha256 == original.sha256 && pending.size_bytes == original.size_bytes,
            "restore journal changed pending hash or size identity"
        );
    }
    if require_complete {
        ensure!(
            mapping.complete
                && mapping.pending.is_none()
                && mapping.entries.len() == manifest.entries.len()
                && mapping.summary.object_versions == manifest.summary.object_versions
                && mapping.summary.delete_markers == manifest.summary.delete_markers
                && mapping.summary.logical_bytes == manifest.summary.logical_bytes,
            "completed restore map does not cover the full source manifest"
        );
        let restored_source: HashSet<_> = mapping
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.kind.as_str(),
                    entry.key.as_str(),
                    entry.source_version_id.as_str(),
                )
            })
            .collect();
        ensure!(
            source
                .keys()
                .all(|identity| restored_source.contains(identity)),
            "completed restore map omitted a source manifest event"
        );
    }
    Ok(())
}

async fn verify_restored_inventory(
    store: &ObjectStore,
    manifest: &ObjectBackupManifest,
    mapping: &ObjectRestoreMap,
) -> Result<()> {
    let listed = list_events(store).await?;
    ensure!(
        listed.len() == mapping.entries.len(),
        "restored bucket contains {} events but restore map names {}",
        listed.len(),
        mapping.entries.len()
    );
    let actual: HashSet<_> = listed
        .iter()
        .map(|entry| {
            (
                entry.kind,
                entry.key.as_str(),
                entry.version_id.as_str(),
                entry.is_latest,
            )
        })
        .collect();
    let source_by_identity: HashMap<_, _> = manifest
        .entries
        .iter()
        .map(|entry| {
            (
                (
                    entry.kind.as_str(),
                    entry.key.as_str(),
                    entry.source_version_id.as_str(),
                ),
                entry,
            )
        })
        .collect();
    for entry in &mapping.entries {
        let source = source_by_identity
            .get(&(
                entry.kind.as_str(),
                entry.key.as_str(),
                entry.source_version_id.as_str(),
            ))
            .context("restore map entry is absent from source manifest")?;
        ensure!(
            actual.contains(&(
                entry.kind.as_str(),
                entry.key.as_str(),
                entry.restored_version_id.as_str(),
                source.is_latest
            )),
            "restored inventory does not contain the expected version for {}",
            entry.key
        );
        if source.kind == "object_version" {
            verify_remote_object(store, source, &entry.restored_version_id).await?;
        }
    }
    Ok(())
}

async fn verify_remote_object(
    store: &ObjectStore,
    entry: &ObjectBackupEntry,
    restored_version_id: &str,
) -> Result<()> {
    let response = store
        .client
        .get_object()
        .bucket(&store.bucket)
        .key(&entry.key)
        .version_id(restored_version_id)
        .send()
        .await
        .with_context(|| {
            format!(
                "could not verify restored object {} version {}",
                entry.key, restored_version_id
            )
        })?;
    ensure!(
        response.version_id() == Some(restored_version_id),
        "restored object verification returned the wrong version"
    );
    ensure!(
        response.content_type() == entry.content_type.as_deref()
            && response.cache_control() == entry.cache_control.as_deref()
            && response.content_disposition() == entry.content_disposition.as_deref()
            && response.content_encoding() == entry.content_encoding.as_deref()
            && response.content_language() == entry.content_language.as_deref(),
        "restored object headers differ for {}",
        entry.key
    );
    let metadata: BTreeMap<_, _> = response
        .metadata()
        .map(|values| {
            values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    ensure!(
        metadata == entry.metadata,
        "restored object metadata differs for {}",
        entry.key
    );
    let mut reader = response.body.into_async_read();
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .context("could not read restored object")?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size = size
            .checked_add(u64::try_from(count).unwrap_or(u64::MAX))
            .context("restored object size overflowed")?;
    }
    let digest = hex::encode(hasher.finalize());
    ensure!(
        entry.sha256.as_deref() == Some(format!("sha256:{digest}").as_str())
            && entry.size_bytes == Some(size),
        "restored object bytes differ for {}",
        entry.key
    );
    Ok(())
}

async fn download_body(body: ByteStream, path: &Path) -> Result<(String, u64)> {
    let mut reader = body.into_async_read();
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("could not create {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        file.write_all(&buffer[..count]).await?;
        hasher.update(&buffer[..count]);
        size = size
            .checked_add(u64::try_from(count).unwrap_or(u64::MAX))
            .context("archived object size overflowed")?;
    }
    file.sync_all().await?;
    Ok((hex::encode(hasher.finalize()), size))
}

async fn verify_blob(path: &Path, digest: &str, size: u64) -> Result<()> {
    ensure!(
        path.is_file() && !path.is_symlink(),
        "archived blob is missing or unsafe: {}",
        path.display()
    );
    let (actual_digest, actual_size) = hash_file(path).await?;
    ensure!(
        actual_digest == digest && actual_size == size,
        "archived blob failed integrity check: {}",
        path.display()
    );
    Ok(())
}

async fn hash_file(path: &Path) -> Result<(String, u64)> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("could not open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size = size
            .checked_add(u64::try_from(count).unwrap_or(u64::MAX))
            .context("file size overflowed")?;
    }
    Ok((hex::encode(hasher.finalize()), size))
}

fn load_manifest(input: &Path) -> Result<ObjectBackupManifest> {
    let path = input.join("manifest.json");
    let bytes =
        std::fs::read(&path).with_context(|| format!("could not read {}", path.display()))?;
    serde_json::from_slice(&bytes).context("object backup manifest is not valid JSON")
}

fn load_restore_map(path: &Path) -> Result<ObjectRestoreMap> {
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    serde_json::from_slice(&bytes).context("object restore map is not valid JSON")
}

fn default_restore_complete() -> bool {
    true
}

fn pending_from_entry(entry: &ObjectBackupEntry) -> ObjectRestorePending {
    ObjectRestorePending {
        kind: entry.kind.clone(),
        key: entry.key.clone(),
        source_version_id: entry.source_version_id.clone(),
        sha256: entry.sha256.clone(),
        size_bytes: entry.size_bytes,
    }
}

fn same_pending_identity(left: &ObjectRestorePending, right: &ObjectRestorePending) -> bool {
    left.kind == right.kind
        && left.key == right.key
        && left.source_version_id == right.source_version_id
        && left.sha256 == right.sha256
        && left.size_bytes == right.size_bytes
}

fn refresh_restore_summary(mapping: &mut ObjectRestoreMap) -> Result<()> {
    let object_versions = mapping
        .entries
        .iter()
        .filter(|entry| entry.kind == "object_version")
        .count();
    let delete_markers = mapping.entries.len().saturating_sub(object_versions);
    let logical_bytes = mapping.entries.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(entry.size_bytes.unwrap_or(0))
            .context("restore journal byte count overflowed")
    })?;
    mapping.summary = ObjectRestoreSummary {
        object_versions,
        delete_markers,
        logical_bytes,
    };
    Ok(())
}

fn write_json_sync(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    let mut file = std::fs::File::create(path)
        .with_context(|| format!("could not create {}", path.display()))?;
    use std::io::Write;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).with_context(|| {
        format!(
            "could not create restore journal directory {}",
            parent.display()
        )
    })?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("restore journal path has no safe file name")?;
    let temporary_path = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::now_v7()));
    let result = (|| -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(value)?;
        bytes.push(b'\n');
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .with_context(|| {
                format!(
                    "could not create temporary restore journal {}",
                    temporary_path.display()
                )
            })?;
        use std::io::Write;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary_path, path).with_context(|| {
            format!(
                "could not atomically publish restore journal {}",
                path.display()
            )
        })?;
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| {
                format!(
                    "could not synchronize restore journal directory {}",
                    parent.display()
                )
            })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

fn digest_from_entry(entry: &ObjectBackupEntry) -> Result<String> {
    let digest = entry
        .sha256
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .context("object version omitted sha256")?;
    ensure!(
        valid_digest(digest),
        "object version contains invalid sha256"
    );
    Ok(digest.to_owned())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn entry_sort_key(left: &ObjectBackupEntry, right: &ObjectBackupEntry) -> std::cmp::Ordering {
    (
        &left.key,
        left.last_modified_unix_seconds,
        left.last_modified_subsecond_nanos,
        &left.kind,
        &left.source_version_id,
    )
        .cmp(&(
            &right.key,
            right.last_modified_unix_seconds,
            right.last_modified_subsecond_nanos,
            &right.kind,
            &right.source_version_id,
        ))
}

fn restore_order(entries: &[ObjectBackupEntry]) -> Vec<&ObjectBackupEntry> {
    let mut grouped = BTreeMap::<&str, Vec<&ObjectBackupEntry>>::new();
    for entry in entries {
        grouped.entry(&entry.key).or_default().push(entry);
    }
    let mut ordered = Vec::with_capacity(entries.len());
    for (_, mut events) in grouped {
        events.sort_by(|left, right| {
            (
                left.is_latest,
                left.last_modified_unix_seconds,
                left.last_modified_subsecond_nanos,
                &left.kind,
                &left.source_version_id,
            )
                .cmp(&(
                    right.is_latest,
                    right.last_modified_unix_seconds,
                    right.last_modified_subsecond_nanos,
                    &right.kind,
                    &right.source_version_id,
                ))
        });
        ordered.extend(events);
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        kind: &str,
        key: &str,
        version: &str,
        latest: bool,
        seconds: i64,
    ) -> ObjectBackupEntry {
        ObjectBackupEntry {
            kind: kind.to_owned(),
            key: key.to_owned(),
            source_version_id: version.to_owned(),
            is_latest: latest,
            last_modified_unix_seconds: seconds,
            last_modified_subsecond_nanos: 0,
            size_bytes: (kind == "object_version").then_some(3),
            sha256: (kind == "object_version").then_some(format!("sha256:{}", "a".repeat(64))),
            etag: None,
            content_type: None,
            cache_control: None,
            content_disposition: None,
            content_encoding: None,
            content_language: None,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn restore_order_places_each_latest_event_last() {
        let entries = vec![
            entry("object_version", "b", "b-latest", true, 1),
            entry("delete_marker", "a", "a-latest", true, 1),
            entry("object_version", "a", "a-old", false, 3),
            entry("object_version", "b", "b-old", false, 2),
        ];
        let order = restore_order(&entries)
            .into_iter()
            .map(|entry| entry.source_version_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(order, vec!["a-old", "a-latest", "b-old", "b-latest"]);
    }

    #[test]
    fn manifest_rejects_missing_or_duplicate_latest_events() {
        let base = ObjectBackupManifest {
            format: BACKUP_FORMAT.to_owned(),
            created_at: now(),
            source_bucket: "bucket".to_owned(),
            bucket_versioning: "Enabled".to_owned(),
            entries: vec![entry("object_version", "key", "one", false, 1)],
            summary: ObjectBackupSummary {
                object_versions: 1,
                delete_markers: 0,
                unique_blobs: 1,
                logical_bytes: 3,
                stored_bytes: 3,
            },
        };
        assert!(validate_manifest(&base).is_err());
        let mut duplicate = base;
        duplicate.entries[0].is_latest = true;
        duplicate
            .entries
            .push(entry("delete_marker", "key", "two", true, 2));
        duplicate.summary.delete_markers = 1;
        assert!(validate_manifest(&duplicate).is_err());
    }

    #[test]
    fn restore_map_requires_exact_counts_and_hashes() {
        let mapping = ObjectRestoreMap {
            format: RESTORE_MAP_FORMAT.to_owned(),
            restored_at: now(),
            source_bucket: "source".to_owned(),
            target_bucket: "target".to_owned(),
            source_manifest_sha256: format!("sha256:{}", "b".repeat(64)),
            target_was_empty: true,
            pending: None,
            complete: true,
            entries: vec![ObjectRestoreEntry {
                kind: "object_version".to_owned(),
                key: "key".to_owned(),
                source_version_id: "source-version".to_owned(),
                restored_version_id: "target-version".to_owned(),
                sha256: Some(format!("sha256:{}", "a".repeat(64))),
                size_bytes: Some(3),
            }],
            summary: ObjectRestoreSummary {
                object_versions: 1,
                delete_markers: 0,
                logical_bytes: 3,
            },
        };
        assert!(validate_restore_map(&mapping).is_ok());
        let mut broken = mapping;
        broken.summary.logical_bytes = 2;
        assert!(validate_restore_map(&broken).is_err());
    }

    #[test]
    fn partial_restore_journal_is_valid_and_atomically_replaceable() {
        let pending_source = entry("object_version", "key", "source-version", true, 1);
        let mut mapping = ObjectRestoreMap {
            format: RESTORE_MAP_FORMAT.to_owned(),
            restored_at: now(),
            source_bucket: "source".to_owned(),
            target_bucket: "target".to_owned(),
            source_manifest_sha256: format!("sha256:{}", "b".repeat(64)),
            target_was_empty: true,
            pending: Some(pending_from_entry(&pending_source)),
            complete: false,
            entries: Vec::new(),
            summary: ObjectRestoreSummary {
                object_versions: 0,
                delete_markers: 0,
                logical_bytes: 0,
            },
        };
        assert!(validate_restore_map(&mapping).is_ok());

        let directory = tempfile::tempdir().expect("temporary journal directory");
        let path = directory.path().join("restore-map.json");
        write_json_atomic(&path, &mapping).expect("write pending journal");
        let loaded = load_restore_map(&path).expect("read pending journal");
        assert!(!loaded.complete);
        assert!(loaded.pending.is_some());

        mapping.pending = None;
        mapping.complete = true;
        mapping.entries.push(ObjectRestoreEntry {
            kind: "object_version".to_owned(),
            key: "key".to_owned(),
            source_version_id: "source-version".to_owned(),
            restored_version_id: "target-version".to_owned(),
            sha256: pending_source.sha256,
            size_bytes: pending_source.size_bytes,
        });
        refresh_restore_summary(&mut mapping).expect("refresh completed summary");
        write_json_atomic(&path, &mapping).expect("replace completed journal");
        let loaded = load_restore_map(&path).expect("read completed journal");
        assert!(loaded.complete);
        assert_eq!(loaded.entries.len(), 1);
        assert!(
            directory
                .path()
                .read_dir()
                .expect("list directory")
                .all(|entry| !entry
                    .expect("directory entry")
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
        );
    }

    #[test]
    fn maximum_portable_object_uses_multipart_restore() {
        assert!(!should_use_multipart_restore(
            MULTIPART_RESTORE_THRESHOLD_BYTES - 1
        ));
        assert!(should_use_multipart_restore(
            MULTIPART_RESTORE_THRESHOLD_BYTES
        ));
        assert!(should_use_multipart_restore(MAX_PORTABLE_OBJECT_BYTES));
        assert!(multipart_restore_part_count(MAX_PORTABLE_OBJECT_BYTES) <= 10_000);
    }

    #[tokio::test]
    async fn live_minio_restore_resumes_and_uses_multipart() {
        if std::env::var("STRAYLIGHT_LIVE_S3_RECOVERY_TEST").as_deref() != Ok("1") {
            eprintln!("STRAYLIGHT_LIVE_S3_RECOVERY_TEST is unset; skipping live recovery test");
            return;
        }

        let config = crate::Config::from_env().expect("load live recovery configuration");
        let store = ObjectStore::new(&config)
            .await
            .expect("create live recovery object store");
        store.ensure_bucket().await.expect("create recovery bucket");
        store
            .client
            .put_bucket_versioning()
            .bucket(&store.bucket)
            .versioning_configuration(
                aws_sdk_s3::types::VersioningConfiguration::builder()
                    .status(aws_sdk_s3::types::BucketVersioningStatus::Enabled)
                    .build(),
            )
            .send()
            .await
            .expect("enable recovery bucket versioning");
        abort_all_multipart_uploads(&store)
            .await
            .expect("clear interrupted multipart fixtures");
        let stale = list_events(&store)
            .await
            .expect("list stale recovery fixtures");
        cleanup_listed_entries(&store, &stale)
            .await
            .expect("clear stale recovery fixtures");

        let directory = tempfile::tempdir().expect("create recovery archive directory");
        let archive = directory.path().join("archive");
        let mapping_path = directory.path().join("restore-map.json");
        let large_path = directory.path().join("large.bin");
        let large_size = MULTIPART_RESTORE_THRESHOLD_BYTES + 1;
        tokio::fs::File::create(&large_path)
            .await
            .expect("create large restore fixture")
            .set_len(large_size)
            .await
            .expect("size large restore fixture");

        let result: Result<()> = Box::pin(async {
            store
                .client
                .put_object()
                .bucket(&store.bucket)
                .key("a-small")
                .content_type("text/plain")
                .metadata("revision", "one")
                .body(ByteStream::from_static(b"first"))
                .send()
                .await
                .expect("write first small version");
            store
                .client
                .put_object()
                .bucket(&store.bucket)
                .key("a-small")
                .content_type("text/plain")
                .metadata("revision", "two")
                .body(ByteStream::from_static(b"second"))
                .send()
                .await
                .expect("write second small version");
            store
                .client
                .delete_object()
                .bucket(&store.bucket)
                .key("a-small")
                .send()
                .await
                .expect("write latest delete marker");
            store
                .client
                .put_object()
                .bucket(&store.bucket)
                .key("z-large")
                .content_type("application/octet-stream")
                .metadata("fixture", "multipart")
                .content_length(i64::try_from(large_size).expect("large size fits i64"))
                .body(
                    ByteStream::from_path(&large_path)
                        .await
                        .expect("stream large source fixture"),
                )
                .send()
                .await
                .expect("write large source object");

            let manifest = export(&store, &archive).await?;
            ensure!(manifest.summary.object_versions == 3);
            ensure!(manifest.summary.delete_markers == 1);
            cleanup_listed_entries(&store, &list_events(&store).await?).await?;
            ensure!(list_events(&store).await?.is_empty());

            let manifest_hash = hash_file(&archive.join("manifest.json")).await?.0;
            let first = restore_order(&manifest.entries)
                .into_iter()
                .next()
                .context("source manifest has no first restore event")?;
            let interrupted = ObjectRestoreMap {
                format: RESTORE_MAP_FORMAT.to_owned(),
                restored_at: now(),
                source_bucket: manifest.source_bucket.clone(),
                target_bucket: store.bucket.clone(),
                source_manifest_sha256: format!("sha256:{manifest_hash}"),
                target_was_empty: true,
                pending: Some(pending_from_entry(first)),
                complete: false,
                entries: Vec::new(),
                summary: ObjectRestoreSummary {
                    object_versions: 0,
                    delete_markers: 0,
                    logical_bytes: 0,
                },
            };
            write_json_atomic(&mapping_path, &interrupted)?;
            let unjournaled = restore_one(&store, &archive, first).await?;
            ensure!(unjournaled.key == first.key);

            let restored = restore(&store, &archive, &mapping_path).await?;
            ensure!(restored.complete);
            ensure!(restored.pending.is_none());
            ensure!(restored.entries.len() == manifest.entries.len());
            ensure!(
                restored
                    .entries
                    .iter()
                    .any(|entry| entry.key == "z-large" && entry.size_bytes == Some(large_size))
            );

            validate_restore_map_against_manifest(&restored, &manifest, true)?;
            cleanup_restore(&store, &mapping_path).await?;
            ensure!(list_events(&store).await?.is_empty());
            Ok(())
        })
        .await;

        let _ = abort_all_multipart_uploads(&store).await;
        if let Ok(entries) = list_events(&store).await {
            let _ = cleanup_listed_entries(&store, &entries).await;
        }
        let _ = store
            .client
            .delete_bucket()
            .bucket(&store.bucket)
            .send()
            .await;
        result.expect("live MinIO recovery round trip");
    }
}
