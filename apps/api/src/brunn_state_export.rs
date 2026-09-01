use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use filetime::FileTime;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::{
    brunn_state_cli::{ApiClient, strip_sha256, unwrap_data},
    messaging_protocol,
};

const EXPORT_MANIFEST_FORMAT: &str = "brunn-workspace-export@v1";
const IMPORT_MANIFEST_FORMAT: &str = "brunn-workspace-import-manifest@v1";
const MANIFEST_PAGE_SIZE: usize = 1_000;
const MAX_EXACT_TEXT_READ_CHARS: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Args)]
pub struct ExportArgs {
    #[arg(long)]
    pub output: PathBuf,
    /// Export every historical version through exact versioned workspace reads.
    #[arg(long)]
    pub history: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EntryKind {
    Markdown,
    Binary,
}

#[derive(Clone, Debug, Deserialize)]
struct ManifestEntry {
    entry_ref: String,
    path: String,
    kind: EntryKind,
    media_type: String,
    version: i64,
    content_hash: String,
    size_bytes: u64,
    #[serde(default)]
    metadata: Value,
    #[serde(default)]
    current: bool,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct ManifestCursor {
    #[serde(alias = "path")]
    after_path: String,
    #[serde(alias = "entry_ref")]
    after_entry_ref: String,
    #[serde(default, alias = "version")]
    after_version: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ManifestPage {
    Offset(usize),
    Cursor(ManifestCursor),
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct PortableMetadata {
    #[serde(alias = "mtime_ns")]
    modified_unix_ns: Option<u64>,
    mode: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
struct PortableEntry {
    path: String,
    archive_path: String,
    kind: EntryKind,
    entry_ref: String,
    version: i64,
    content_hash: String,
    size_bytes: u64,
    media_type: String,
    modified_unix_ns: Option<u64>,
    mode: Option<u32>,
    metadata: Value,
    current: bool,
    deleted: bool,
    created_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct PortableManifest {
    format: String,
    version: u8,
    workspace_generation: i64,
    workspace_generation_is_snapshot: bool,
    entries: Vec<PortableEntry>,
}

pub async fn run(client: &ApiClient, args: ExportArgs) -> Result<()> {
    if args.output.exists() {
        bail!("refusing to overwrite {}", args.output.display());
    }

    let (generation, entries) = fetch_manifest(client, args.history).await?;
    let parent = args
        .output
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let output_name = args
        .output
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("workspace");
    let staging_path = parent.join(format!(
        ".{output_name}.brunn-export-{}.tmp",
        Uuid::now_v7()
    ));
    std::fs::create_dir(&staging_path)?;
    set_private_directory(&staging_path)?;
    std::fs::create_dir(staging_path.join("workspace"))?;
    set_private_directory(&staging_path.join("workspace"))?;
    let mut staging = StagingDirectory {
        path: staging_path,
        published: false,
    };

    let outputs = output_plan(entries, args.history)?;
    let mut portable_entries = Vec::with_capacity(outputs.len());
    for (relative_output, entry) in outputs {
        let destination = safe_join(&staging.path, &relative_output)?;
        match entry.kind {
            EntryKind::Markdown => {
                download_markdown(client, &entry, &destination).await?;
            }
            EntryKind::Binary => {
                let version = entry.version.to_string();
                client
                    .download_verified(
                        &format!("/v1/workspace/binaries/{}/content", entry.entry_ref),
                        &[("version", version.as_str())],
                        &destination,
                        &entry.content_hash,
                        Some(entry.size_bytes),
                    )
                    .await
                    .with_context(|| format!("could not export {}", entry.path))?;
            }
        }
        let portable = portable_metadata(&entry.metadata)?;
        restore_file_metadata(&destination, &portable)?;
        portable_entries.push(PortableEntry {
            path: entry.path,
            archive_path: relative_output,
            kind: entry.kind,
            entry_ref: entry.entry_ref,
            version: entry.version,
            content_hash: format!("sha256:{}", strip_sha256(&entry.content_hash)),
            size_bytes: entry.size_bytes,
            media_type: entry.media_type,
            modified_unix_ns: portable.modified_unix_ns,
            mode: portable.mode,
            metadata: entry.metadata,
            current: entry.current,
            deleted: entry.deleted,
            created_at: entry.created_at,
        });
    }
    portable_entries.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));
    let manifest = PortableManifest {
        format: EXPORT_MANIFEST_FORMAT.to_owned(),
        version: 1,
        workspace_generation: generation,
        workspace_generation_is_snapshot: false,
        entries: portable_entries,
    };
    write_json_new(&staging.path.join("manifest.json"), &manifest)?;
    write_checksums(&staging.path)?;

    if args.output.exists() {
        bail!("refusing to overwrite {}", args.output.display());
    }
    std::fs::rename(&staging.path, &args.output)?;
    staging.published = true;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "complete",
            "output": args.output,
            "workspace_generation": generation,
            "files": manifest.entries.len(),
            "bytes": manifest.entries.iter().map(|entry| entry.size_bytes).sum::<u64>(),
            "manifest_sha256": format!(
                "sha256:{}",
                hash_file(&args.output.join("manifest.json"))?
            )
        }))?
    );
    Ok(())
}

