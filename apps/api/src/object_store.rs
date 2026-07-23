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

use crate::{
    config::Config,
    error::{ApiError, ApiResult},
    models::UserId,
};

const MAX_STAGE_BYTES: usize = 512 * 1024 * 1024;
const MAX_ARCHIVE_FILES: usize = 10_000;
const MAX_ARCHIVE_EXPANDED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
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
        if self
            .client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .is_err()
        {
            self.client
                .create_bucket()
                .bucket(&self.bucket)
                .send()
                .await
                .map_err(|error| {
                    ApiError::Internal(format!("could not create object bucket: {error}"))
                })?;
        }
        Ok(())
    }

    pub async fn put_user_blob(
        &self,
        user_id: UserId,
        content_type: Option<&str>,
        bytes: Bytes,
    ) -> ApiResult<StoredBlob> {
        if bytes.len() > MAX_STAGE_BYTES {
            return Err(ApiError::public(
                http::StatusCode::PAYLOAD_TOO_LARGE,
                "stage_limit_exceeded",
                "staged objects are limited to 512 MiB",
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
        request
            .metadata("sha256", &digest)
            .send()
            .await
            .map_err(|error| ApiError::Internal(format!("object upload failed: {error}")))?;
        Ok(StoredBlob {
            sha256: format!("sha256:{digest}"),
            size_bytes: bytes.len(),
            object_key: key,
        })
    }

    pub async fn get(&self, key: &str) -> ApiResult<Bytes> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| ApiError::not_found("asset_not_found", &format!("{key}: {error}")))?;
        output
            .body
            .collect()
            .await
            .map(|body| body.into_bytes())
            .map_err(|error| ApiError::Internal(format!("object download failed: {error}")))
    }

    pub async fn delete(&self, key: &str) -> ApiResult<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| ApiError::Internal(format!("object deletion failed: {error}")))?;
        Ok(())
    }
}

pub fn inspect_archive(name: &str, bytes: &[u8]) -> ApiResult<ArchiveInventory> {
    Ok(extract_archive(name, bytes)?.inventory)
}

pub fn extract_archive(name: &str, bytes: &[u8]) -> ApiResult<ArchiveInspection> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".zip") {
        extract_zip(bytes)
    } else if lower.ends_with(".tar") || lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        extract_tar(bytes, lower.ends_with(".gz") || lower.ends_with(".tgz"))
    } else {
        Ok(ArchiveInspection::default())
    }
}

fn extract_zip(bytes: &[u8]) -> ApiResult<ArchiveInspection> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| ApiError::invalid(format!("invalid zip archive: {error}")))?;
    if archive.len() > MAX_ARCHIVE_FILES {
        return Err(ApiError::invalid(
            "archive contains more than 10,000 entries",
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
                "archive expands beyond the 2 GiB safety limit",
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
                "archive contains more than 10,000 entries",
            ));
        }
        let mut entry =
            entry.map_err(|error| ApiError::invalid(format!("invalid tar entry: {error}")))?;
        let size = entry.size();
        expanded = expanded.saturating_add(size);
        if expanded > MAX_ARCHIVE_EXPANDED_BYTES {
            return Err(ApiError::invalid(
                "archive expands beyond the 2 GiB safety limit",
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
