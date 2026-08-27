use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{ArgAction, Args};
use fs2::FileExt;
use futures::{StreamExt, stream};
use globset::{Glob, GlobSet, GlobSetBuilder};
use reqwest::{Body, multipart};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio_util::io::ReaderStream;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    carrystate_cli::{ApiClient, strip_sha256, unwrap_data},
    messaging_protocol,
};

const IMPORT_MANIFEST_FORMAT: &str = "straylight-workspace-import-manifest@v1";
const EXPORT_MANIFEST_FORMAT: &str = "straylight-workspace-export@v1";
const TIER_A_PORTABLE_COMPANION_FORMAT: &str = "straylight-tier-a-portable-companion@v1";
const TIER_A_HISTORY_STAGE_FORMAT: &str = "straylight-tier-a-history-stage@v1";
const TIER_A_ORDINARY_HISTORY_SEMANTICS: &str = "ordinary_content_transition";
const TIER_A_EXACT_HISTORY_SEMANTICS: &str = "preserve_intentional_exact_bytes_version";
const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MANAGED_CONVERSATION_BYTES: u64 =
    messaging_protocol::MAX_CANONICAL_CONVERSATION_BYTES as u64;
const MAX_MULTIPART_BINARY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STREAMED_BINARY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_CONTEXT_SOURCE_BYTES: u64 = 256 * 1024;
const MAX_CONTEXTS_PER_BINARY: usize = 4;
const MAX_CONTEXT_CHARS: usize = 500;
const MAX_BINARY_PROVENANCE_BYTES: usize = 24_000;
const MANIFEST_PAGE_SIZE: usize = 1_000;
const MAX_CONCURRENT_MARKDOWN_WRITES: usize = 8;
const MAX_CONCURRENT_BINARY_WRITES: usize = 2;

#[derive(Debug, Clone, Args)]
pub struct ImportArgs {
    /// Directory whose relative paths become workspace paths. For a CLI export,
    /// pass either the export directory or its `workspace` child.
    #[arg(long)]
    pub root: PathBuf,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    pub describe_binaries: bool,
    #[arg(long)]
    pub include: Vec<String>,
    #[arg(long)]
    pub exclude: Vec<String>,
    #[arg(long)]
    pub manifest_out: Option<PathBuf>,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EntryKind {
    Markdown,
    Binary,
}

impl EntryKind {
    fn server_name(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Binary => "binary",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct PortableMetadata {
    #[serde(alias = "mtime_ns")]
    modified_unix_ns: Option<u64>,
    mode: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TierAHistorySemantics {
    OrdinaryContentTransition,
    PreserveIntentionalExactBytesVersion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TierAHistoryStage {
    target_lineage_ordinal: i64,
    semantics: TierAHistorySemantics,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct AttachmentContext {
    source_markdown_path: String,
    excerpt: String,
}

#[derive(Clone, Debug, Serialize)]
struct InventoryEntry {
    path: String,
    kind: EntryKind,
    content_hash: String,
    size_bytes: u64,
    media_type: String,
    portable: PortableMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attachment_context: Vec<AttachmentContext>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    source_metadata: Value,
    #[serde(skip)]
    observed: ObservedFile,
}

#[derive(Clone, Debug, Default)]
struct ObservedFile {
    modified_unix_ns: Option<u64>,
    mode: Option<u32>,
    device_id: Option<u64>,
    inode: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
struct IgnoredEntry {
    path: String,
    reason: String,
}

#[derive(Clone, Debug, Default, Serialize)]
struct InventoryTotals {
    files: usize,
    markdown_files: usize,
    binary_files: usize,
    bytes: u64,
    ignored: usize,
}

#[derive(Clone, Debug, Serialize)]
struct InventoryManifest {
    format: String,
    version: u8,
    root: String,
    entries: Vec<InventoryEntry>,
    ignored: Vec<IgnoredEntry>,
    totals: InventoryTotals,
    inventory_sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
struct RemoteEntry {
    entry_ref: String,
    path: String,
    kind: String,
    version: i64,
    content_hash: String,
    size_bytes: u64,
    #[serde(default)]
    metadata: Value,
}

#[derive(Clone, Debug, Deserialize)]
struct RemoteManifestCursor {
    after_path: String,
    after_entry_ref: String,
}

enum RemoteManifestPage {
    Offset(usize),
    Cursor(RemoteManifestCursor),
}

#[derive(Clone, Debug, Deserialize)]
struct PortableExportEntry {
    path: String,
    #[serde(default)]
    archive_path: Option<String>,
    kind: EntryKind,
    content_hash: String,
    size_bytes: u64,
    media_type: String,
    #[serde(default)]
    modified_unix_ns: Option<u64>,
    #[serde(default)]
    mode: Option<u32>,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Deserialize)]
struct PortableExportManifest {
    format: String,
    entries: Vec<PortableExportEntry>,
}

pub async fn run(client: Option<&ApiClient>, args: ImportArgs) -> Result<()> {
    let requested_root = reject_symlink_root(&args.root)?;
    let root = resolve_export_workspace_root(&requested_root)?;
    ensure_auxiliary_path_outside_root(&root, args.manifest_out.as_deref(), "manifest_out")?;
    ensure_auxiliary_path_outside_root(&root, args.state_dir.as_deref(), "state_dir")?;

    let manifest = inventory(&root, &args)?;
    if let Some(path) = &args.manifest_out {
        write_json_replace(path, &manifest)?;
    }
    if args.dry_run {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "dry_run",
                "authoritative_write_performed": false,
                "manifest": manifest
            }))?
        );
        return Ok(());
    }

    let client = client.ok_or_else(|| anyhow!("API client is required for import"))?;
    let state_base = args.state_dir.clone().unwrap_or_else(default_state_dir);
    std::fs::create_dir_all(&state_base)?;
    set_private_directory(&state_base)?;
    let state_key = hex::encode(Sha256::digest(
        format!("{}\0{}", client.endpoint(), root.display()).as_bytes(),
    ));
    let state_root = state_base.join(&state_key[..32]);
    std::fs::create_dir_all(&state_root)?;
    set_private_directory(&state_root)?;
    let _lock = StateLock::acquire(state_root.join("import.lock"))?;
    write_json_replace(&state_root.join("inventory.json"), &manifest)?;

    let mut remote = fetch_remote_manifest(client).await?;
    let mut uploaded = 0_usize;
    let mut skipped = 0_usize;
    for phase in [
        ImportPhase::ManagedConversation,
        ImportPhase::Markdown,
        ImportPhase::Binary,
        ImportPhase::BinaryCompanion,
    ] {
        let counts = import_phase(
            client,
            &root,
            &manifest.entries,
            &mut remote,
            args.describe_binaries,
            phase,
        )
        .await?;
        uploaded += counts.uploaded;
        skipped += counts.skipped;
    }

    let verified = fetch_remote_manifest(client).await?;
    verify_complete_import(&manifest.entries, &verified)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "complete",
            "root": root,
            "files": manifest.totals.files,
            "bytes": manifest.totals.bytes,
            "uploaded": uploaded,
            "skipped": skipped,
            "inventory_sha256": format!("sha256:{}", manifest.inventory_sha256),
            "state_path": state_root
        }))?
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImportPhase {
    ManagedConversation,
    Markdown,
    Binary,
    BinaryCompanion,
}

impl ImportPhase {
    fn includes(self, entry: &InventoryEntry) -> bool {
        match self {
            Self::ManagedConversation => is_managed_conversation_entry(entry),
            Self::Markdown => {
                entry.kind == EntryKind::Markdown
                    && !is_binary_companion_entry(entry)
                    && !is_managed_conversation_entry(entry)
            }
            Self::Binary => entry.kind == EntryKind::Binary,
            Self::BinaryCompanion => {
                entry.kind == EntryKind::Markdown && is_binary_companion_entry(entry)
            }
        }
    }

    fn concurrency(self) -> usize {
        match self {
            Self::ManagedConversation => 1,
            Self::Binary => MAX_CONCURRENT_BINARY_WRITES,
            Self::Markdown | Self::BinaryCompanion => MAX_CONCURRENT_MARKDOWN_WRITES,
        }
    }

    fn forces_upload(self) -> bool {
        self == Self::ManagedConversation
    }
}

struct ImportPlan<'a> {
    entry: &'a InventoryEntry,
    portable_companion: Option<&'a InventoryEntry>,
    companion_binary: Option<&'a InventoryEntry>,
    current: Option<RemoteEntry>,
    upload: bool,
    expected_version: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ImportDecision {
    upload: bool,
    expected_version: i64,
}

struct ImportOutcome {
    path_key: String,
    server_entry: RemoteEntry,
    generated_companion: Option<RemoteEntry>,
    uploaded: bool,
}

#[derive(Default)]
struct ImportCounts {
    uploaded: usize,
    skipped: usize,
}

fn effective_client_metadata(metadata: &Value) -> &Value {
    metadata
        .get("client")
        .filter(|value| value.is_object())
        .unwrap_or(metadata)
}

fn managed_conversation_id(path: &str, metadata: &Value) -> Option<Uuid> {
    let path_id = messaging_protocol::conversation_id_from_path(path)?;
    let metadata = effective_client_metadata(metadata);
    (metadata.get("kind").and_then(Value::as_str) == Some("conversation")
        && metadata.get("schema").and_then(Value::as_str) == Some("conversation.v1")
        && metadata
            .get("conversation")
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            == Some(path_id))
    .then_some(path_id)
}

fn is_managed_conversation_entry(entry: &InventoryEntry) -> bool {
    entry.kind == EntryKind::Markdown
        && entry.media_type == "text/markdown"
        && managed_conversation_id(&entry.path, &entry.source_metadata).is_some()
}

fn managed_conversation_parent(entry: &InventoryEntry) -> Result<Option<Uuid>> {
    if !is_managed_conversation_entry(entry) {
        return Ok(None);
    }
    let value = effective_client_metadata(&entry.source_metadata)
        .get("conversation")
        .and_then(|value| value.get("continues_from"));
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(
            Uuid::parse_str(value).context("conversation continues_from is not a UUID")?,
        )),
        Some(_) => bail!("conversation continues_from must be a UUID or null"),
    }
}

fn import_phase_entries(
    entries: &[InventoryEntry],
    phase: ImportPhase,
) -> Result<Vec<&InventoryEntry>> {
    let selected = entries
        .iter()
        .filter(|entry| phase.includes(entry))
        .collect::<Vec<_>>();
    if phase != ImportPhase::ManagedConversation {
        return Ok(selected);
    }

    let mut nodes = Vec::with_capacity(selected.len());
    let mut known = BTreeSet::new();
    for entry in selected {
        let conversation_id = managed_conversation_id(&entry.path, &entry.source_metadata)
            .ok_or_else(|| anyhow!("managed conversation import lacks canonical identity"))?;
        if !known.insert(conversation_id) {
            bail!("workspace import contains a duplicate managed conversation id");
        }
        nodes.push((conversation_id, managed_conversation_parent(entry)?, entry));
    }

    let mut emitted = BTreeSet::new();
    let mut ordered = Vec::with_capacity(nodes.len());
    while ordered.len() < nodes.len() {
        let before = ordered.len();
        for (conversation_id, parent_id, entry) in &nodes {
            if emitted.contains(conversation_id) {
                continue;
            }
            if parent_id.is_some_and(|parent| known.contains(&parent) && !emitted.contains(&parent))
            {
                continue;
            }
            emitted.insert(*conversation_id);
            ordered.push(*entry);
        }
        if ordered.len() == before {
            bail!("managed conversation continuation graph contains a cycle");
        }
    }
    Ok(ordered)
}