async fn fetch_manifest(client: &ApiClient, history: bool) -> Result<(i64, Vec<ManifestEntry>)> {
    let mut page_request = ManifestPage::Offset(0);
    let mut observed_generation = 0_i64;
    let mut output = Vec::new();
    let mut prior_sort = None::<(String, String, i64)>;
    let mut collisions = BTreeMap::<String, String>::new();
    loop {
        let owned_query = manifest_page_query(&page_request, history);
        let query = owned_query
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect::<Vec<_>>();
        let response = client.get_json("/v1/workspace/manifest", &query).await?;
        let data = unwrap_data(&response);
        let page_generation = data
            .get("workspace_generation")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("workspace manifest lacks generation"))?;
        observed_generation = observed_generation.max(page_generation);
        let page = data
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("workspace manifest lacks entries"))?;
        for value in page {
            let entry: ManifestEntry = serde_json::from_value(value.clone())
                .context("workspace manifest contains an invalid entry")?;
            validate_workspace_path(&entry.path)?;
            validate_hash(&entry.content_hash)?;
            if entry.version <= 0 {
                bail!("workspace manifest contains an invalid version");
            }
            let sort = (entry.path.clone(), entry.entry_ref.clone(), entry.version);
            if prior_sort
                .as_ref()
                .is_some_and(|previous| previous >= &sort)
            {
                bail!("workspace manifest is not strictly path/version ordered");
            }
            prior_sort = Some(sort);
            let key = collision_key(&entry.path);
            if let Some(previous) = collisions.get(&key)
                && previous != &entry.entry_ref
            {
                bail!(
                    "workspace paths {previous:?} and {:?} collide on a portable filesystem",
                    entry.path
                );
            }
            collisions.insert(key, entry.entry_ref.clone());
            output.push(entry);
        }
        if let Some(cursor) = manifest_next_cursor(data, history)? {
            if page.is_empty() {
                bail!("workspace manifest returned a cursor for an empty page");
            }
            page_request = ManifestPage::Cursor(cursor);
            continue;
        }
        match page_request {
            ManifestPage::Cursor(_) => break,
            ManifestPage::Offset(offset) if page.len() == MANIFEST_PAGE_SIZE => {
                page_request = ManifestPage::Offset(
                    offset
                        .checked_add(page.len())
                        .ok_or_else(|| anyhow!("workspace manifest offset overflow"))?,
                );
            }
            ManifestPage::Offset(_) => break,
        }
    }
    Ok((observed_generation, output))
}

fn manifest_page_query(page: &ManifestPage, history: bool) -> Vec<(&'static str, String)> {
    let mut query = vec![
        ("limit", MANIFEST_PAGE_SIZE.to_string()),
        ("history", history.to_string()),
    ];
    match page {
        ManifestPage::Offset(offset) => query.push(("offset", offset.to_string())),
        ManifestPage::Cursor(cursor) => {
            query.push(("after_path", cursor.after_path.clone()));
            query.push(("after_entry_ref", cursor.after_entry_ref.clone()));
            if history {
                query.push((
                    "after_version",
                    cursor.after_version.unwrap_or_default().to_string(),
                ));
            }
        }
    }
    query
}