async fn import_phase(
    client: &ApiClient,
    root: &Path,
    entries: &[InventoryEntry],
    remote: &mut BTreeMap<String, RemoteEntry>,
    describe_binaries: bool,
    phase: ImportPhase,
) -> Result<ImportCounts> {
    let mut plans = Vec::new();
    for entry in import_phase_entries(entries, phase)? {
        let portable_companion = portable_companion_entry(entry, entries)?;
        let companion_binary = tier_a_binary_for_companion(entry, entries)?;
        let key = collision_key(&entry.path);
        let current = remote.get(&key).cloned();
        if let Some(current) = &current {
            ensure_remote_identity(entry, current)?;
        }
        let companion_ready = entry.kind != EntryKind::Binary
            || current
                .as_ref()
                .is_some_and(|value| binary_companion_is_present(value, remote));
        let decision =
            plan_import_entry(entry, current.as_ref(), companion_ready, companion_binary)?;
        plans.push(ImportPlan {
            entry,
            portable_companion,
            companion_binary,
            expected_version: decision.expected_version,
            current,
            upload: decision.upload || phase.forces_upload(),
        });
    }

    let mut pending = stream::iter(plans)
        .map(|plan| async move {
            let path_key = collision_key(&plan.entry.path);
            if !plan.upload {
                return Ok(ImportOutcome {
                    path_key,
                    server_entry: plan
                        .current
                        .expect("a skipped import plan has a matching remote entry"),
                    generated_companion: None,
                    uploaded: false,
                });
            }
            let (server_entry, generated_companion) = match plan.entry.kind {
                EntryKind::Markdown => (
                    write_markdown(
                        client,
                        root,
                        plan.entry,
                        plan.companion_binary,
                        plan.expected_version,
                    )
                    .await?,
                    None,
                ),
                EntryKind::Binary => {
                    let (binary, companion) = upload_binary(
                        client,
                        root,
                        plan.entry,
                        plan.portable_companion,
                        describe_binaries,
                        plan.expected_version,
                    )
                    .await?;
                    (binary, Some(companion))
                }
            };
            Ok::<_, anyhow::Error>(ImportOutcome {
                path_key,
                server_entry,
                generated_companion,
                uploaded: true,
            })
        })
        .buffer_unordered(phase.concurrency());

    let mut counts = ImportCounts::default();
    while let Some(outcome) = pending.next().await {
        let outcome = outcome?;
        if outcome.uploaded {
            counts.uploaded += 1;
        } else {
            counts.skipped += 1;
        }
        remote.insert(outcome.path_key, outcome.server_entry);
        if let Some(companion) = outcome.generated_companion {
            remote.insert(collision_key(&companion.path), companion);
        }
    }
    Ok(counts)
}

fn is_managed_conversation_portable_entry(path: &str, entry: &PortableExportEntry) -> bool {
    entry.kind == EntryKind::Markdown
        && entry.media_type == "text/markdown"
        && managed_conversation_id(path, &entry.metadata).is_some()
}

fn validate_managed_conversation_portable_entry(
    path: &str,
    entry: &PortableExportEntry,
    inspected: &InspectedFile,
) -> Result<()> {
    if entry.size_bytes != inspected.size_bytes
        || strip_sha256(&entry.content_hash) != inspected.sha256
    {
        bail!("managed conversation bytes do not match the portable export manifest");
    }
    let bytes = inspected
        .bytes
        .as_deref()
        .ok_or_else(|| anyhow!("managed conversation exceeds the import collection limit"))?;
    let markdown = std::str::from_utf8(bytes)
        .context("managed conversation export content is not valid UTF-8")?;
    messaging_protocol::validate_conversation_entry(path, &entry.metadata, markdown)
        .map_err(|error| anyhow!(error.to_string()))?
        .ok_or_else(|| anyhow!("managed conversation export lacks canonical metadata"))?;
    Ok(())
}

fn inventory(root: &Path, args: &ImportArgs) -> Result<InventoryManifest> {
    let includes = build_globs(&args.include)?;
    let excludes = build_globs(&args.exclude)?;
    let portable = load_portable_export_metadata(root)?;
    let mut entries = Vec::new();
    let mut ignored = Vec::new();
    let mut markdown_source_paths = Vec::new();
    let mut collisions = BTreeMap::<String, String>::new();

    for item in WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let item = item?;
        if item.path() == root || item.file_type().is_dir() {
            continue;
        }
        let relative = relative_path(root, item.path())?;
        if item.file_type().is_symlink() {
            ignored.push(IgnoredEntry {
                path: relative,
                reason: "symbolic_link_not_followed".to_owned(),
            });
            continue;
        }
        if !item.file_type().is_file() {
            bail!("workspace import rejects special file {relative:?}");
        }
        if includes
            .as_ref()
            .is_some_and(|set| !set.is_match(&relative))
        {
            ignored.push(IgnoredEntry {
                path: relative,
                reason: "not_included".to_owned(),
            });
            continue;
        }
        if excludes.as_ref().is_some_and(|set| set.is_match(&relative)) {
            ignored.push(IgnoredEntry {
                path: relative,
                reason: "excluded".to_owned(),
            });
            continue;
        }
        validate_relative_path(&relative)?;
        let normalized = collision_key(&relative);
        if let Some(previous) = collisions.insert(normalized, relative.clone()) {
            bail!(
                "portable path collision between {previous:?} and {relative:?}; case and Unicode-normalized paths must be unique"
            );
        }

        let metadata = item.metadata()?;
        let portable_entry = portable.get(&relative);
        let managed_portable = portable_entry
            .filter(|candidate| is_managed_conversation_portable_entry(&relative, candidate));
        if managed_portable
            .is_some_and(|candidate| candidate.size_bytes > MAX_MANAGED_CONVERSATION_BYTES)
        {
            bail!(
                "{relative} exceeds the managed conversation {}-byte limit",
                messaging_protocol::MAX_CANONICAL_CONVERSATION_BYTES
            );
        }
        let collect_limit = if managed_portable.is_some() {
            MAX_MANAGED_CONVERSATION_BYTES
        } else {
            MAX_TEXT_BYTES
        };
        let inspected = inspect_file(item.path(), &metadata, collect_limit)?;
        if let Some(candidate) = managed_portable {
            validate_managed_conversation_portable_entry(&relative, candidate, &inspected)?;
        }
        let kind = classify_file(item.path(), inspected.bytes.as_deref());
        let matching_portable = portable_entry.filter(|candidate| {
            candidate.kind == kind
                && strip_sha256(&candidate.content_hash) == inspected.sha256
                && candidate.size_bytes == inspected.size_bytes
        });
        let detected_media_type = media_type(item.path(), inspected.bytes.as_deref(), kind);
        let observed = observed_file(&metadata);
        let portable_metadata = matching_portable
            .map(|value| PortableMetadata {
                modified_unix_ns: value.modified_unix_ns,
                mode: value.mode,
            })
            .unwrap_or_else(|| PortableMetadata {
                modified_unix_ns: observed.modified_unix_ns,
                mode: observed.mode,
            });
        let media_type = matching_portable
            .map(|value| value.media_type.clone())
            .filter(|value| {
                kind == EntryKind::Binary
                    || matches!(value.as_str(), "text/markdown" | "text/plain")
            })
            .unwrap_or(detected_media_type);
        validate_media_type(&media_type)?;
        if kind == EntryKind::Markdown
            && inspected.size_bytes <= MAX_CONTEXT_SOURCE_BYTES
            && relative.to_ascii_lowercase().ends_with(".md")
        {
            markdown_source_paths.push(relative.clone());
        }
        entries.push(InventoryEntry {
            path: relative,
            kind,
            content_hash: format!("sha256:{}", inspected.sha256),
            size_bytes: inspected.size_bytes,
            media_type,
            portable: portable_metadata,
            attachment_context: Vec::new(),
            source_metadata: matching_portable
                .map(|value| value.metadata.clone())
                .unwrap_or(Value::Null),
            observed,
        });
    }

    entries.sort_by(|left, right| left.path.cmp(&right.path));
    ignored.sort_by(|left, right| left.path.cmp(&right.path));
    attach_context_from_files(root, &mut entries, &markdown_source_paths)?;
    let totals = InventoryTotals {
        files: entries.len(),
        markdown_files: entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::Markdown)
            .count(),
        binary_files: entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::Binary)
            .count(),
        bytes: entries.iter().map(|entry| entry.size_bytes).sum(),
        ignored: ignored.len(),
    };
    let identity = json!({
        "format": IMPORT_MANIFEST_FORMAT,
        "version": 1,
        "entries": entries,
        "ignored": ignored
    });
    let inventory_sha256 = hash_bytes(&serde_json::to_vec(&identity)?);
    Ok(InventoryManifest {
        format: IMPORT_MANIFEST_FORMAT.to_owned(),
        version: 1,
        root: root.display().to_string(),
        entries,
        ignored,
        totals,
        inventory_sha256,
    })
}

struct InspectedFile {
    sha256: String,
    size_bytes: u64,
    bytes: Option<Vec<u8>>,
}

fn inspect_file(
    path: &Path,
    expected: &std::fs::Metadata,
    collect_limit: u64,
) -> Result<InspectedFile> {
    let mut file =
        File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let opened = file.metadata()?;
    ensure_same_file(path, expected, &opened)?;
    let mut hasher = Sha256::new();
    let mut size_bytes = 0_u64;
    let collect = opened.len() <= collect_limit;
    let mut bytes = collect.then(|| Vec::with_capacity(opened.len() as usize));
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size_bytes = size_bytes
            .checked_add(u64::try_from(count)?)
            .ok_or_else(|| anyhow!("file size overflow"))?;
        if let Some(bytes) = &mut bytes {
            bytes.extend_from_slice(&buffer[..count]);
        }
    }
    ensure_same_file(path, expected, &file.metadata()?)?;
    if size_bytes != opened.len() {
        bail!("{} changed while it was inventoried", path.display());
    }
    Ok(InspectedFile {
        sha256: hex::encode(hasher.finalize()),
        size_bytes,
        bytes,
    })
}

fn classify_file(path: &Path, bytes: Option<&[u8]>) -> EntryKind {
    let Some(bytes) = bytes else {
        return EntryKind::Binary;
    };
    if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        return EntryKind::Binary;
    }
    if let Some(kind) = infer::get(bytes)
        && !is_textual_media_type(kind.mime_type())
    {
        return EntryKind::Binary;
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "heic"
            | "pdf"
            | "zip"
            | "gz"
            | "bz2"
            | "xz"
            | "sqlite"
            | "db"
            | "parquet"
            | "woff"
            | "woff2"
    ) {
        return EntryKind::Binary;
    }
    EntryKind::Markdown
}

fn media_type(path: &Path, bytes: Option<&[u8]>, kind: EntryKind) -> String {
    match kind {
        EntryKind::Markdown => {
            if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    matches!(value.to_ascii_lowercase().as_str(), "md" | "markdown")
                })
            {
                "text/markdown".to_owned()
            } else {
                "text/plain".to_owned()
            }
        }
        EntryKind::Binary => bytes
            .and_then(infer::get)
            .map(|value| value.mime_type().to_owned())
            .or_else(|| {
                mime_guess::from_path(path)
                    .first_raw()
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| "application/octet-stream".to_owned()),
    }
}

fn is_textual_media_type(value: &str) -> bool {
    value.starts_with("text/")
        || matches!(
            value,
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/sql"
                | "application/yaml"
                | "application/toml"
        )
        || value.ends_with("+json")
        || value.ends_with("+xml")
}

async fn write_markdown(
    client: &ApiClient,
    root: &Path,
    entry: &InventoryEntry,
    companion_binary: Option<&InventoryEntry>,
    expected_version: i64,
) -> Result<RemoteEntry> {
    let bytes = read_unchanged_file(root, entry)?;
    let content = String::from_utf8(bytes).with_context(|| {
        format!(
            "{} was valid UTF-8 during inventory but is no longer valid",
            entry.path
        )
    })?;
    let metadata = markdown_metadata(entry);
    let target_identity = tier_a_history_stage(&entry.source_metadata)?
        .map(|stage| stage.target_lineage_ordinal.to_string())
        .unwrap_or_default();
    let idempotency_key = format!(
        "workspace-import:{}",
        &hash_bytes(
            format!(
                "{}\0{}\0{}",
                entry.path, entry.content_hash, target_identity
            )
            .as_bytes()
        )[..32]
    );
    let response = client
        .post_json(
            "/v1/workspace/write",
            &json!({
                "path": entry.path,
                "content": content,
                "media_type": entry.media_type,
                "expected_version": expected_version,
                "idempotency_key": idempotency_key,
                "metadata": metadata
            }),
        )
        .await
        .with_context(|| format!("could not import {}", entry.path))?;
    let data = unwrap_data(&response);
    let remote = RemoteEntry {
        entry_ref: required_str(data, "entry_ref")?.to_owned(),
        path: required_str(data, "path")?.to_owned(),
        kind: "markdown".to_owned(),
        version: required_i64(data, "version")?,
        content_hash: required_str(data, "content_hash")?.to_owned(),
        size_bytes: entry.size_bytes,
        metadata,
    };
    verify_upload_receipt(entry, &remote, companion_binary)?;
    Ok(remote)
}

async fn upload_binary(
    client: &ApiClient,
    root: &Path,
    entry: &InventoryEntry,
    portable_companion: Option<&InventoryEntry>,
    describe: bool,
    expected_version: i64,
) -> Result<(RemoteEntry, RemoteEntry)> {
    if entry.size_bytes > MAX_STREAMED_BINARY_BYTES {
        bail!(
            "{} is {} bytes; the current workspace binary endpoint is limited to {} bytes",
            entry.path,
            entry.size_bytes,
            MAX_STREAMED_BINARY_BYTES
        );
    }
    let source = safe_source_path(root, &entry.path)?;
    let file = File::open(&source)?;
    let opened_metadata = file.metadata()?;
    ensure_observed_file(entry, &opened_metadata)?;
    let provenance = binary_provenance(entry)?;
    let limitations = entry
        .source_metadata
        .get("limitations")
        .and_then(Value::as_str)
        .unwrap_or(
            "Imported exact bytes are authoritative; the companion Markdown is retrieval context.",
        )
        .to_owned();
    let response = if entry.size_bytes <= MAX_MULTIPART_BINARY_BYTES {
        let file_name = source
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("binary")
            .to_owned();
        let stream = ReaderStream::new(tokio::fs::File::from_std(file));
        let part = multipart::Part::stream_with_length(Body::wrap_stream(stream), entry.size_bytes)
            .file_name(file_name)
            .mime_str(&entry.media_type)?;
        let mut form = multipart::Form::new()
            .text("path", entry.path.clone())
            .text("media_type", entry.media_type.clone())
            .text("expected_content_hash", entry.content_hash.clone())
            .text("expected_version", expected_version.to_string())
            .text("provenance", provenance.clone())
            .text("limitations", limitations.clone());
        if let Some(value) = entry.portable.modified_unix_ns {
            form = form.text("mtime_ns", value.to_string());
        }
        if let Some(value) = entry.portable.mode {
            form = form.text("mode", value.to_string());
        }
        form = form.part("file", part);
        if let Some(companion) = portable_companion {
            let content = read_unchanged_file(root, companion)?;
            let description = String::from_utf8(content)
                .context("portable binary companion is not valid UTF-8")?;
            form = form
                .text("description", description)
                .text(
                    "portable_companion_format",
                    TIER_A_PORTABLE_COMPANION_FORMAT,
                )
                .text("portable_companion_path", companion.path.clone())
                .text("portable_companion_sha256", companion.content_hash.clone());
            if let Some(value) = companion.portable.modified_unix_ns {
                form = form.text("portable_companion_mtime_ns", value.to_string());
            }
            if let Some(value) = companion.portable.mode {
                form = form.text("portable_companion_mode", value.to_string());
            }
        } else if describe {
            form = form.text("description", binary_description(entry));
        }
        client.post_multipart("/v1/workspace/binaries", form).await
    } else {
        let stream = ReaderStream::new(tokio::fs::File::from_std(file));
        let mtime = entry
            .portable
            .modified_unix_ns
            .and_then(|value| i64::try_from(value).ok())
            .map(|value| value.to_string());
        let mode = entry.portable.mode.map(|value| value.to_string());
        let expected_version = expected_version.to_string();
        let expected_content_hash = entry.content_hash.clone();
        let mut query = vec![
            ("path", entry.path.as_str()),
            ("media_type", entry.media_type.as_str()),
            ("expected_content_hash", expected_content_hash.as_str()),
            ("provenance", provenance.as_str()),
            ("expected_version", expected_version.as_str()),
        ];
        if let Some(value) = &mtime {
            query.push(("mtime_ns", value.as_str()));
        }
        if let Some(value) = &mode {
            query.push(("mode", value.as_str()));
        }
        client
            .put_stream(
                "/v1/workspace/binaries/content",
                &query,
                &entry.media_type,
                entry.size_bytes,
                Body::wrap_stream(stream),
            )
            .await
    }
    .with_context(|| format!("could not import {}", entry.path))?;
    let data = unwrap_data(&response);
    if required_str(data, "media_type")? != entry.media_type {
        bail!("binary media type verification failed for {}", entry.path);
    }
    let companion = data
        .get("companion")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("binary upload response lacks companion"))?;
    let companion_path = companion
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("binary upload response lacks companion path"))?
        .to_owned();
    let companion_entry = RemoteEntry {
        entry_ref: companion
            .get("entry_ref")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("binary upload response lacks companion entry_ref"))?
            .to_owned(),
        path: companion_path.clone(),
        kind: "markdown".to_owned(),
        version: companion
            .get("version")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("binary upload response lacks companion version"))?,
        content_hash: portable_companion
            .map(|value| value.content_hash.clone())
            .unwrap_or_default(),
        size_bytes: portable_companion.map_or(0, |value| value.size_bytes),
        metadata: portable_companion
            .map(|value| normalized_portable_companion_metadata(entry, value))
            .unwrap_or_else(|| json!({"kind": "binary_description", "binary_path": entry.path})),
    };
    let remote = RemoteEntry {
        entry_ref: required_str(data, "entry_ref")?.to_owned(),
        path: required_str(data, "path")?.to_owned(),
        kind: "binary".to_owned(),
        version: required_i64(data, "version")?,
        content_hash: required_str(data, "content_hash")?.to_owned(),
        size_bytes: required_u64(data, "size_bytes")?,
        metadata: json!({
            "companion_path": companion_path,
            "description_status": data.get("description_status"),
            "portable": entry.portable,
            "provenance": provenance,
            "limitations": limitations
        }),
    };
    verify_upload_receipt(entry, &remote, None)?;
    let metadata_response = client
        .get_json(&format!("/v1/workspace/binaries/{}", remote.entry_ref), &[])
        .await?;
    let metadata = unwrap_data(&metadata_response);
    if required_str(metadata, "path")? != entry.path
        || strip_sha256(required_str(metadata, "content_hash")?)
            != strip_sha256(&entry.content_hash)
        || required_u64(metadata, "size_bytes")? != entry.size_bytes
    {
        bail!("binary metadata verification failed for {}", entry.path);
    }
    Ok((remote, companion_entry))
}

fn normalized_portable_companion_metadata(
    binary: &InventoryEntry,
    companion: &InventoryEntry,
) -> Value {
    json!({
        "_straylight_import": {
            "format": IMPORT_MANIFEST_FORMAT,
            "portable_companion_format": TIER_A_PORTABLE_COMPANION_FORMAT,
            "content_sha256": companion.content_hash,
        },
        "binary_path": binary.path,
        "content_hash": binary.content_hash,
        "description_status": "byte_copied",
        "kind": "binary_description",
        "portable": companion.portable,
    })
}

fn markdown_metadata(entry: &InventoryEntry) -> Value {
    let mut metadata = entry
        .source_metadata
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    metadata.insert(
        "portable".to_owned(),
        serde_json::to_value(&entry.portable).unwrap_or_else(|_| json!({})),
    );
    metadata.insert(
        "_straylight_import".to_owned(),
        json!({"format": IMPORT_MANIFEST_FORMAT}),
    );
    Value::Object(metadata)
}

fn binary_provenance(entry: &InventoryEntry) -> Result<String> {
    let original = entry
        .source_metadata
        .get("provenance")
        .cloned()
        .and_then(unwrap_original_provenance)
        .unwrap_or(Value::Null);
    let mut payload = json!({
        "format": IMPORT_MANIFEST_FORMAT,
        "portable": entry.portable,
        "attachment_context": entry.attachment_context,
        "original_provenance": original
    });
    let mut serialized = serde_json::to_string(&payload)?;
    if serialized.len() > MAX_BINARY_PROVENANCE_BYTES {
        let original_hash = hash_bytes(
            payload
                .get("original_provenance")
                .map(Value::to_string)
                .unwrap_or_default()
                .as_bytes(),
        );
        payload["original_provenance"] = json!({
            "omitted_from_upload": true,
            "sha256": format!("sha256:{original_hash}")
        });
        serialized = serde_json::to_string(&payload)?;
    }
    if serialized.len() > MAX_BINARY_PROVENANCE_BYTES {
        bail!("bounded binary provenance unexpectedly exceeds its transport limit");
    }
    Ok(serialized)
}