fn manifest_next_cursor(data: &Value, history: bool) -> Result<Option<ManifestCursor>> {
    let Some(value) = data.get("next") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let cursor: ManifestCursor = serde_json::from_value(value.clone())
        .context("workspace manifest has an invalid cursor")?;
    validate_workspace_path(&cursor.after_path)?;
    if !cursor.after_entry_ref.starts_with("entry:") {
        bail!("workspace manifest cursor has an invalid entry reference");
    }
    if history && cursor.after_version.is_none_or(|version| version <= 0) {
        bail!("workspace history cursor has an invalid version");
    }
    Ok(Some(cursor))
}

fn output_plan(entries: Vec<ManifestEntry>, history: bool) -> Result<Vec<(String, ManifestEntry)>> {
    let mut output = Vec::new();
    for entry in entries {
        if history {
            let version_path = format!(
                "history/{}/v{:020}-{}",
                &hash_bytes(entry.entry_ref.as_bytes())[..32],
                entry.version,
                &strip_sha256(&entry.content_hash)[..16]
            );
            output.push((version_path, entry.clone()));
            if entry.current && !entry.deleted {
                output.push((format!("workspace/{}", entry.path), entry));
            }
        } else {
            output.push((format!("workspace/{}", entry.path), entry));
        }
    }
    output.sort_by(|left, right| left.0.cmp(&right.0));
    let mut seen = BTreeMap::<String, String>::new();
    for (path, entry) in &output {
        validate_relative_path(path)?;
        let key = collision_key(path);
        if let Some(previous) = seen.insert(key, path.clone()) {
            bail!(
                "export output paths {previous:?} and {path:?} collide for {}",
                entry.path
            );
        }
    }
    Ok(output)
}

async fn download_markdown(
    client: &ApiClient,
    entry: &ManifestEntry,
    destination: &Path,
) -> Result<()> {
    let max_chars = exact_text_read_limit(entry)?;
    let response = client
        .post_json(
            "/v1/workspace/read",
            &json!({
                "requests": [{
                    "ref": entry.entry_ref,
                    "view": "full",
                    "max_chars": max_chars,
                    "version": entry.version
                }]
            }),
        )
        .await
        .with_context(|| format!("could not read {}", entry.path))?;
    let data = unwrap_data(&response);
    let item = data
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| (items.len() == 1).then(|| &items[0]))
        .ok_or_else(|| {
            anyhow!(
                "workspace.read returned an invalid result for {}",
                entry.path
            )
        })?;
    let path = item
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("workspace.read omitted path"))?;
    let version = item
        .get("version")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("workspace.read omitted version"))?;
    let content_hash = item
        .get("content_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("workspace.read omitted content_hash"))?;
    let text = item
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("workspace.read omitted exact text"))?;
    if path != entry.path
        || version != entry.version
        || strip_sha256(content_hash) != strip_sha256(&entry.content_hash)
    {
        bail!(
            "workspace.read did not return the exact manifest version and hash for {}",
            entry.path
        );
    }
    if hash_bytes(text.as_bytes()) != strip_sha256(&entry.content_hash)
        && text.chars().count() == max_chars
    {
        bail!(
            "{} exceeds workspace.read's current {}-character exact-content limit",
            entry.path,
            max_chars
        );
    }
    if is_managed_conversation_manifest_entry(entry) {
        messaging_protocol::validate_conversation_entry(&entry.path, &entry.metadata, text)
            .map_err(|error| anyhow!(error.to_string()))?
            .ok_or_else(|| anyhow!("managed conversation export lacks canonical metadata"))?;
    }
    write_verified_bytes(
        destination,
        text.as_bytes(),
        &entry.content_hash,
        entry.size_bytes,
    )
}