fn unwrap_original_provenance(value: Value) -> Option<Value> {
    let parsed = value
        .as_str()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    if parsed
        .as_ref()
        .and_then(|value| value.get("format"))
        .and_then(Value::as_str)
        == Some(IMPORT_MANIFEST_FORMAT)
    {
        parsed.and_then(|value| value.get("original_provenance").cloned())
    } else {
        Some(value)
    }
}

fn binary_description(entry: &InventoryEntry) -> String {
    if entry.attachment_context.is_empty() {
        return format!(
            "Imported binary `{}` ({}, {} bytes).",
            entry.path, entry.media_type, entry.size_bytes
        );
    }
    let mut output = format!(
        "Imported binary `{}` ({}, {} bytes).\n\nReferenced by:\n",
        entry.path, entry.media_type, entry.size_bytes
    );
    for context in &entry.attachment_context {
        output.push_str(&format!(
            "- `{}`: {}\n",
            context.source_markdown_path, context.excerpt
        ));
    }
    output
}

async fn fetch_remote_manifest(client: &ApiClient) -> Result<BTreeMap<String, RemoteEntry>> {
    let mut page_request = RemoteManifestPage::Offset(0);
    let mut entries = BTreeMap::new();
    loop {
        let mut owned_query = vec![("limit", MANIFEST_PAGE_SIZE.to_string())];
        match &page_request {
            RemoteManifestPage::Offset(offset) => {
                owned_query.push(("offset", offset.to_string()));
            }
            RemoteManifestPage::Cursor(cursor) => {
                owned_query.push(("after_path", cursor.after_path.clone()));
                owned_query.push(("after_entry_ref", cursor.after_entry_ref.clone()));
            }
        }
        let query = owned_query
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect::<Vec<_>>();
        let response = client.get_json("/v1/workspace/manifest", &query).await?;
        let data = unwrap_data(&response);
        let page = data
            .get("entries")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("workspace manifest lacks entries"))?;
        for value in page {
            let entry: RemoteEntry = serde_json::from_value(value.clone())
                .context("workspace manifest contains an invalid entry")?;
            validate_relative_path(&entry.path)?;
            let key = collision_key(&entry.path);
            if entries.insert(key, entry).is_some() {
                bail!("workspace manifest contains a normalized path collision");
            }
        }
        if let Some(value) = data.get("next").filter(|value| !value.is_null()) {
            if page.is_empty() {
                bail!("workspace manifest returned a cursor for an empty page");
            }
            let cursor: RemoteManifestCursor = serde_json::from_value(value.clone())
                .context("workspace manifest has an invalid cursor")?;
            validate_relative_path(&cursor.after_path)?;
            if !cursor.after_entry_ref.starts_with("entry:") {
                bail!("workspace manifest cursor has an invalid entry reference");
            }
            page_request = RemoteManifestPage::Cursor(cursor);
            continue;
        }
        match page_request {
            RemoteManifestPage::Cursor(_) => break,
            RemoteManifestPage::Offset(offset) if page.len() == MANIFEST_PAGE_SIZE => {
                page_request = RemoteManifestPage::Offset(
                    offset
                        .checked_add(page.len())
                        .ok_or_else(|| anyhow!("workspace manifest offset overflow"))?,
                );
            }
            RemoteManifestPage::Offset(_) => break,
        }
    }
    Ok(entries)
}

fn tier_a_history_stage(metadata: &Value) -> Result<Option<TierAHistoryStage>> {
    let Some(value) = metadata.get("_straylight_tier_a_history") else {
        return Ok(None);
    };
    if value.get("format").and_then(Value::as_str) != Some(TIER_A_HISTORY_STAGE_FORMAT) {
        bail!("Tier-A history stage metadata has a missing or unsupported format");
    }
    let target_lineage_ordinal = value
        .get("target_lineage_ordinal")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow!("Tier-A history stage target ordinal must be positive"))?;
    let semantics = match value.get("semantics").and_then(Value::as_str) {
        Some(TIER_A_ORDINARY_HISTORY_SEMANTICS) => TierAHistorySemantics::OrdinaryContentTransition,
        Some(TIER_A_EXACT_HISTORY_SEMANTICS) => {
            TierAHistorySemantics::PreserveIntentionalExactBytesVersion
        }
        _ => bail!("Tier-A history stage semantics are missing or unsupported"),
    };
    if semantics == TierAHistorySemantics::PreserveIntentionalExactBytesVersion
        && target_lineage_ordinal < 2
    {
        bail!("an intentional exact-byte history version must target ordinal 2 or later");
    }
    Ok(Some(TierAHistoryStage {
        target_lineage_ordinal,
        semantics,
    }))
}

fn content_identity_matches(local: &InventoryEntry, remote: &RemoteEntry) -> bool {
    strip_sha256(&remote.content_hash) == strip_sha256(&local.content_hash)
        && remote.size_bytes == local.size_bytes
}

fn normalized_portable_companion_identity_matches(
    companion: &InventoryEntry,
    binary: &InventoryEntry,
    remote: &RemoteEntry,
) -> bool {
    let Some(portable) = remote
        .metadata
        .get("portable")
        .filter(|value| value.is_object())
        .cloned()
        .and_then(|value| serde_json::from_value::<PortableMetadata>(value).ok())
    else {
        return false;
    };
    let Some(import) = remote
        .metadata
        .get("_straylight_import")
        .filter(|value| value.is_object())
    else {
        return false;
    };
    companion.path == remote.path
        && companion.kind.server_name() == remote.kind
        && content_identity_matches(companion, remote)
        && portable == companion.portable
        && remote.metadata.get("kind").and_then(Value::as_str) == Some("binary_description")
        && remote.metadata.get("binary_path").and_then(Value::as_str) == Some(binary.path.as_str())
        && remote
            .metadata
            .get("description_status")
            .and_then(Value::as_str)
            == Some("byte_copied")
        && remote
            .metadata
            .get("content_hash")
            .and_then(Value::as_str)
            .is_some_and(|value| strip_sha256(value) == strip_sha256(&binary.content_hash))
        && import.get("format").and_then(Value::as_str) == Some(IMPORT_MANIFEST_FORMAT)
        && import
            .get("portable_companion_format")
            .and_then(Value::as_str)
            == Some(TIER_A_PORTABLE_COMPANION_FORMAT)
        && import
            .get("content_sha256")
            .and_then(Value::as_str)
            .is_some_and(|value| strip_sha256(value) == strip_sha256(&companion.content_hash))
}

fn remote_history_identity_matches(
    local: &InventoryEntry,
    remote: &RemoteEntry,
    stage: TierAHistoryStage,
) -> Result<bool> {
    let local_legacy = local.source_metadata.get("legacy");
    let remote_legacy = remote.metadata.get("legacy");
    let legacy_matches = local_legacy.is_some()
        && local_legacy == remote_legacy
        && remote_legacy
            .and_then(|value| value.get("lineage_ordinal"))
            .and_then(Value::as_i64)
            == Some(stage.target_lineage_ordinal);
    Ok(legacy_matches
        && tier_a_history_stage(&remote.metadata)?.is_none_or(|remote_stage| remote_stage == stage))
}

fn complete_current_identity_matches(
    local: &InventoryEntry,
    remote: &RemoteEntry,
    companion_ready: bool,
    stage: TierAHistoryStage,
) -> Result<bool> {
    Ok(content_identity_matches(local, remote)
        && remote_portable_metadata(remote) == local.portable
        && companion_ready
        && remote_history_identity_matches(local, remote, stage)?)
}

fn plan_import_entry(
    entry: &InventoryEntry,
    current: Option<&RemoteEntry>,
    companion_ready: bool,
    companion_binary: Option<&InventoryEntry>,
) -> Result<ImportDecision> {
    let stage = tier_a_history_stage(&entry.source_metadata)?;
    if let Some(binary) = companion_binary {
        let target = stage.map_or(1, |value| value.target_lineage_ordinal);
        if stage.is_some_and(|value| {
            value.semantics == TierAHistorySemantics::PreserveIntentionalExactBytesVersion
        }) {
            bail!(
                "same-byte binary companion history is unsupported for {:?}; \
                 refusing to collapse or synthesize a companion version",
                entry.path
            );
        }
        let Some(current) = current else {
            bail!(
                "Tier-A binary companion {:?} is absent at target {}; \
                 it must be imported atomically with its binary",
                entry.path,
                target
            );
        };
        if current.version < target {
            bail!(
                "Tier-A binary companion {:?} remains at predecessor ordinal {}; \
                 target {} must be advanced atomically with its binary",
                entry.path,
                current.version,
                target
            );
        }
        if current.version > target {
            bail!(
                "Tier-A binary companion history is ahead for {:?}: \
                 current ordinal {} exceeds target {}",
                entry.path,
                current.version,
                target
            );
        }
        if !normalized_portable_companion_identity_matches(entry, binary, current) {
            bail!(
                "Tier-A binary companion target identity differs for {:?} at ordinal {}",
                entry.path,
                target
            );
        }
        return Ok(ImportDecision {
            upload: false,
            expected_version: target,
        });
    }
    let Some(stage) = stage else {
        let matching_identity = current.is_some_and(|value| {
            content_identity_matches(entry, value)
                && remote_portable_metadata(value) == entry.portable
        });
        return Ok(ImportDecision {
            expected_version: current.map_or(0, |value| value.version),
            upload: !(matching_identity && companion_ready),
        });
    };
    if entry.kind != EntryKind::Markdown {
        bail!(
            "Tier-A ordinal replay metadata is supported only for Markdown; \
             binary history must fail closed"
        );
    }
    let Some(current) = current else {
        if stage.target_lineage_ordinal != 1 {
            bail!(
                "Tier-A history gap for {:?}: target ordinal {} has no predecessor",
                entry.path,
                stage.target_lineage_ordinal
            );
        }
        return Ok(ImportDecision {
            upload: true,
            expected_version: 0,
        });
    };
    let current_version = current.version;
    let target = stage.target_lineage_ordinal;
    if current_version < target - 1 {
        bail!(
            "Tier-A history gap for {:?}: current ordinal {} cannot reach target {}",
            entry.path,
            current_version,
            target
        );
    }
    if current_version > target {
        bail!(
            "Tier-A history is ahead for {:?}: current ordinal {} exceeds target {}",
            entry.path,
            current_version,
            target
        );
    }
    if current_version == target {
        if !complete_current_identity_matches(entry, current, companion_ready, stage)? {
            bail!(
                "Tier-A history target identity differs for {:?} at ordinal {}",
                entry.path,
                target
            );
        }
        return Ok(ImportDecision {
            upload: false,
            expected_version: target,
        });
    }
    if current_version != target - 1 {
        bail!(
            "Tier-A history ordinal for {:?} is neither predecessor nor target",
            entry.path
        );
    }
    let content_matches = content_identity_matches(entry, current);
    match stage.semantics {
        TierAHistorySemantics::PreserveIntentionalExactBytesVersion if !content_matches => {
            bail!(
                "Tier-A exact-history semantics for {:?} do not match the predecessor bytes",
                entry.path
            );
        }
        TierAHistorySemantics::OrdinaryContentTransition if content_matches => {
            bail!(
                "Tier-A stage for {:?} repeats predecessor bytes without exact-history semantics",
                entry.path
            );
        }
        _ => {}
    }
    Ok(ImportDecision {
        upload: true,
        expected_version: current_version,
    })
}