fn exact_text_read_limit(entry: &ManifestEntry) -> Result<usize> {
    if !is_managed_conversation_manifest_entry(entry) {
        return Ok(MAX_EXACT_TEXT_READ_CHARS);
    }
    if entry.size_bytes > messaging_protocol::MAX_CANONICAL_CONVERSATION_BYTES as u64 {
        bail!(
            "{} exceeds the managed conversation {}-byte limit",
            entry.path,
            messaging_protocol::MAX_CANONICAL_CONVERSATION_BYTES
        );
    }
    Ok(messaging_protocol::MAX_CANONICAL_CONVERSATION_BYTES)
}

fn is_managed_conversation_manifest_entry(entry: &ManifestEntry) -> bool {
    if entry.kind != EntryKind::Markdown || entry.media_type != "text/markdown" {
        return false;
    }
    let Some(path_id) = messaging_protocol::conversation_id_from_path(&entry.path) else {
        return false;
    };
    let metadata = entry
        .metadata
        .get("client")
        .filter(|value| value.is_object())
        .unwrap_or(&entry.metadata);
    metadata.get("kind").and_then(Value::as_str) == Some("conversation")
        && metadata.get("schema").and_then(Value::as_str) == Some("conversation.v1")
        && metadata
            .get("conversation")
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            == Some(path_id)
}

fn write_verified_bytes(
    destination: &Path,
    bytes: &[u8],
    expected_hash: &str,
    expected_size: u64,
) -> Result<()> {
    let actual_size = u64::try_from(bytes.len())?;
    let actual_hash = hash_bytes(bytes);
    if actual_size != expected_size || actual_hash != strip_sha256(expected_hash) {
        bail!(
            "download integrity check failed: expected sha256:{} / {} bytes, received sha256:{} / {} bytes",
            strip_sha256(expected_hash),
            expected_size,
            actual_hash,
            actual_size
        );
    }
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("export destination has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(destination)?;
    file.write_all(bytes)?;
    file.flush()?;
    Ok(())
}

fn portable_metadata(metadata: &Value) -> Result<PortableMetadata> {
    if let Some(value) = metadata.get("portable").filter(|value| !value.is_null()) {
        return validate_portable_metadata(serde_json::from_value(value.clone())?);
    }
    let Some(provenance) = metadata.get("provenance").and_then(Value::as_str) else {
        return Ok(PortableMetadata::default());
    };
    let Ok(value) = serde_json::from_str::<Value>(provenance) else {
        return Ok(PortableMetadata::default());
    };
    if value.get("format").and_then(Value::as_str) != Some(IMPORT_MANIFEST_FORMAT) {
        return Ok(PortableMetadata::default());
    }
    let portable = value.get("portable").cloned().unwrap_or_else(|| json!({}));
    validate_portable_metadata(serde_json::from_value(portable)?)
}

fn validate_portable_metadata(mut metadata: PortableMetadata) -> Result<PortableMetadata> {
    if metadata.mode.is_some_and(|value| value > 0o777) {
        bail!("workspace entry contains non-portable mode bits");
    }
    metadata.mode = metadata.mode.map(|value| value & 0o777);
    Ok(metadata)
}

fn restore_file_metadata(path: &Path, metadata: &PortableMetadata) -> Result<()> {
    if let Some(modified_unix_ns) = metadata.modified_unix_ns {
        let seconds = i64::try_from(modified_unix_ns / 1_000_000_000)
            .context("file modification time exceeds platform range")?;
        let nanoseconds = u32::try_from(modified_unix_ns % 1_000_000_000)?;
        filetime::set_file_mtime(path, FileTime::from_unix_time(seconds, nanoseconds))?;
    }
    #[cfg(unix)]
    if let Some(mode) = metadata.mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn write_json_new(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

fn write_checksums(root: &Path) -> Result<()> {
    let mut paths = BTreeSet::new();
    collect_files(root, root, &mut paths)?;
    let checksum_path = root.join("CHECKSUMS.sha256");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut output = options.open(&checksum_path)?;
    for relative in paths {
        let digest = hash_file(&safe_join(root, &relative)?)?;
        writeln!(output, "{digest}  {relative}")?;
    }
    output.flush()?;
    Ok(())
}

fn collect_files(root: &Path, directory: &Path, output: &mut BTreeSet<String>) -> Result<()> {
    let mut entries = std::fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            bail!("export staging unexpectedly contains a symbolic link");
        }
        if file_type.is_dir() {
            collect_files(root, &entry.path(), output)?;
        } else if file_type.is_file() {
            output.insert(platform_to_relative(entry.path().strip_prefix(root)?)?);
        } else {
            bail!("export staging unexpectedly contains a special file");
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn hash_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn validate_hash(value: &str) -> Result<()> {
    let value = strip_sha256(value);
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("workspace manifest contains an invalid SHA-256");
    }
    Ok(())
}

fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative_path(relative)?;
    let destination = root.join(relative.split('/').collect::<PathBuf>());
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("export destination has no parent"))?;
    std::fs::create_dir_all(parent)?;
    Ok(destination)
}

fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 4_096
        || path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        bail!("unsafe workspace path: {path:?}");
    }
    Ok(())
}