fn ensure_remote_identity(local: &InventoryEntry, remote: &RemoteEntry) -> Result<()> {
    if local.path != remote.path {
        bail!(
            "workspace path {:?} collides with server path {:?}; case-only and Unicode-only renames need an explicit server rename",
            local.path,
            remote.path
        );
    }
    if local.kind.server_name() != remote.kind {
        bail!(
            "workspace path {:?} is {} locally but {} on the server",
            local.path,
            local.kind.server_name(),
            remote.kind
        );
    }
    Ok(())
}

fn verify_upload_receipt(
    local: &InventoryEntry,
    remote: &RemoteEntry,
    companion_binary: Option<&InventoryEntry>,
) -> Result<()> {
    ensure_remote_identity(local, remote)?;
    if strip_sha256(&remote.content_hash) != strip_sha256(&local.content_hash)
        || remote.size_bytes != local.size_bytes
    {
        bail!(
            "server integrity receipt does not match {}: expected {} / {} bytes, received {} / {} bytes",
            local.path,
            local.content_hash,
            local.size_bytes,
            remote.content_hash,
            remote.size_bytes
        );
    }
    let stage = tier_a_history_stage(&local.source_metadata)?;
    if let Some(binary) = companion_binary {
        let target = stage.map_or(1, |value| value.target_lineage_ordinal);
        if stage.is_some_and(|value| {
            value.semantics == TierAHistorySemantics::PreserveIntentionalExactBytesVersion
        }) {
            bail!(
                "same-byte binary companion history is unsupported for {}",
                local.path
            );
        }
        if remote.version != target {
            bail!(
                "server history receipt does not match {}: expected ordinal {}, received {}",
                local.path,
                target,
                remote.version
            );
        }
        if !normalized_portable_companion_identity_matches(local, binary, remote) {
            bail!(
                "server companion receipt does not retain the Tier-A stable identity for {}",
                local.path
            );
        }
        return Ok(());
    }
    if let Some(stage) = stage {
        if remote.version != stage.target_lineage_ordinal {
            bail!(
                "server history receipt does not match {}: expected ordinal {}, received {}",
                local.path,
                stage.target_lineage_ordinal,
                remote.version
            );
        }
        if !remote_history_identity_matches(local, remote, stage)? {
            bail!(
                "server history receipt does not retain the Tier-A target identity for {}",
                local.path
            );
        }
    }
    Ok(())
}

fn verify_complete_import(
    local: &[InventoryEntry],
    remote: &BTreeMap<String, RemoteEntry>,
) -> Result<()> {
    for entry in local {
        let server = remote
            .get(&collision_key(&entry.path))
            .ok_or_else(|| anyhow!("server manifest is missing {}", entry.path))?;
        let companion_binary = tier_a_binary_for_companion(entry, local)?;
        verify_upload_receipt(entry, server, companion_binary)?;
        if entry.kind == EntryKind::Binary && !binary_companion_is_present(server, remote) {
            bail!(
                "server manifest is missing the companion for {}",
                entry.path
            );
        }
    }
    Ok(())
}

fn binary_companion_is_present(
    binary: &RemoteEntry,
    remote: &BTreeMap<String, RemoteEntry>,
) -> bool {
    binary
        .metadata
        .get("companion_path")
        .and_then(Value::as_str)
        .and_then(|path| remote.get(&collision_key(path)))
        .is_some_and(|entry| entry.kind == "markdown")
}

fn remote_portable_metadata(remote: &RemoteEntry) -> PortableMetadata {
    if let Some(portable) = remote
        .metadata
        .get("portable")
        .filter(|value| !value.is_null())
    {
        return serde_json::from_value(portable.clone()).unwrap_or_default();
    }
    let Some(provenance) = remote.metadata.get("provenance").and_then(Value::as_str) else {
        return PortableMetadata::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(provenance) else {
        return PortableMetadata::default();
    };
    if value.get("format").and_then(Value::as_str) != Some(IMPORT_MANIFEST_FORMAT) {
        return PortableMetadata::default();
    }
    value
        .get("portable")
        .cloned()
        .and_then(|portable| serde_json::from_value(portable).ok())
        .unwrap_or_default()
}

fn read_unchanged_file(root: &Path, entry: &InventoryEntry) -> Result<Vec<u8>> {
    let path = safe_source_path(root, &entry.path)?;
    let metadata = std::fs::metadata(&path)?;
    ensure_observed_file(entry, &metadata)?;
    let bytes = std::fs::read(&path)?;
    if u64::try_from(bytes.len())? != entry.size_bytes
        || hash_bytes(&bytes) != strip_sha256(&entry.content_hash)
    {
        bail!("{} changed after inventory; rerun the import", entry.path);
    }
    Ok(bytes)
}

fn ensure_observed_file(entry: &InventoryEntry, metadata: &std::fs::Metadata) -> Result<()> {
    if !metadata.is_file()
        || metadata.len() != entry.size_bytes
        || file_device_id(metadata) != entry.observed.device_id
        || file_inode(metadata) != entry.observed.inode
        || system_time_ns(metadata.modified().ok()) != entry.observed.modified_unix_ns
    {
        bail!("{} changed after inventory; rerun the import", entry.path);
    }
    Ok(())
}

fn ensure_same_file(
    path: &Path,
    expected: &std::fs::Metadata,
    observed: &std::fs::Metadata,
) -> Result<()> {
    if !observed.is_file()
        || expected.len() != observed.len()
        || file_device_id(expected) != file_device_id(observed)
        || file_inode(expected) != file_inode(observed)
    {
        bail!("{} changed while it was being read", path.display());
    }
    Ok(())
}

fn observed_file(metadata: &std::fs::Metadata) -> ObservedFile {
    ObservedFile {
        modified_unix_ns: system_time_ns(metadata.modified().ok()),
        mode: file_mode(metadata),
        device_id: file_device_id(metadata),
        inode: file_inode(metadata),
    }
}

struct AttachmentIndex {
    paths: BTreeSet<String>,
    basenames: BTreeMap<String, Vec<String>>,
    binary_paths: BTreeSet<String>,
}

fn attachment_index(entries: &[InventoryEntry]) -> AttachmentIndex {
    let paths = entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    let mut basenames = BTreeMap::<String, Vec<String>>::new();
    for path in &paths {
        if let Some(name) = path.rsplit('/').next() {
            basenames
                .entry(name.to_owned())
                .or_default()
                .push(path.clone());
        }
    }
    let binary_paths = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::Binary)
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    AttachmentIndex {
        paths,
        basenames,
        binary_paths,
    }
}

fn attach_context_from_files(
    root: &Path,
    entries: &mut [InventoryEntry],
    source_paths: &[String],
) -> Result<()> {
    let index = attachment_index(entries);
    let mut found = BTreeMap::<String, Vec<AttachmentContext>>::new();
    for source_path in source_paths {
        let entry = entries
            .iter()
            .find(|entry| entry.path == *source_path)
            .ok_or_else(|| anyhow!("attachment context source disappeared from inventory"))?;
        let bytes = read_unchanged_file(root, entry)?;
        let text = std::str::from_utf8(&bytes)
            .with_context(|| format!("{source_path} is no longer valid UTF-8"))?;
        collect_attachment_context(source_path, text, &index, &mut found);
    }
    apply_attachment_context(entries, found);
    Ok(())
}

#[cfg(test)]
fn attach_context(entries: &mut [InventoryEntry], sources: &[(String, String)]) {
    let index = attachment_index(entries);
    let mut found = BTreeMap::<String, Vec<AttachmentContext>>::new();
    for (source_path, text) in sources {
        collect_attachment_context(source_path, text, &index, &mut found);
    }
    apply_attachment_context(entries, found);
}

fn collect_attachment_context(
    source_path: &str,
    text: &str,
    index: &AttachmentIndex,
    found: &mut BTreeMap<String, Vec<AttachmentContext>>,
) {
    for (target, offset) in link_targets(text) {
        let Some(resolved) = resolve_link(source_path, &target, &index.paths, &index.basenames)
        else {
            continue;
        };
        if !index.binary_paths.contains(&resolved) {
            continue;
        }
        let contexts = found.entry(resolved).or_default();
        if contexts.len() >= MAX_CONTEXTS_PER_BINARY {
            continue;
        }
        let excerpt = context_excerpt(text, offset);
        let context = AttachmentContext {
            source_markdown_path: source_path.to_owned(),
            excerpt,
        };
        if !context.excerpt.is_empty() && !contexts.contains(&context) {
            contexts.push(context);
        }
    }
}

fn apply_attachment_context(
    entries: &mut [InventoryEntry],
    mut found: BTreeMap<String, Vec<AttachmentContext>>,
) {
    for entry in entries {
        entry.attachment_context = found.remove(&entry.path).unwrap_or_default();
    }
}

fn link_targets(text: &str) -> Vec<(String, usize)> {
    let mut output = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0_usize;
    while index + 1 < bytes.len() && output.len() < 256 {
        if bytes[index] == b'[' && bytes[index + 1] == b'[' {
            if let Some(end) = text[index + 2..].find("]]") {
                let value = &text[index + 2..index + 2 + end];
                output.push((value.split('|').next().unwrap_or(value).to_owned(), index));
                index += end + 4;
                continue;
            }
        } else if bytes[index] == b']'
            && bytes[index + 1] == b'('
            && let Some(end) = text[index + 2..].find(')')
        {
            let value = text[index + 2..index + 2 + end]
                .trim()
                .trim_matches('<')
                .trim_matches('>');
            output.push((value.to_owned(), index));
            index += end + 3;
            continue;
        }
        index += 1;
    }
    output
}

fn resolve_link(
    source_path: &str,
    raw: &str,
    paths: &BTreeSet<String>,
    basenames: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    let target = raw.trim();
    if target.is_empty()
        || target.starts_with(['/', '#'])
        || target.contains("://")
        || target.starts_with("mailto:")
        || target.starts_with("data:")
        || target.contains(['#', '?', '\\'])
    {
        return None;
    }
    let parent = source_path.rsplit_once('/').map(|(parent, _)| parent);
    let joined = normalize_link_path(parent, target)?;
    if paths.contains(&joined) {
        return Some(joined);
    }
    if paths.contains(target) {
        return Some(target.to_owned());
    }
    if !target.contains('/')
        && let Some(candidates) = basenames.get(target)
        && candidates.len() == 1
    {
        return candidates.first().cloned();
    }
    None
}