fn validate_workspace_path(path: &str) -> Result<()> {
    validate_relative_path(path)?;
    if path.len() > 1_024 {
        bail!("workspace path exceeds the server limit");
    }
    Ok(())
}

fn platform_to_relative(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .ok_or_else(|| anyhow!("export generated a non-UTF-8 path"))?,
            ),
            _ => bail!("export generated an unsafe path"),
        }
    }
    let result = parts.join("/");
    validate_relative_path(&result)?;
    Ok(result)
}

fn collision_key(path: &str) -> String {
    path.nfc().collect::<String>().to_lowercase()
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory(_: &Path) -> Result<()> {
    Ok(())
}

struct StagingDirectory {
    path: PathBuf,
    published: bool,
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn manifest_entry(version: i64, current: bool) -> ManifestEntry {
        ManifestEntry {
            entry_ref: "entry:550e8400-e29b-41d4-a716-446655440000".to_owned(),
            path: "Trips/plan.md".to_owned(),
            kind: EntryKind::Markdown,
            media_type: "text/markdown".to_owned(),
            version,
            content_hash: format!("sha256:{:064x}", version),
            size_bytes: 1,
            metadata: json!({}),
            current,
            deleted: false,
            created_at: Some(format!("2026-07-2{version}T00:00:00Z")),
        }
    }

    #[test]
    fn portable_metadata_supports_markdown_and_binary_transport_shapes() {
        let direct = portable_metadata(&json!({
            "portable": {"modified_unix_ns": 123, "mode": 0o640}
        }))
        .unwrap();
        assert_eq!(direct.modified_unix_ns, Some(123));
        assert_eq!(direct.mode, Some(0o640));

        let wrapped = portable_metadata(&json!({
            "provenance": serde_json::to_string(&json!({
                "format": IMPORT_MANIFEST_FORMAT,
                "portable": {"modified_unix_ns": 456, "mode": 0o600},
                "original_provenance": null
            })).unwrap()
        }))
        .unwrap();
        assert_eq!(wrapped.modified_unix_ns, Some(456));
        assert_eq!(wrapped.mode, Some(0o600));
    }

    #[test]
    fn managed_conversation_exact_read_limit_is_narrow() {
        let conversation_id = Uuid::parse_str("018f0000-0000-7000-8000-000000000010").unwrap();
        let mut managed = manifest_entry(1, true);
        managed.path = messaging_protocol::conversation_path(conversation_id);
        managed.size_bytes = (MAX_EXACT_TEXT_READ_CHARS + 1) as u64;
        managed.metadata = json!({
            "kind": "conversation",
            "schema": "conversation.v1",
            "conversation": {"id": conversation_id}
        });
        assert!(is_managed_conversation_manifest_entry(&managed));
        assert_eq!(
            exact_text_read_limit(&managed).unwrap(),
            messaging_protocol::MAX_CANONICAL_CONVERSATION_BYTES
        );

        let mut ordinary_path = managed.clone();
        ordinary_path.path = "Trips/conversation.md".to_owned();
        assert!(!is_managed_conversation_manifest_entry(&ordinary_path));
        assert_eq!(
            exact_text_read_limit(&ordinary_path).unwrap(),
            MAX_EXACT_TEXT_READ_CHARS
        );

        let mut unmarked = managed.clone();
        unmarked.metadata = json!({});
        assert!(!is_managed_conversation_manifest_entry(&unmarked));
        assert_eq!(
            exact_text_read_limit(&unmarked).unwrap(),
            MAX_EXACT_TEXT_READ_CHARS
        );
    }

    #[test]
    fn checksum_ledger_is_sorted_and_covers_manifest_and_workspace_files() {
        let temporary = tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join("workspace")).unwrap();
        std::fs::write(root.join("workspace/b.md"), b"b").unwrap();
        std::fs::write(root.join("workspace/a.md"), b"a").unwrap();
        std::fs::write(root.join("manifest.json"), b"{}").unwrap();
        write_checksums(root).unwrap();
        let ledger = std::fs::read_to_string(root.join("CHECKSUMS.sha256")).unwrap();
        let paths = ledger
            .lines()
            .map(|line| line.split_once("  ").unwrap().1)
            .collect::<Vec<_>>();
        assert_eq!(paths, ["manifest.json", "workspace/a.md", "workspace/b.md"]);
    }

    #[test]
    fn verified_text_write_preserves_crlf_and_refuses_hash_mismatch() {
        let temporary = tempdir().unwrap();
        let destination = temporary.path().join("note.md");
        let bytes = b"# Note\r\n";
        write_verified_bytes(
            &destination,
            bytes,
            &format!("sha256:{}", hash_bytes(bytes)),
            bytes.len() as u64,
        )
        .unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), bytes);
        assert!(
            write_verified_bytes(
                &temporary.path().join("bad.md"),
                bytes,
                &format!("sha256:{}", "0".repeat(64)),
                bytes.len() as u64,
            )
            .is_err()
        );
    }

    #[test]
    fn unsafe_and_portably_colliding_paths_fail() {
        assert!(validate_relative_path("../escape").is_err());
        assert_eq!(collision_key("Cafe\u{301}.md"), collision_key("Café.md"));
    }

    #[test]
    fn history_paths_are_deterministic_and_keep_current_workspace_paths() {
        let first = output_plan(
            vec![manifest_entry(1, false), manifest_entry(2, true)],
            true,
        )
        .unwrap();
        let second = output_plan(
            vec![manifest_entry(1, false), manifest_entry(2, true)],
            true,
        )
        .unwrap();
        assert_eq!(
            first.iter().map(|item| &item.0).collect::<Vec<_>>(),
            second.iter().map(|item| &item.0).collect::<Vec<_>>()
        );
        assert_eq!(first.len(), 3);
        assert!(
            first
                .iter()
                .any(|(path, _)| path == "workspace/Trips/plan.md")
        );
        assert_eq!(
            first
                .iter()
                .filter(|(path, _)| path.starts_with("history/"))
                .count(),
            2
        );
    }

    #[test]
    fn manifest_pagination_uses_keyset_fields_when_server_returns_a_cursor() {
        let cursor = manifest_next_cursor(
            &json!({
                "next": {
                    "after_path": "Trips/plan.md",
                    "after_entry_ref": "entry:550e8400-e29b-41d4-a716-446655440000",
                    "after_version": 7
                }
            }),
            true,
        )
        .unwrap()
        .unwrap();
        let query = manifest_page_query(&ManifestPage::Cursor(cursor), true)
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        assert_eq!(query.get("after_path"), Some(&"Trips/plan.md".to_owned()));
        assert_eq!(
            query.get("after_entry_ref"),
            Some(&"entry:550e8400-e29b-41d4-a716-446655440000".to_owned())
        );
        assert_eq!(query.get("after_version"), Some(&"7".to_owned()));
        assert!(!query.contains_key("offset"));
    }

    #[test]
    fn manifest_pagination_keeps_offset_fallback_for_older_servers() {
        let query = manifest_page_query(&ManifestPage::Offset(2_000), false)
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        assert_eq!(query.get("offset"), Some(&"2000".to_owned()));
        assert!(!query.contains_key("after_path"));
        assert!(manifest_next_cursor(&json!({}), false).unwrap().is_none());
    }
}