fn normalize_link_path(parent: Option<&str>, target: &str) -> Option<String> {
    let combined = parent
        .map(|parent| format!("{parent}/{target}"))
        .unwrap_or_else(|| target.to_owned());
    let mut parts = Vec::new();
    for part in combined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            value => parts.push(value),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn context_excerpt(text: &str, offset: usize) -> String {
    let start = text[..offset.min(text.len())]
        .rfind('\n')
        .map_or(0, |value| value + 1);
    let end = text[offset.min(text.len())..]
        .find('\n')
        .map_or(text.len(), |value| offset.min(text.len()) + value);
    text[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_CONTEXT_CHARS)
        .collect()
}

fn load_portable_export_metadata(root: &Path) -> Result<BTreeMap<String, PortableExportEntry>> {
    let Some(parent) = root.parent() else {
        return Ok(BTreeMap::new());
    };
    if root.file_name().and_then(|value| value.to_str()) != Some("workspace") {
        return Ok(BTreeMap::new());
    }
    let manifest_path = parent.join("manifest.json");
    if !manifest_path.is_file() {
        return Ok(BTreeMap::new());
    }
    let bytes = std::fs::read(&manifest_path)?;
    verify_manifest_checksum(parent, &bytes)?;
    let manifest: PortableExportManifest =
        serde_json::from_slice(&bytes).context("invalid workspace export manifest")?;
    if manifest.format != EXPORT_MANIFEST_FORMAT {
        return Ok(BTreeMap::new());
    }
    let mut entries = BTreeMap::new();
    for entry in manifest.entries {
        validate_relative_path(&entry.path)?;
        if entry
            .archive_path
            .as_deref()
            .is_some_and(|path| path != format!("workspace/{}", entry.path))
        {
            continue;
        }
        if entries.insert(entry.path.clone(), entry).is_some() {
            bail!("workspace export manifest contains a duplicate path");
        }
    }
    Ok(entries)
}

fn verify_manifest_checksum(root: &Path, manifest: &[u8]) -> Result<()> {
    let checksums = std::fs::read_to_string(root.join("CHECKSUMS.sha256"))
        .context("workspace export lacks CHECKSUMS.sha256")?;
    let expected = checksums
        .lines()
        .filter_map(|line| line.split_once("  "))
        .find_map(|(hash, path)| (path == "manifest.json").then_some(hash))
        .ok_or_else(|| anyhow!("CHECKSUMS.sha256 omits manifest.json"))?;
    if expected.len() != 64
        || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
        || expected.to_ascii_lowercase() != hash_bytes(manifest)
    {
        bail!("workspace export manifest does not match CHECKSUMS.sha256");
    }
    Ok(())
}

fn resolve_export_workspace_root(root: &Path) -> Result<PathBuf> {
    let workspace = root.join("workspace");
    if root.join("manifest.json").is_file() && workspace.is_dir() {
        reject_symlink_root(&workspace)
    } else {
        Ok(root.to_owned())
    }
}

fn reject_symlink_root(path: &Path) -> Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect import root {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "import root must not be a symbolic link: {}",
            path.display()
        );
    }
    if !metadata.is_dir() {
        bail!("import root is not a directory: {}", path.display());
    }
    path.canonicalize()
        .with_context(|| format!("could not resolve import root {}", path.display()))
}

fn ensure_auxiliary_path_outside_root(
    root: &Path,
    candidate: Option<&Path>,
    name: &str,
) -> Result<()> {
    let Some(candidate) = candidate else {
        return Ok(());
    };
    let absolute = if candidate.is_absolute() {
        candidate.to_owned()
    } else {
        env::current_dir()?.join(candidate)
    };
    if absolute.starts_with(root) {
        bail!("{name} must be outside the imported root");
    }
    Ok(())
}

fn build_globs(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).with_context(|| format!("invalid glob {pattern:?}"))?);
    }
    Ok(Some(builder.build()?))
}

fn relative_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root)?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .ok_or_else(|| anyhow!("workspace paths must be UTF-8"))?,
            ),
            _ => bail!("unsafe local path {}", path.display()),
        }
    }
    let result = parts.join("/");
    validate_relative_path(&result)?;
    Ok(result)
}

fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 1_024
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

fn validate_media_type(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value.chars().any(char::is_control)
        || !value.contains('/')
    {
        bail!("invalid media type: {value:?}");
    }
    Ok(())
}

fn safe_source_path(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative_path(relative)?;
    let path = root.join(relative.split('/').collect::<PathBuf>());
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("workspace source is no longer a regular file: {relative}");
    }
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root) {
        bail!("workspace source escaped its root: {relative}");
    }
    Ok(canonical)
}

fn collision_key(path: &str) -> String {
    path.nfc().collect::<String>().to_lowercase()
}

fn tier_a_metadata(entry: &InventoryEntry) -> Option<&Value> {
    entry
        .source_metadata
        .get("_straylight_tier_a")
        .filter(|value| {
            value.get("format").and_then(Value::as_str) == Some(TIER_A_PORTABLE_COMPANION_FORMAT)
        })
}

fn is_binary_companion_entry(entry: &InventoryEntry) -> bool {
    (entry.path.starts_with(".straylight/binaries/") && entry.path.ends_with(".md"))
        || is_tier_a_portable_companion_entry(entry)
}

fn is_tier_a_portable_companion_entry(entry: &InventoryEntry) -> bool {
    entry.source_metadata.get("kind").and_then(Value::as_str) == Some("binary_description")
        && tier_a_metadata(entry).is_some()
}

fn portable_companion_entry<'a>(
    binary: &InventoryEntry,
    entries: &'a [InventoryEntry],
) -> Result<Option<&'a InventoryEntry>> {
    if binary.kind != EntryKind::Binary {
        return Ok(None);
    }
    let Some(metadata) = tier_a_metadata(binary) else {
        return Ok(None);
    };
    let Some(path) = metadata
        .get("binary_description_path")
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    validate_relative_path(path)?;
    if binary.size_bytes > MAX_MULTIPART_BINARY_BYTES {
        bail!(
            "{} requires an exact portable companion, but paired binary imports \
             currently support at most {} bytes",
            binary.path,
            MAX_MULTIPART_BINARY_BYTES
        );
    }
    let expected_hash = metadata
        .get("binary_description_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{} omits binary_description_hash", binary.path))?;
    let normalized_hash = strip_sha256(expected_hash);
    if normalized_hash.len() != 64 || !normalized_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("{} has an invalid binary_description_hash", binary.path);
    }
    let companion = entries
        .iter()
        .find(|entry| collision_key(&entry.path) == collision_key(path))
        .ok_or_else(|| anyhow!("{} omits exact companion {path}", binary.path))?;
    if companion.path != path
        || companion.kind != EntryKind::Markdown
        || companion.size_bytes > 256_000
        || strip_sha256(&companion.content_hash) != strip_sha256(expected_hash)
        || companion
            .source_metadata
            .get("kind")
            .and_then(Value::as_str)
            != Some("binary_description")
        || companion
            .source_metadata
            .get("binary_path")
            .and_then(Value::as_str)
            != Some(binary.path.as_str())
        || tier_a_metadata(companion).is_none()
    {
        bail!(
            "{} does not have a valid one-to-one exact portable companion",
            binary.path
        );
    }
    Ok(Some(companion))
}

fn tier_a_binary_for_companion<'a>(
    companion: &InventoryEntry,
    entries: &'a [InventoryEntry],
) -> Result<Option<&'a InventoryEntry>> {
    if !is_tier_a_portable_companion_entry(companion) {
        return Ok(None);
    }
    let binary_path = companion
        .source_metadata
        .get("binary_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{} omits its binary_path", companion.path))?;
    validate_relative_path(binary_path)?;
    let binary = entries
        .iter()
        .find(|entry| collision_key(&entry.path) == collision_key(binary_path))
        .ok_or_else(|| anyhow!("{} omits paired binary {binary_path}", companion.path))?;
    if binary.path != binary_path || binary.kind != EntryKind::Binary {
        bail!("{} does not point to an exact binary entry", companion.path);
    }
    let paired = portable_companion_entry(binary, entries)?
        .ok_or_else(|| anyhow!("{} is not claimed by its paired binary", companion.path))?;
    if paired.path != companion.path {
        bail!(
            "{} is not the one-to-one companion claimed by {}",
            companion.path,
            binary.path
        );
    }
    Ok(Some(binary))
}

fn required_str<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("workspace response lacks {field}"))
}

fn required_i64(value: &Value, field: &str) -> Result<i64> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("workspace response lacks {field}"))
}

fn required_u64(value: &Value, field: &str) -> Result<u64> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("workspace response lacks {field}"))
}

fn hash_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_json_replace(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".carrystate-state-{}.tmp", Uuid::now_v7()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn default_state_dir() -> PathBuf {
    env::var_os("CARRYSTATE_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/state/carrystate/workspace-imports"))
        })
        .unwrap_or_else(|| env::temp_dir().join("carrystate-workspace-imports"))
}

struct StateLock {
    _file: File,
}

impl StateLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&path)?;
        file.try_lock_exclusive()
            .with_context(|| format!("another import is using {}", path.display()))?;
        file.set_len(0)?;
        writeln!(file, "pid={}", std::process::id())?;
        file.flush()?;
        Ok(Self { _file: file })
    }
}

fn system_time_ns(value: Option<SystemTime>) -> Option<u64> {
    let duration = value?.duration_since(UNIX_EPOCH).ok()?;
    duration
        .as_secs()
        .checked_mul(1_000_000_000)?
        .checked_add(u64::from(duration.subsec_nanos()))
}

#[cfg(unix)]
fn file_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.mode() & 0o777)
}

#[cfg(not(unix))]
fn file_mode(_: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn file_device_id(metadata: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.dev())
}

#[cfg(not(unix))]
fn file_device_id(_: &std::fs::Metadata) -> Option<u64> {
    None
}

#[cfg(unix)]
fn file_inode(metadata: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino())
}

#[cfg(not(unix))]
fn file_inode(_: &std::fs::Metadata) -> Option<u64> {
    None
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

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn args(root: PathBuf) -> ImportArgs {
        ImportArgs {
            root,
            describe_binaries: true,
            include: Vec::new(),
            exclude: Vec::new(),
            manifest_out: None,
            state_dir: None,
            dry_run: true,
        }
    }

    fn history_metadata(target: i64, semantics: &str) -> Value {
        json!({
            "legacy": {
                "lineage_ordinal": target,
                "asset_ref": format!("asset:synthetic-{target}")
            },
            "_straylight_tier_a_history": {
                "format": TIER_A_HISTORY_STAGE_FORMAT,
                "target_lineage_ordinal": target,
                "semantics": semantics
            },
            "portable": {
                "modified_unix_ns": null,
                "mode": null
            }
        })
    }

    fn history_entry(target: i64, semantics: &str) -> InventoryEntry {
        InventoryEntry {
            path: "Notes/repeated.md".to_owned(),
            kind: EntryKind::Markdown,
            content_hash: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 7,
            media_type: "text/markdown".to_owned(),
            portable: PortableMetadata::default(),
            attachment_context: Vec::new(),
            source_metadata: history_metadata(target, semantics),
            observed: ObservedFile::default(),
        }
    }

    fn conversation_entry(id: Uuid, parent: Option<Uuid>) -> InventoryEntry {
        InventoryEntry {
            path: messaging_protocol::conversation_path(id),
            kind: EntryKind::Markdown,
            content_hash: format!("sha256:{}", "a".repeat(64)),
            size_bytes: MAX_TEXT_BYTES + 1,
            media_type: "text/markdown".to_owned(),
            portable: PortableMetadata::default(),
            attachment_context: Vec::new(),
            source_metadata: json!({
                "kind": "conversation",
                "schema": "conversation.v1",
                "conversation": {
                    "id": id,
                    "continues_from": parent
                }
            }),
            observed: ObservedFile::default(),
        }
    }

    fn remote_history_entry(version: i64, target: i64, semantics: &str) -> RemoteEntry {
        RemoteEntry {
            entry_ref: format!("entry:{}", Uuid::now_v7()),
            path: "Notes/repeated.md".to_owned(),
            kind: "markdown".to_owned(),
            version,
            content_hash: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 7,
            metadata: history_metadata(target, semantics),
        }
    }

    fn tier_a_companion_pair(target: i64, semantics: &str) -> (InventoryEntry, InventoryEntry) {
        let description_hash = format!("sha256:{}", "a".repeat(64));
        let portable = PortableMetadata {
            modified_unix_ns: Some(1_700_000_000_000_000_000),
            mode: Some(0o600),
        };
        let binary = InventoryEntry {
            path: "sources/receipt.png".to_owned(),
            kind: EntryKind::Binary,
            content_hash: format!("sha256:{}", "b".repeat(64)),
            size_bytes: 10,
            media_type: "image/png".to_owned(),
            portable: portable.clone(),
            attachment_context: vec![],
            source_metadata: json!({
                "_straylight_tier_a": {
                    "format": TIER_A_PORTABLE_COMPANION_FORMAT,
                    "binary_description_path": "workspace/assets/receipt.png.md",
                    "binary_description_hash": description_hash,
                    "copy_method": "exact_legacy_bytes"
                }
            }),
            observed: ObservedFile::default(),
        };
        let mut source_metadata = history_metadata(target, semantics);
        let values = source_metadata
            .as_object_mut()
            .expect("history metadata is an object");
        values.insert("kind".to_owned(), json!("binary_description"));
        values.insert("binary_path".to_owned(), json!(binary.path));
        values.insert("description_status".to_owned(), json!("complete"));
        values.insert(
            "_straylight_tier_a".to_owned(),
            json!({
                "format": TIER_A_PORTABLE_COMPANION_FORMAT,
                "copy_method": "exact_legacy_bytes"
            }),
        );
        let companion = InventoryEntry {
            path: "workspace/assets/receipt.png.md".to_owned(),
            kind: EntryKind::Markdown,
            content_hash: description_hash,
            size_bytes: 20,
            media_type: "text/markdown".to_owned(),
            portable,
            attachment_context: vec![],
            source_metadata,
            observed: ObservedFile::default(),
        };
        (binary, companion)
    }

    fn normalized_companion_remote(
        binary: &InventoryEntry,
        companion: &InventoryEntry,
        version: i64,
    ) -> RemoteEntry {
        RemoteEntry {
            entry_ref: format!("entry:{}", Uuid::now_v7()),
            path: companion.path.clone(),
            kind: "markdown".to_owned(),
            version,
            content_hash: companion.content_hash.clone(),
            size_bytes: companion.size_bytes,
            metadata: json!({
                "_straylight_import": {
                    "format": IMPORT_MANIFEST_FORMAT,
                    "portable_companion_format": TIER_A_PORTABLE_COMPANION_FORMAT,
                    "content_sha256": companion.content_hash,
                },
                "binary_path": binary.path,
                "content_hash": binary.content_hash,
                "description_status": "byte_copied",
                "kind": "binary_description",
                "portable": companion.portable,
            }),
        }
    }

    #[test]
    fn inventory_is_sorted_and_exact() {
        let temporary = tempdir().unwrap();
        std::fs::write(temporary.path().join("b.txt"), b"beta\r\n").unwrap();
        std::fs::write(temporary.path().join("a.md"), b"# Alpha\n").unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let manifest = inventory(&root, &args(root.clone())).unwrap();
        assert_eq!(
            manifest
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["a.md", "b.txt"]
        );
        assert_eq!(manifest.entries[0].kind, EntryKind::Markdown);
        assert_eq!(
            manifest.entries[1].content_hash,
            format!("sha256:{}", hash_bytes(b"beta\r\n"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn inventory_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let temporary = tempdir().unwrap();
        std::fs::write(temporary.path().join("real.md"), b"real").unwrap();
        symlink("real.md", temporary.path().join("alias.md")).unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let manifest = inventory(&root, &args(root.clone())).unwrap();
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.ignored[0].path, "alias.md");
        assert_eq!(manifest.ignored[0].reason, "symbolic_link_not_followed");
    }

    #[test]
    fn utf8_text_and_binary_are_classified_without_extension_guessing() {
        assert_eq!(
            classify_file(Path::new("data.sql"), Some(b"select * from facts;\n")),
            EntryKind::Markdown
        );
        assert_eq!(
            classify_file(Path::new("receipt.bin"), Some(&[0, 1, 2, 3])),
            EntryKind::Binary
        );
    }

    #[test]
    fn attachment_context_is_bounded_and_targets_binary_files() {
        let observed = ObservedFile::default();
        let mut entries = vec![
            InventoryEntry {
                path: "Trips/plan.md".to_owned(),
                kind: EntryKind::Markdown,
                content_hash: "sha256:00".to_owned(),
                size_bytes: 1,
                media_type: "text/markdown".to_owned(),
                portable: PortableMetadata::default(),
                attachment_context: vec![],
                source_metadata: Value::Null,
                observed: observed.clone(),
            },
            InventoryEntry {
                path: "Trips/receipt.jpg".to_owned(),
                kind: EntryKind::Binary,
                content_hash: "sha256:11".to_owned(),
                size_bytes: 1,
                media_type: "image/jpeg".to_owned(),
                portable: PortableMetadata::default(),
                attachment_context: vec![],
                source_metadata: Value::Null,
                observed,
            },
        ];
        attach_context(
            &mut entries,
            &[(
                "Trips/plan.md".to_owned(),
                "Expense proof: ![[receipt.jpg|meal receipt]]".to_owned(),
            )],
        );
        assert_eq!(entries[1].attachment_context.len(), 1);
        assert!(
            entries[1].attachment_context[0]
                .excerpt
                .contains("Expense proof")
        );
    }

    #[test]
    fn attachment_context_retains_only_the_bounded_best_effort_set() {
        let observed = ObservedFile::default();
        let mut entries = vec![
            InventoryEntry {
                path: "receipt.jpg".to_owned(),
                kind: EntryKind::Binary,
                content_hash: "sha256:11".to_owned(),
                size_bytes: 1,
                media_type: "image/jpeg".to_owned(),
                portable: PortableMetadata::default(),
                attachment_context: vec![],
                source_metadata: Value::Null,
                observed: observed.clone(),
            },
            InventoryEntry {
                path: "plan.md".to_owned(),
                kind: EntryKind::Markdown,
                content_hash: "sha256:22".to_owned(),
                size_bytes: 1,
                media_type: "text/markdown".to_owned(),
                portable: PortableMetadata::default(),
                attachment_context: vec![],
                source_metadata: Value::Null,
                observed,
            },
        ];
        let sources = (0..(MAX_CONTEXTS_PER_BINARY + 5))
            .map(|index| {
                (
                    format!("source-{index}.md"),
                    format!("Context {index}: [receipt](receipt.jpg)"),
                )
            })
            .collect::<Vec<_>>();
        attach_context(&mut entries, &sources);

        assert_eq!(entries[0].attachment_context.len(), MAX_CONTEXTS_PER_BINARY);
        assert!(
            entries[0]
                .attachment_context
                .iter()
                .all(|context| context.excerpt.chars().count() <= MAX_CONTEXT_CHARS)
        );
    }

    #[test]
    fn import_phases_keep_generated_binary_companions_last() {
        let observed = ObservedFile::default();
        let entry = |path: &str, kind| InventoryEntry {
            path: path.to_owned(),
            kind,
            content_hash: "sha256:11".to_owned(),
            size_bytes: 1,
            media_type: "application/octet-stream".to_owned(),
            portable: PortableMetadata::default(),
            attachment_context: vec![],
            source_metadata: Value::Null,
            observed: observed.clone(),
        };
        let markdown = entry("plan.md", EntryKind::Markdown);
        let binary = entry("receipt.jpg", EntryKind::Binary);
        let companion = entry(".straylight/binaries/receipt.md", EntryKind::Markdown);

        assert!(ImportPhase::Markdown.includes(&markdown));
        assert!(ImportPhase::Binary.includes(&binary));
        assert!(ImportPhase::BinaryCompanion.includes(&companion));
        assert!(!ImportPhase::Markdown.includes(&companion));
    }

    #[test]
    fn managed_conversations_import_parent_first_without_reordering_inventory() {
        let child_id = Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
        let parent_id = Uuid::parse_str("ffffffff-ffff-4fff-bfff-ffffffffffff").unwrap();
        let child = conversation_entry(child_id, Some(parent_id));
        let parent = conversation_entry(parent_id, None);
        let ordinary = InventoryEntry {
            path: "Notes/ordinary.md".to_owned(),
            kind: EntryKind::Markdown,
            content_hash: format!("sha256:{}", "b".repeat(64)),
            size_bytes: 10,
            media_type: "text/markdown".to_owned(),
            portable: PortableMetadata::default(),
            attachment_context: Vec::new(),
            source_metadata: Value::Null,
            observed: ObservedFile::default(),
        };
        let entries = vec![child, ordinary, parent];

        let managed = import_phase_entries(&entries, ImportPhase::ManagedConversation).unwrap();
        assert_eq!(
            managed
                .iter()
                .map(|entry| managed_conversation_id(&entry.path, &entry.source_metadata).unwrap())
                .collect::<Vec<_>>(),
            [parent_id, child_id]
        );
        assert_eq!(
            import_phase_entries(&entries, ImportPhase::Markdown)
                .unwrap()
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["Notes/ordinary.md"]
        );
        assert_eq!(ImportPhase::ManagedConversation.concurrency(), 1);
        assert!(ImportPhase::ManagedConversation.forces_upload());
        assert!(!ImportPhase::Markdown.forces_upload());

        let matching_parent = RemoteEntry {
            entry_ref: format!("entry:{parent_id}"),
            path: entries[2].path.clone(),
            kind: "markdown".to_owned(),
            version: 1,
            content_hash: entries[2].content_hash.clone(),
            size_bytes: entries[2].size_bytes,
            metadata: json!({
                "portable": {
                    "modified_unix_ns": null,
                    "mode": null
                }
            }),
        };
        let decision = plan_import_entry(&entries[2], Some(&matching_parent), true, None).unwrap();
        assert!(!decision.upload);
        assert!(decision.upload || ImportPhase::ManagedConversation.forces_upload());
    }

    #[test]
    fn tier_a_companions_are_one_to_one_exact_and_run_after_binary_upload() {
        let observed = ObservedFile::default();
        let description_hash = format!("sha256:{}", "a".repeat(64));
        let binary = InventoryEntry {
            path: "sources/receipt.png".to_owned(),
            kind: EntryKind::Binary,
            content_hash: format!("sha256:{}", "b".repeat(64)),
            size_bytes: 10,
            media_type: "image/png".to_owned(),
            portable: PortableMetadata::default(),
            attachment_context: vec![],
            source_metadata: json!({
                "_straylight_tier_a": {
                    "format": TIER_A_PORTABLE_COMPANION_FORMAT,
                    "binary_description_path": "workspace/assets/receipt.png.md",
                    "binary_description_hash": description_hash
                }
            }),
            observed: observed.clone(),
        };
        let companion = InventoryEntry {
            path: "workspace/assets/receipt.png.md".to_owned(),
            kind: EntryKind::Markdown,
            content_hash: description_hash,
            size_bytes: 20,
            media_type: "text/markdown".to_owned(),
            portable: PortableMetadata::default(),
            attachment_context: vec![],
            source_metadata: json!({
                "kind": "binary_description",
                "binary_path": "sources/receipt.png",
                "_straylight_tier_a": {
                    "format": TIER_A_PORTABLE_COMPANION_FORMAT,
                    "copy_method": "exact_legacy_bytes"
                }
            }),
            observed,
        };
        let entries = vec![binary, companion];

        assert!(ImportPhase::Binary.includes(&entries[0]));
        assert!(ImportPhase::BinaryCompanion.includes(&entries[1]));
        assert!(!ImportPhase::Markdown.includes(&entries[1]));
        assert_eq!(
            portable_companion_entry(&entries[0], &entries)
                .unwrap()
                .unwrap()
                .path,
            "workspace/assets/receipt.png.md"
        );

        let mut mismatched = entries;
        mismatched[1].content_hash = format!("sha256:{}", "c".repeat(64));
        assert!(portable_companion_entry(&mismatched[0], &mismatched).is_err());
    }

    #[test]
    fn fetched_normalized_tier_a_companion_reconciles_and_final_verifies() {
        let (binary, companion) = tier_a_companion_pair(1, TIER_A_ORDINARY_HISTORY_SEMANTICS);
        let local = vec![binary, companion];
        let paired_binary = tier_a_binary_for_companion(&local[1], &local)
            .unwrap()
            .unwrap();
        let remote_companion = normalized_companion_remote(paired_binary, &local[1], 1);
        assert_eq!(
            normalized_portable_companion_metadata(paired_binary, &local[1]),
            remote_companion.metadata
        );

        assert_eq!(
            plan_import_entry(
                &local[1],
                Some(&remote_companion),
                true,
                Some(paired_binary),
            )
            .unwrap(),
            ImportDecision {
                upload: false,
                expected_version: 1,
            }
        );
        verify_upload_receipt(&local[1], &remote_companion, Some(paired_binary)).unwrap();

        let remote_binary = RemoteEntry {
            entry_ref: format!("entry:{}", Uuid::now_v7()),
            path: local[0].path.clone(),
            kind: "binary".to_owned(),
            version: 1,
            content_hash: local[0].content_hash.clone(),
            size_bytes: local[0].size_bytes,
            metadata: json!({
                "companion_path": local[1].path,
                "portable": local[0].portable,
            }),
        };
        let remote = BTreeMap::from([
            (collision_key(&remote_binary.path), remote_binary),
            (collision_key(&remote_companion.path), remote_companion),
        ]);
        verify_complete_import(&local, &remote).unwrap();
    }

    #[test]
    fn normalized_tier_a_companion_mismatches_fail_closed() {
        let (binary, companion) = tier_a_companion_pair(1, TIER_A_ORDINARY_HISTORY_SEMANTICS);
        let rejected = |remote: &RemoteEntry| {
            assert!(plan_import_entry(&companion, Some(remote), true, Some(&binary)).is_err());
            assert!(verify_upload_receipt(&companion, remote, Some(&binary)).is_err());
        };

        let mut wrong_binary_path = normalized_companion_remote(&binary, &companion, 1);
        wrong_binary_path.metadata["binary_path"] = json!("sources/other.png");
        rejected(&wrong_binary_path);

        let mut wrong_status = normalized_companion_remote(&binary, &companion, 1);
        wrong_status.metadata["description_status"] = json!("provided");
        rejected(&wrong_status);

        let mut wrong_binary_hash = normalized_companion_remote(&binary, &companion, 1);
        wrong_binary_hash.metadata["content_hash"] = json!(format!("sha256:{}", "c".repeat(64)));
        rejected(&wrong_binary_hash);

        let mut wrong_companion_hash = normalized_companion_remote(&binary, &companion, 1);
        wrong_companion_hash.metadata["_straylight_import"]["content_sha256"] =
            json!(format!("sha256:{}", "d".repeat(64)));
        rejected(&wrong_companion_hash);

        let mut wrong_portable = normalized_companion_remote(&binary, &companion, 1);
        wrong_portable.metadata["portable"]["mode"] = json!(0o644);
        rejected(&wrong_portable);

        let wrong_version = normalized_companion_remote(&binary, &companion, 2);
        rejected(&wrong_version);
    }

    #[test]
    fn changed_tier_a_companion_must_advance_atomically_with_binary() {
        let (binary, companion) = tier_a_companion_pair(2, TIER_A_ORDINARY_HISTORY_SEMANTICS);
        let predecessor = RemoteEntry {
            version: 1,
            content_hash: format!("sha256:{}", "e".repeat(64)),
            size_bytes: companion.size_bytes + 1,
            ..normalized_companion_remote(&binary, &companion, 1)
        };
        let error = plan_import_entry(&companion, Some(&predecessor), true, Some(&binary))
            .unwrap_err()
            .to_string();
        assert!(error.contains("advanced atomically with its binary"));

        let atomic_target = normalized_companion_remote(&binary, &companion, 2);
        assert_eq!(
            plan_import_entry(&companion, Some(&atomic_target), true, Some(&binary)).unwrap(),
            ImportDecision {
                upload: false,
                expected_version: 2,
            }
        );
    }

    #[test]
    fn exact_history_binary_companions_are_unsupported() {
        let (binary, companion) = tier_a_companion_pair(2, TIER_A_EXACT_HISTORY_SEMANTICS);
        let target = normalized_companion_remote(&binary, &companion, 2);

        assert!(plan_import_entry(&companion, Some(&target), true, Some(&binary)).is_err());
        assert!(verify_upload_receipt(&companion, &target, Some(&binary)).is_err());
    }

    #[test]
    fn tier_a_exact_history_forces_only_the_immediate_next_ordinal() {
        let entry = history_entry(2, TIER_A_EXACT_HISTORY_SEMANTICS);
        let predecessor = remote_history_entry(1, 1, TIER_A_ORDINARY_HISTORY_SEMANTICS);
        let decision = plan_import_entry(&entry, Some(&predecessor), true, None).unwrap();

        assert_eq!(
            decision,
            ImportDecision {
                upload: true,
                expected_version: 1
            }
        );
    }

    #[test]
    fn tier_a_exact_history_retry_is_idempotent_only_at_matching_target() {
        let entry = history_entry(2, TIER_A_EXACT_HISTORY_SEMANTICS);
        let target = remote_history_entry(2, 2, TIER_A_EXACT_HISTORY_SEMANTICS);
        let decision = plan_import_entry(&entry, Some(&target), true, None).unwrap();

        assert_eq!(
            decision,
            ImportDecision {
                upload: false,
                expected_version: 2
            }
        );

        let mut pre_protocol_target = target;
        pre_protocol_target
            .metadata
            .as_object_mut()
            .unwrap()
            .remove("_straylight_tier_a_history");
        assert_eq!(
            plan_import_entry(&entry, Some(&pre_protocol_target), true, None).unwrap(),
            ImportDecision {
                upload: false,
                expected_version: 2
            }
        );

        let mismatched_target = remote_history_entry(2, 2, TIER_A_ORDINARY_HISTORY_SEMANTICS);
        assert!(plan_import_entry(&entry, Some(&mismatched_target), true, None).is_err());

        let mut mismatched_legacy = remote_history_entry(2, 2, TIER_A_EXACT_HISTORY_SEMANTICS);
        mismatched_legacy.metadata["legacy"]["asset_ref"] = json!("asset:different");
        assert!(plan_import_entry(&entry, Some(&mismatched_legacy), true, None).is_err());

        let mut normalized_without_legacy =
            remote_history_entry(2, 2, TIER_A_EXACT_HISTORY_SEMANTICS);
        normalized_without_legacy.metadata = json!({
            "_straylight_import": {"format": IMPORT_MANIFEST_FORMAT},
            "portable": entry.portable,
        });
        assert!(plan_import_entry(&entry, Some(&normalized_without_legacy), true, None).is_err());
    }

    #[test]
    fn tier_a_history_gap_and_ahead_states_fail_closed() {
        let entry = history_entry(2, TIER_A_EXACT_HISTORY_SEMANTICS);
        assert!(plan_import_entry(&entry, None, true, None).is_err());

        let ahead = remote_history_entry(3, 3, TIER_A_EXACT_HISTORY_SEMANTICS);
        assert!(plan_import_entry(&entry, Some(&ahead), true, None).is_err());
    }

    #[test]
    fn repeated_bytes_require_exact_history_semantics_and_markdown() {
        let ordinary = history_entry(2, TIER_A_ORDINARY_HISTORY_SEMANTICS);
        let predecessor = remote_history_entry(1, 1, TIER_A_ORDINARY_HISTORY_SEMANTICS);
        assert!(plan_import_entry(&ordinary, Some(&predecessor), true, None).is_err());

        let mut binary = history_entry(2, TIER_A_EXACT_HISTORY_SEMANTICS);
        binary.kind = EntryKind::Binary;
        assert!(plan_import_entry(&binary, Some(&predecessor), true, None).is_err());
    }

    #[test]
    fn traversal_and_normalized_collisions_are_rejected() {
        assert!(validate_relative_path("../escape").is_err());
        assert_eq!(collision_key("Cafe\u{301}.md"), collision_key("Café.md"));
    }
}
