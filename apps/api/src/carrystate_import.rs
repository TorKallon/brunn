use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{File, OpenOptions as StdOpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{ArgAction, Args};
use fs2::FileExt;
use globset::{Glob, GlobSet, GlobSetBuilder};
use reqwest::{Body, multipart};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{io::AsyncReadExt, time::sleep};
use tokio_util::io::ReaderStream;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::carrystate_cli::{ApiClient, strip_sha256, unwrap_data, value_str};

const MANIFEST_FORMAT: &str = "carrystate-vault-import-manifest@v1";
const STATE_FORMAT: &str = "carrystate-vault-import-state@v2";
const MAX_CONTEXT_MARKDOWN_BYTES: u64 = 256 * 1024;
const MAX_CONTEXT_LINK_TARGET_BYTES: usize = 1_024;
const MAX_CONTEXT_LINKS_PER_MARKDOWN: usize = 256;
const MAX_CONTEXTS_PER_TARGET: usize = 8;
const MAX_CONTEXT_EXCERPT_CHARS: usize = 800;
const MAX_CONTEXT_TOTAL_CHARS_PER_TARGET: usize = 4_000;
const MAX_PORTABLE_EXPORT_MANIFEST_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PORTABLE_EXPORT_ENTRIES: usize = 200_000;
const MAX_PORTABLE_EXPORT_CHECKSUM_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Args)]
pub struct ImportArgs {
    #[arg(long)]
    pub root: PathBuf,
    #[arg(long)]
    pub scope: String,
    #[arg(long)]
    pub vault_id: Option<String>,
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
    #[arg(long, default_value_t = 200)]
    pub batch_files: usize,
    #[arg(long, default_value_t = 48 * 1024 * 1024)]
    pub batch_bytes: u64,
    #[arg(long, default_value_t = 900)]
    pub processing_timeout_seconds: u64,
    #[arg(long)]
    pub mirror: bool,
    #[arg(long, requires = "mirror")]
    pub confirm_mirror: Option<String>,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultManifest {
    pub format: String,
    pub vault_id: String,
    pub scope: String,
    pub root: String,
    pub describe_binaries: bool,
    pub entries: Vec<InventoryEntry>,
    pub ignored: Vec<IgnoredEntry>,
    pub totals: InventoryTotals,
    pub manifest_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryEntry {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub media_type: String,
    pub modified_unix_ns: Option<u64>,
    pub mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_modified_unix_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_mode: Option<u32>,
    pub device_id: Option<u64>,
    pub inode: Option<u64>,
    pub description_expected: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_context: Vec<AttachmentContext>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttachmentContext {
    pub source_markdown_path: String,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoredEntry {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InventoryTotals {
    pub files: usize,
    pub bytes: u64,
    pub ignored: usize,
    pub descriptions_expected: usize,
}

#[derive(Debug, Clone)]
struct PortableExportMetadata {
    sha256: String,
    size_bytes: u64,
    media_type: String,
    modified_unix_ns: Option<u64>,
    mode: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImportState {
    format: String,
    vault_id: String,
    scope: String,
    #[serde(default)]
    mirror: bool,
    manifest: VaultManifest,
    batches: Vec<BatchState>,
    completed_at_unix_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchState {
    index: usize,
    batch_hash: String,
    paths: Vec<String>,
    #[serde(default)]
    upload_ref: Option<String>,
    stage_ref: Option<String>,
    import_receipt: Option<String>,
    corpus_revision: Option<String>,
    status: BatchStatus,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BatchStatus {
    Pending,
    Staged,
    Complete,
}

pub async fn run(client: Option<&ApiClient>, args: ImportArgs) -> Result<()> {
    if args.batch_files == 0 || args.batch_files > 1_000 {
        bail!("batch_files must be between 1 and 1,000");
    }
    if args.batch_bytes == 0 || args.batch_bytes > 64 * 1024 * 1024 {
        bail!("batch_bytes must be between 1 byte and 64 MiB");
    }
    let root = args
        .root
        .canonicalize()
        .with_context(|| format!("could not resolve vault root {}", args.root.display()))?;
    if !root.is_dir() {
        bail!("vault root is not a directory: {}", root.display());
    }
    let vault_id = args
        .vault_id
        .clone()
        .unwrap_or_else(|| derived_vault_id(&root));
    validate_vault_id(&vault_id)?;
    let manifest = inventory(&root, &vault_id, &args)?;
    if let Some(path) = &args.manifest_out {
        write_json_atomic(path, &manifest)?;
    }
    if args.dry_run {
        let batches = build_batches(&manifest.entries, args.batch_files, args.batch_bytes)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "dry_run",
                "manifest": manifest,
                "batch_count": batches.len(),
                "authoritative_write_performed": false
            }))?
        );
        return Ok(());
    }
    let client = client.ok_or_else(|| anyhow!("API client is required for import"))?;
    let state_dir = args.state_dir.clone().unwrap_or_else(default_state_dir);
    std::fs::create_dir_all(&state_dir)?;
    set_private_directory_permissions(&state_dir)?;
    let state_key = hex::encode(Sha256::digest(
        format!("{}\0{}\0{}", vault_id, args.scope, root.display()).as_bytes(),
    ));
    let state_path = state_dir.join(format!("{}.json", &state_key[..32]));
    let _lock = StateLock::acquire(state_dir.join(format!("{}.lock", &state_key[..32])))?;
    let previous = read_state(&state_path)?;
    let mut removed_paths = removed_paths(previous.as_ref(), &manifest);
    if args.mirror {
        let server_paths = current_vault_assets(client, &manifest, &args.scope)
            .await?
            .into_keys()
            .collect::<std::collections::HashSet<_>>();
        let local_paths = manifest
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<std::collections::HashSet<_>>();
        removed_paths = server_paths
            .into_iter()
            .filter(|path| !local_paths.contains(path.as_str()))
            .collect();
        removed_paths.sort();
    }
    if args.mirror && !removed_paths.is_empty() {
        let preview_hash = mirror_preview_hash(&manifest, &removed_paths)?;
        if args.confirm_mirror.as_deref().map(strip_sha256) != Some(preview_hash.as_str()) {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "status": "mirror_preview",
                    "authoritative_write_performed": false,
                    "vault_id": vault_id,
                    "scope": args.scope,
                    "removed_paths": removed_paths,
                    "removal_count": removed_paths.len(),
                    "confirm_mirror": format!("sha256:{preview_hash}")
                }))?
            );
            return Ok(());
        }
    }
    if args.mirror && manifest.entries.is_empty() {
        let (stage_ref, import_receipt, corpus_revision) =
            process_empty_mirror(client, &args, &manifest).await?;
        let state = ImportState {
            format: STATE_FORMAT.to_owned(),
            vault_id: manifest.vault_id.clone(),
            scope: manifest.scope.clone(),
            mirror: true,
            manifest: manifest.clone(),
            batches: vec![BatchState {
                index: 0,
                batch_hash: manifest.manifest_sha256.clone(),
                paths: Vec::new(),
                upload_ref: None,
                stage_ref: Some(stage_ref),
                import_receipt,
                corpus_revision,
                status: BatchStatus::Complete,
            }],
            completed_at_unix_seconds: Some(unix_seconds()),
        };
        write_json_atomic(&state_path, &state)?;
        let verification = verify_import(client, &manifest, &args.scope, true).await?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "complete",
                "authoritative_write_performed": true,
                "vault_id": vault_id,
                "scope": args.scope,
                "manifest_hash": format!("sha256:{}", manifest.manifest_sha256),
                "files": 0,
                "bytes": 0,
                "batches": 1,
                "descriptions_expected": 0,
                "verification": verification,
                "mirror": true,
                "removed_paths": removed_paths,
                "state_path": state_path
            }))?
        );
        return Ok(());
    }
    let mut state = prepare_state(
        previous.as_ref(),
        manifest.clone(),
        args.batch_files,
        args.batch_bytes,
        args.mirror,
    )?;
    if state.completed_at_unix_seconds.is_some()
        && state
            .batches
            .iter()
            .all(|batch| batch.status == BatchStatus::Complete)
    {
        let verification = verify_import(client, &manifest, &args.scope, args.mirror).await?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "unchanged",
                "authoritative_write_performed": false,
                "vault_id": vault_id,
                "scope": args.scope,
                "manifest_hash": format!("sha256:{}", manifest.manifest_sha256),
                "files": manifest.totals.files,
                "bytes": manifest.totals.bytes,
                "batches": state.batches.len(),
                "descriptions_expected": manifest.totals.descriptions_expected,
                "verification": verification,
                "mirror": args.mirror,
                "removed_paths": removed_paths,
                "state_path": state_path
            }))?
        );
        return Ok(());
    }
    if state.batches.is_empty() {
        state.completed_at_unix_seconds = Some(unix_seconds());
        write_json_atomic(&state_path, &state)?;
        let verification = verify_import(client, &manifest, &args.scope, args.mirror).await?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "unchanged",
                "authoritative_write_performed": false,
                "vault_id": vault_id,
                "scope": args.scope,
                "manifest_hash": format!("sha256:{}", manifest.manifest_sha256),
                "files": manifest.totals.files,
                "bytes": manifest.totals.bytes,
                "batches": 0,
                "descriptions_expected": manifest.totals.descriptions_expected,
                "verification": verification,
                "mirror": args.mirror,
                "removed_paths": removed_paths,
                "state_path": state_path
            }))?
        );
        return Ok(());
    }
    write_json_atomic(&state_path, &state)?;

    for batch_index in 0..state.batches.len() {
        if state.batches[batch_index].status == BatchStatus::Complete {
            continue;
        }
        process_batch(client, &root, &args, &mut state, batch_index, &state_path).await?;
    }
    commit_generation(client, &args, &mut state, &state_path).await?;
    state.completed_at_unix_seconds = Some(unix_seconds());
    write_json_atomic(&state_path, &state)?;
    let verification = verify_import(client, &manifest, &args.scope, args.mirror).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "complete",
            "authoritative_write_performed": true,
            "vault_id": vault_id,
            "scope": args.scope,
            "manifest_hash": format!("sha256:{}", manifest.manifest_sha256),
            "files": manifest.totals.files,
            "bytes": manifest.totals.bytes,
            "batches": state.batches.len(),
            "descriptions_expected": manifest.totals.descriptions_expected,
            "verification": verification,
            "mirror": args.mirror,
            "removed_paths": removed_paths,
            "state_path": state_path
        }))?
    );
    Ok(())
}

async fn process_empty_mirror(
    client: &ApiClient,
    args: &ImportArgs,
    manifest: &VaultManifest,
) -> Result<(String, Option<String>, Option<String>)> {
    let sentinel_path = ".carrystate/internal/empty-snapshot";
    let form = multipart::Form::new()
        .text("scope", args.scope.clone())
        .text("stable_import_id", format!("vault:{}", manifest.vault_id))
        .text("describe_binaries", "false")
        .text("path", sentinel_path)
        .text("metadata", "{}")
        .part(
            "file",
            multipart::Part::bytes(Vec::new())
                .file_name("empty-snapshot")
                .mime_str("application/octet-stream")?,
        );
    let staged = client.post_multipart("/v1/memory/stage", form).await?;
    let stage_ref = value_str(unwrap_data(&staged), "stage_ref")?.to_owned();
    let status = poll_stage(
        client,
        &stage_ref,
        Duration::from_secs(args.processing_timeout_seconds),
    )
    .await?;
    let sentinel_present = status
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|entry| entry.get("path").and_then(Value::as_str) == Some(sentinel_path));
    if !sentinel_present {
        bail!("empty mirror stage lost its internal snapshot marker");
    }
    let response = client
        .post_json(
            "/v1/memory/save",
            &json!({
                "intent": "vault_import_empty_mirror",
                "scope": args.scope,
                "root_refs": [],
                "source_refs": [],
                "base_corpus_revision": status.get("base_corpus_revision"),
                "idempotency_key": format!(
                    "vault-empty-mirror:{}:{}",
                    short_hash(&manifest.vault_id),
                    &manifest.manifest_sha256[..16]
                ),
                "items": [{
                    "action": "create",
                    "kind": "import_receipt",
                    "payload": {
                        "stage_ref": stage_ref,
                        "selected_entries": [sentinel_path],
                        "snapshot_paths": [],
                        "stable_import_id": format!("vault:{}", manifest.vault_id),
                        "generation_id": format!(
                            "vault-generation:{}:{}",
                            short_hash(&manifest.vault_id),
                            &manifest.manifest_sha256[..32]
                        )
                    }
                }]
            }),
        )
        .await?;
    let data = unwrap_data(&response);
    Ok((
        stage_ref,
        data.get("import_receipt")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        data.get("corpus_revision")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    ))
}

fn inventory(root: &Path, vault_id: &str, args: &ImportArgs) -> Result<VaultManifest> {
    let includes = build_globs(&args.include)?;
    let excludes = build_globs(&args.exclude)?;
    let portable_export_metadata = load_portable_export_metadata(root)?;
    let mut entries = Vec::new();
    let mut markdown_sources = Vec::new();
    let mut ignored = Vec::new();
    let mut collision_keys = BTreeMap::<String, String>::new();
    for item in WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let item = item?;
        if item.path() == root || item.file_type().is_dir() {
            continue;
        }
        let relative = relative_path(root, item.path())?;
        if is_generated_description_path(&relative) {
            ignored.push(IgnoredEntry {
                path: relative,
                reason: "system_generated_derivative".to_owned(),
            });
            continue;
        }
        if item.file_type().is_symlink() {
            ignored.push(IgnoredEntry {
                path: relative,
                reason: "symbolic_link".to_owned(),
            });
            continue;
        }
        if !item.file_type().is_file() {
            ignored.push(IgnoredEntry {
                path: relative,
                reason: "not_a_regular_file".to_owned(),
            });
            continue;
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
        let collision_key = relative.nfc().collect::<String>().to_lowercase();
        if let Some(prior) = collision_keys.insert(collision_key, relative.clone()) {
            bail!(
                "portable path collision between {prior:?} and {relative:?}; case and Unicode-normalized paths must be unique"
            );
        }
        let metadata = item.metadata()?;
        let (sha256, size_bytes, prefix) = inspect_file(item.path(), &metadata)?;
        let detected_media_type = detect_media_type(item.path(), &prefix);
        let export_metadata = portable_export_metadata
            .get(&relative)
            .filter(|entry| entry.sha256 == sha256 && entry.size_bytes == size_bytes);
        let observed_modified_unix_ns = metadata.modified().ok().and_then(system_time_ns);
        let observed_mode = file_mode(&metadata);
        let media_type = export_metadata
            .map(|entry| entry.media_type.clone())
            .unwrap_or(detected_media_type);
        let modified_unix_ns = export_metadata
            .and_then(|entry| entry.modified_unix_ns)
            .or(observed_modified_unix_ns);
        let mode = export_metadata
            .and_then(|entry| entry.mode)
            .or(observed_mode);
        let description_expected = args.describe_binaries
            && (size_bytes > args.batch_bytes || server_marks_unsupported(&relative, &prefix));
        if should_scan_markdown_context(&relative, size_bytes)
            && let Ok(text) = String::from_utf8(prefix)
        {
            markdown_sources.push(MarkdownSource {
                path: relative.clone(),
                text,
            });
        }
        entries.push(InventoryEntry {
            path: relative,
            size_bytes,
            sha256,
            media_type,
            modified_unix_ns,
            mode,
            observed_modified_unix_ns,
            observed_mode,
            device_id: file_device_id(&metadata),
            inode: file_inode(&metadata),
            description_expected,
            attachment_context: Vec::new(),
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    markdown_sources.sort_by(|left, right| left.path.cmp(&right.path));
    attach_markdown_context(&mut entries, &markdown_sources);
    ignored.sort_by(|left, right| left.path.cmp(&right.path));
    let totals = InventoryTotals {
        files: entries.len(),
        bytes: entries.iter().map(|entry| entry.size_bytes).sum(),
        ignored: ignored.len(),
        descriptions_expected: entries
            .iter()
            .filter(|entry| entry.description_expected)
            .count(),
    };
    let identity_entries = entries
        .iter()
        .map(portable_manifest_entry)
        .collect::<Vec<_>>();
    let identity = json!({
        "format": MANIFEST_FORMAT,
        "vault_id": vault_id,
        "scope": args.scope,
        "describe_binaries": args.describe_binaries,
        "entries": identity_entries,
        "ignored": ignored
    });
    let manifest_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(&identity)?));
    Ok(VaultManifest {
        format: MANIFEST_FORMAT.to_owned(),
        vault_id: vault_id.to_owned(),
        scope: args.scope.clone(),
        root: root.display().to_string(),
        describe_binaries: args.describe_binaries,
        entries,
        ignored,
        totals,
        manifest_sha256,
    })
}

fn load_portable_export_metadata(root: &Path) -> Result<HashMap<String, PortableExportMetadata>> {
    if root.file_name().and_then(|name| name.to_str()) != Some("sources") {
        return Ok(HashMap::new());
    }
    let Some(parent) = root.parent() else {
        return Ok(HashMap::new());
    };
    let manifest_path = parent.join("manifest.json");
    let Ok(metadata) = std::fs::symlink_metadata(&manifest_path) else {
        return Ok(HashMap::new());
    };
    if !metadata.file_type().is_file() {
        return Ok(HashMap::new());
    }
    if metadata.len() > MAX_PORTABLE_EXPORT_MANIFEST_BYTES {
        bail!(
            "CarryState export manifest exceeds {} bytes",
            MAX_PORTABLE_EXPORT_MANIFEST_BYTES
        );
    }
    let bytes = std::fs::read(&manifest_path)?;
    let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(HashMap::new());
    };
    if manifest.get("format").and_then(Value::as_str) != Some("carrystate-portable-vault@v1") {
        return Ok(HashMap::new());
    }
    verify_portable_manifest_checksum(parent, &bytes)?;
    let entries = manifest
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("CarryState export manifest lacks entries"))?;
    if entries.len() > MAX_PORTABLE_EXPORT_ENTRIES {
        bail!(
            "CarryState export manifest exceeds {} entries",
            MAX_PORTABLE_EXPORT_ENTRIES
        );
    }
    let mut by_path = HashMap::new();
    for entry in entries {
        let Some(export_path) = entry.get("path").and_then(Value::as_str) else {
            bail!("CarryState export manifest entry lacks path");
        };
        let Some(path) = export_path.strip_prefix("sources/") else {
            continue;
        };
        validate_relative_path(path)?;
        let content_hash = entry
            .get("content_hash")
            .and_then(Value::as_str)
            .map(strip_sha256)
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| anyhow!("CarryState export entry {path} lacks a SHA-256 hash"))?
            .to_ascii_lowercase();
        let size_bytes = entry
            .get("size_bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("CarryState export entry {path} lacks size_bytes"))?;
        let media_type = entry
            .get("media_type")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 255)
            .ok_or_else(|| anyhow!("CarryState export entry {path} lacks media_type"))?
            .to_owned();
        let modified_unix_ns = entry.get("modified_unix_ns").and_then(Value::as_u64);
        let mode = entry
            .get("mode")
            .and_then(Value::as_u64)
            .map(u32::try_from)
            .transpose()
            .map_err(|_| anyhow!("CarryState export entry {path} has an invalid mode"))?;
        if mode.is_some_and(|value| value > 0o777) {
            bail!("CarryState export entry {path} has non-portable mode bits");
        }
        if by_path
            .insert(
                path.to_owned(),
                PortableExportMetadata {
                    sha256: content_hash,
                    size_bytes,
                    media_type,
                    modified_unix_ns,
                    mode,
                },
            )
            .is_some()
        {
            bail!("CarryState export manifest contains duplicate source path {path}");
        }
    }
    Ok(by_path)
}

fn verify_portable_manifest_checksum(root: &Path, manifest_bytes: &[u8]) -> Result<()> {
    let checksum_path = root.join("CHECKSUMS.sha256");
    let metadata = std::fs::symlink_metadata(&checksum_path)
        .context("CarryState export lacks CHECKSUMS.sha256")?;
    if !metadata.file_type().is_file() {
        bail!("CarryState export CHECKSUMS.sha256 is not a regular file");
    }
    if metadata.len() > MAX_PORTABLE_EXPORT_CHECKSUM_BYTES {
        bail!(
            "CarryState export checksum ledger exceeds {} bytes",
            MAX_PORTABLE_EXPORT_CHECKSUM_BYTES
        );
    }
    let checksum_bytes = std::fs::read(&checksum_path)?;
    let checksum_text = std::str::from_utf8(&checksum_bytes)
        .context("CarryState export checksum ledger is not UTF-8")?;
    let mut expected = None;
    for (line_number, line) in checksum_text.lines().enumerate() {
        let Some((digest, path)) = line.split_once("  ") else {
            bail!(
                "CarryState export checksum ledger has an invalid line {}",
                line_number + 1
            );
        };
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!(
                "CarryState export checksum ledger has an invalid digest on line {}",
                line_number + 1
            );
        }
        validate_relative_path(path)?;
        if path == "manifest.json" && expected.replace(digest.to_ascii_lowercase()).is_some() {
            bail!("CarryState export checksum ledger repeats manifest.json");
        }
    }
    let expected =
        expected.ok_or_else(|| anyhow!("CarryState export checksum ledger omits manifest.json"))?;
    let observed = hex::encode(Sha256::digest(manifest_bytes));
    if observed != expected {
        bail!("CarryState export manifest does not match CHECKSUMS.sha256");
    }
    Ok(())
}

fn portable_manifest_entry(entry: &InventoryEntry) -> Value {
    json!({
        "path": entry.path,
        "size_bytes": entry.size_bytes,
        "sha256": entry.sha256,
        "media_type": entry.media_type,
        "modified_unix_ns": entry.modified_unix_ns,
        "mode": entry.mode,
        "description_expected": entry.description_expected,
        "attachment_context": entry.attachment_context
    })
}

#[derive(Debug)]
struct MarkdownSource {
    path: String,
    text: String,
}

#[derive(Clone, Copy, Debug)]
enum LocalLinkSyntax {
    Obsidian,
    Markdown,
}

#[derive(Debug)]
struct LocalLinkOccurrence {
    target: String,
    byte_offset: usize,
    syntax: LocalLinkSyntax,
}

struct LocalPathResolver {
    paths: BTreeSet<String>,
    basenames: BTreeMap<String, Vec<String>>,
}

impl LocalPathResolver {
    fn new(entries: &[InventoryEntry]) -> Self {
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
        Self { paths, basenames }
    }

    fn resolve(
        &self,
        source_markdown_path: &str,
        raw_target: &str,
        syntax: LocalLinkSyntax,
    ) -> Option<String> {
        let target = sanitize_local_link_target(raw_target, syntax)?;
        let source_parent = source_markdown_path
            .rsplit_once('/')
            .map(|(parent, _)| parent);
        let mut candidates = BTreeSet::new();
        match syntax {
            LocalLinkSyntax::Markdown => {
                let relative = join_local_path(source_parent, &target)?;
                self.add_existing_variants(&relative, false, &mut candidates);
            }
            LocalLinkSyntax::Obsidian => {
                let relative = join_local_path(source_parent, &target)?;
                self.add_existing_variants(&relative, true, &mut candidates);
                self.add_existing_variants(&target, true, &mut candidates);
                if !target.contains('/') {
                    self.add_basename_variants(&target, &mut candidates);
                }
            }
        }
        (candidates.len() == 1).then(|| candidates.into_iter().next().unwrap())
    }

    fn add_existing_variants(
        &self,
        candidate: &str,
        allow_markdown_extension: bool,
        output: &mut BTreeSet<String>,
    ) {
        if self.paths.contains(candidate) {
            output.insert(candidate.to_owned());
        }
        if allow_markdown_extension && Path::new(candidate).extension().is_none() {
            let markdown = format!("{candidate}.md");
            if self.paths.contains(&markdown) {
                output.insert(markdown);
            }
        }
    }

    fn add_basename_variants(&self, target: &str, output: &mut BTreeSet<String>) {
        if let Some(paths) = self.basenames.get(target) {
            output.extend(paths.iter().cloned());
        }
        if Path::new(target).extension().is_none()
            && let Some(paths) = self.basenames.get(&format!("{target}.md"))
        {
            output.extend(paths.iter().cloned());
        }
    }
}

fn attach_markdown_context(entries: &mut [InventoryEntry], sources: &[MarkdownSource]) {
    let resolver = LocalPathResolver::new(entries);
    let mut contexts = BTreeMap::<String, Vec<AttachmentContext>>::new();
    let mut context_chars = BTreeMap::<String, usize>::new();
    let mut seen = BTreeSet::<(String, String, String)>::new();

    for source in sources {
        for occurrence in markdown_link_occurrences(&source.text) {
            let Some(target) =
                resolver.resolve(&source.path, &occurrence.target, occurrence.syntax)
            else {
                continue;
            };
            if target == source.path {
                continue;
            }
            if contexts
                .get(&target)
                .is_some_and(|values| values.len() >= MAX_CONTEXTS_PER_TARGET)
                || context_chars
                    .get(&target)
                    .is_some_and(|used| *used >= MAX_CONTEXT_TOTAL_CHARS_PER_TARGET)
            {
                continue;
            }
            let excerpt = bounded_context_excerpt(&source.text, occurrence.byte_offset);
            if excerpt.is_empty()
                || !seen.insert((target.clone(), source.path.clone(), excerpt.clone()))
            {
                continue;
            }
            let target_contexts = contexts.entry(target.clone()).or_default();
            if target_contexts.len() >= MAX_CONTEXTS_PER_TARGET {
                continue;
            }
            let used = context_chars.entry(target).or_default();
            let remaining = MAX_CONTEXT_TOTAL_CHARS_PER_TARGET.saturating_sub(*used);
            if remaining == 0 {
                continue;
            }
            let excerpt = truncate_chars(&excerpt, remaining.min(MAX_CONTEXT_EXCERPT_CHARS));
            if excerpt.is_empty() {
                continue;
            }
            *used += excerpt.chars().count();
            target_contexts.push(AttachmentContext {
                source_markdown_path: source.path.clone(),
                excerpt,
            });
        }
    }

    for entry in entries {
        if let Some(mut values) = contexts.remove(&entry.path) {
            values.sort_by(|left, right| {
                (&left.source_markdown_path, &left.excerpt)
                    .cmp(&(&right.source_markdown_path, &right.excerpt))
            });
            entry.attachment_context = values;
        }
    }
}

fn markdown_link_occurrences(text: &str) -> Vec<LocalLinkOccurrence> {
    let mut output = Vec::new();
    let mut fenced = false;
    let mut line_offset = 0;
    for line_with_ending in text.split_inclusive('\n') {
        let line = line_with_ending.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        } else if !fenced {
            parse_markdown_line_links(line, line_offset, &mut output);
        }
        if output.len() >= MAX_CONTEXT_LINKS_PER_MARKDOWN {
            break;
        }
        line_offset += line_with_ending.len();
    }
    output
}

fn parse_markdown_line_links(
    line: &str,
    line_offset: usize,
    output: &mut Vec<LocalLinkOccurrence>,
) {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut inline_code = false;
    while index < bytes.len() && output.len() < MAX_CONTEXT_LINKS_PER_MARKDOWN {
        if bytes[index] == b'`' && !is_markdown_escaped(bytes, index) {
            inline_code = !inline_code;
            index += 1;
            continue;
        }
        if inline_code || bytes[index] != b'[' || is_markdown_escaped(bytes, index) {
            index += 1;
            continue;
        }
        if bytes.get(index + 1) == Some(&b'[') {
            let content_start = index + 2;
            let search_end = bytes
                .len()
                .min(content_start + MAX_CONTEXT_LINK_TARGET_BYTES + 2);
            if let Some(relative_end) = bytes[content_start..search_end]
                .windows(2)
                .position(|pair| pair == b"]]")
            {
                let content_end = content_start + relative_end;
                output.push(LocalLinkOccurrence {
                    target: line[content_start..content_end].to_owned(),
                    byte_offset: line_offset + index,
                    syntax: LocalLinkSyntax::Obsidian,
                });
                index = content_end + 2;
                continue;
            }
        } else {
            let label_search_end = bytes.len().min(index + 2 + 512);
            if let Some(label_end_relative) = bytes[index + 1..label_search_end]
                .iter()
                .position(|byte| *byte == b']')
            {
                let label_end = index + 1 + label_end_relative;
                if bytes.get(label_end + 1) == Some(&b'(') {
                    let target_start = label_end + 2;
                    let target_search_end = bytes
                        .len()
                        .min(target_start + MAX_CONTEXT_LINK_TARGET_BYTES + 1);
                    if let Some(target_end_relative) = bytes[target_start..target_search_end]
                        .iter()
                        .position(|byte| *byte == b')')
                    {
                        let target_end = target_start + target_end_relative;
                        output.push(LocalLinkOccurrence {
                            target: line[target_start..target_end].to_owned(),
                            byte_offset: line_offset + index,
                            syntax: LocalLinkSyntax::Markdown,
                        });
                        index = target_end + 1;
                        continue;
                    }
                }
            }
        }
        index += 1;
    }
}

fn is_markdown_escaped(bytes: &[u8], index: usize) -> bool {
    let mut backslashes = 0;
    let mut cursor = index;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        backslashes += 1;
        cursor -= 1;
    }
    backslashes % 2 == 1
}

fn sanitize_local_link_target(raw: &str, syntax: LocalLinkSyntax) -> Option<String> {
    let mut target = raw.trim();
    if matches!(syntax, LocalLinkSyntax::Obsidian) {
        target = target.split('|').next()?.trim();
    }
    if target.starts_with('<') && target.ends_with('>') {
        target = target[1..target.len() - 1].trim();
    }
    if target.is_empty()
        || target.len() > MAX_CONTEXT_LINK_TARGET_BYTES
        || target.starts_with('/')
        || target.starts_with('~')
        || target.contains('\\')
        || target.contains('\0')
        || target.contains('#')
        || target.contains('?')
        || target.contains(':')
        || target.starts_with('^')
    {
        return None;
    }
    let mut parts = Vec::new();
    for part in target.split('/') {
        match part {
            "" | ".." => return None,
            "." => {}
            value => parts.push(value),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn join_local_path(parent: Option<&str>, target: &str) -> Option<String> {
    let joined = match parent {
        Some(parent) if !parent.is_empty() => format!("{parent}/{target}"),
        _ => target.to_owned(),
    };
    validate_relative_path(&joined).ok()?;
    Some(joined)
}

fn bounded_context_excerpt(text: &str, byte_offset: usize) -> String {
    let byte_offset = byte_offset.min(text.len());
    let paragraph_start = text[..byte_offset]
        .rfind("\n\n")
        .map_or(0, |index| index + 2);
    let paragraph_end = text[byte_offset..]
        .find("\n\n")
        .map_or(text.len(), |index| byte_offset + index);
    let paragraph = text[paragraph_start..paragraph_end].trim();
    if paragraph.is_empty() {
        return String::new();
    }
    let relative_offset = byte_offset
        .saturating_sub(paragraph_start)
        .min(paragraph.len());
    let cleaned = paragraph
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect::<String>();
    if cleaned.chars().count() <= MAX_CONTEXT_EXCERPT_CHARS {
        return cleaned;
    }
    let center = paragraph[..relative_offset].chars().count();
    let start = center.saturating_sub(MAX_CONTEXT_EXCERPT_CHARS / 2);
    cleaned
        .chars()
        .skip(start)
        .take(MAX_CONTEXT_EXCERPT_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect::<String>()
}

fn is_markdown_path(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".md")
}

fn should_scan_markdown_context(path: &str, size_bytes: u64) -> bool {
    is_markdown_path(path) && size_bytes <= MAX_CONTEXT_MARKDOWN_BYTES
}

fn prepare_state(
    previous: Option<&ImportState>,
    manifest: VaultManifest,
    batch_files: usize,
    batch_bytes: u64,
    mirror: bool,
) -> Result<ImportState> {
    if let Some(previous) = previous
        && previous.format == STATE_FORMAT
        && previous.vault_id == manifest.vault_id
        && previous.scope == manifest.scope
        && previous.mirror == mirror
        && previous.manifest.manifest_sha256 == manifest.manifest_sha256
    {
        return Ok(previous.clone());
    }
    let previous_entries: HashMap<&str, &InventoryEntry> = previous
        .filter(|state| state.completed_at_unix_seconds.is_some())
        .map(|state| {
            state
                .manifest
                .entries
                .iter()
                .map(|entry| (entry.path.as_str(), entry))
                .collect()
        })
        .unwrap_or_default();
    let mut changed: Vec<InventoryEntry> = manifest
        .entries
        .iter()
        .filter(|entry| {
            previous_entries
                .get(entry.path.as_str())
                .is_none_or(|previous| {
                    previous.sha256 != entry.sha256
                        || previous.size_bytes != entry.size_bytes
                        || previous.media_type != entry.media_type
                        || previous.modified_unix_ns != entry.modified_unix_ns
                        || previous.mode != entry.mode
                        || previous.description_expected != entry.description_expected
                        || previous.attachment_context != entry.attachment_context
                })
        })
        .cloned()
        .collect();
    if changed.is_empty() && mirror {
        let anchor = manifest
            .entries
            .iter()
            .min_by_key(|entry| (entry.description_expected, entry.size_bytes, &entry.path))
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "an empty vault snapshot cannot be finalized through the bounded stage contract"
                )
            })?;
        changed.push(anchor);
    }
    let batches = build_batches(&changed, batch_files, batch_bytes)?
        .into_iter()
        .enumerate()
        .map(|(index, entries)| BatchState {
            index,
            batch_hash: batch_hash(&entries),
            paths: entries.into_iter().map(|entry| entry.path).collect(),
            upload_ref: None,
            stage_ref: None,
            import_receipt: None,
            corpus_revision: None,
            status: BatchStatus::Pending,
        })
        .collect();
    Ok(ImportState {
        format: STATE_FORMAT.to_owned(),
        vault_id: manifest.vault_id.clone(),
        scope: manifest.scope.clone(),
        mirror,
        manifest,
        batches,
        completed_at_unix_seconds: None,
    })
}

fn build_batches(
    entries: &[InventoryEntry],
    max_files: usize,
    max_bytes: u64,
) -> Result<Vec<Vec<InventoryEntry>>> {
    let mut batches = Vec::new();
    let mut batch = Vec::new();
    let mut batch_bytes = 0_u64;
    for entry in entries {
        if entry.size_bytes > max_bytes {
            if !batch.is_empty() {
                batches.push(std::mem::take(&mut batch));
                batch_bytes = 0;
            }
            batches.push(vec![entry.clone()]);
            continue;
        }
        if !batch.is_empty()
            && (batch.len() >= max_files
                || batch_bytes.saturating_add(entry.size_bytes) > max_bytes)
        {
            batches.push(std::mem::take(&mut batch));
            batch_bytes = 0;
        }
        batch_bytes = batch_bytes.saturating_add(entry.size_bytes);
        batch.push(entry.clone());
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    Ok(batches)
}

async fn process_batch(
    client: &ApiClient,
    root: &Path,
    args: &ImportArgs,
    state: &mut ImportState,
    batch_index: usize,
    state_path: &Path,
) -> Result<()> {
    let paths = state.batches[batch_index].paths.clone();
    let entry_map: HashMap<&str, &InventoryEntry> = state
        .manifest
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let entries = paths
        .iter()
        .map(|path| {
            entry_map
                .get(path.as_str())
                .map(|entry| (*entry).clone())
                .ok_or_else(|| anyhow!("import state refers to missing manifest path {path}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let stable_import_id = format!(
        "vault-batch:{}:{}:{}",
        short_hash(&state.vault_id),
        &state.manifest.manifest_sha256[..16],
        &state.batches[batch_index].batch_hash[..16]
    );
    let stage_ref = if let Some(stage_ref) = &state.batches[batch_index].stage_ref {
        stage_ref.clone()
    } else if entries.len() == 1 && entries[0].size_bytes > args.batch_bytes {
        let stage = process_resumable_entry(
            client,
            root,
            args,
            state,
            batch_index,
            &entries[0],
            &stable_import_id,
            state_path,
        )
        .await?;
        let stage_ref = value_str(unwrap_data(&stage), "stage_ref")?.to_owned();
        state.batches[batch_index].stage_ref = Some(stage_ref.clone());
        state.batches[batch_index].status = BatchStatus::Staged;
        write_json_atomic(state_path, state)?;
        stage_ref
    } else {
        let mut form = multipart::Form::new()
            .text("scope", args.scope.clone())
            .text("stable_import_id", stable_import_id.clone())
            .text("describe_binaries", args.describe_binaries.to_string());
        for entry in &entries {
            let source_path = root.join(path_to_platform(&entry.path)?);
            let expected_metadata = std::fs::symlink_metadata(&source_path)?;
            ensure_inventory_entry_unchanged(&source_path, entry, &expected_metadata)?;
            let file = open_inventory_file(&source_path)?;
            let opened_metadata = file.metadata()?;
            ensure_same_regular_file(&source_path, &expected_metadata, &opened_metadata)?;
            ensure_inventory_entry_unchanged(&source_path, entry, &opened_metadata)?;
            let file = tokio::fs::File::from_std(file);
            let stream = ReaderStream::new(file);
            let body = Body::wrap_stream(stream);
            let filename = Path::new(&entry.path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("upload.bin")
                .to_owned();
            let part = multipart::Part::stream_with_length(body, entry.size_bytes)
                .file_name(filename)
                .mime_str(&entry.media_type)?;
            form = form
                .text("path", entry.path.clone())
                .text(
                    "metadata",
                    serde_json::to_string(&json!({
                        "modified_unix_ns": entry.modified_unix_ns,
                        "mode": entry.mode,
                        "attachment_context": entry.attachment_context
                    }))?,
                )
                .part("file", part);
        }
        let response = client.post_multipart("/v1/memory/stage", form).await?;
        let data = unwrap_data(&response);
        let stage_ref = value_str(data, "stage_ref")?.to_owned();
        state.batches[batch_index].stage_ref = Some(stage_ref.clone());
        state.batches[batch_index].status = BatchStatus::Staged;
        write_json_atomic(state_path, state)?;
        stage_ref
    };
    let status = poll_stage(
        client,
        &stage_ref,
        Duration::from_secs(args.processing_timeout_seconds),
    )
    .await?;
    verify_staged_entries(&status, &entries)?;
    let staged_paths: BTreeMap<&str, &Value> = status
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get("path")
                .and_then(Value::as_str)
                .map(|path| (path, entry))
        })
        .collect();
    for entry in &entries {
        if entry.description_expected {
            let description = description_path(&entry.path);
            if !staged_paths.contains_key(description.as_str()) {
                bail!(
                    "stage {} reached ready without the expected description for {}",
                    stage_ref,
                    entry.path
                );
            }
        }
    }
    state.batches[batch_index].status = BatchStatus::Staged;
    write_json_atomic(state_path, state)?;
    Ok(())
}

async fn commit_generation(
    client: &ApiClient,
    args: &ImportArgs,
    state: &mut ImportState,
    state_path: &Path,
) -> Result<()> {
    if state.batches.len() > 512 {
        bail!(
            "atomic import requires at most 512 upload batches; increase --batch-files or --batch-bytes"
        );
    }
    let canonical_import_id = format!("vault:{}", state.vault_id);
    let generation_id = format!(
        "vault-generation:{}:{}",
        short_hash(&state.vault_id),
        &state.manifest.manifest_sha256[..32]
    );
    let entry_map: HashMap<&str, &InventoryEntry> = state
        .manifest
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let mut base_revision = None::<String>;
    let mut items = Vec::with_capacity(state.batches.len());
    for (batch_index, batch) in state.batches.iter().enumerate() {
        let stage_ref = batch
            .stage_ref
            .as_deref()
            .ok_or_else(|| anyhow!("batch {batch_index} has not been staged"))?;
        let status = poll_stage(
            client,
            stage_ref,
            Duration::from_secs(args.processing_timeout_seconds),
        )
        .await?;
        let current_base = status
            .get("base_corpus_revision")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("stage {stage_ref} lacks base_corpus_revision"))?;
        if let Some(expected) = base_revision.as_deref() {
            if expected != current_base {
                bail!(
                    "vault changed while the import was staging; no files were promoted ({} vs {})",
                    expected,
                    current_base
                );
            }
        } else {
            base_revision = Some(current_base.to_owned());
        }
        let entries = batch
            .paths
            .iter()
            .map(|path| {
                entry_map
                    .get(path.as_str())
                    .map(|entry| (*entry).clone())
                    .ok_or_else(|| anyhow!("import state refers to missing manifest path {path}"))
            })
            .collect::<Result<Vec<_>>>()?;
        verify_staged_entries(&status, &entries)?;
        let staged_paths: BTreeSet<&str> = status
            .get("entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("path").and_then(Value::as_str))
            .collect();
        let mut selected_entries = batch.paths.clone();
        for entry in &entries {
            if entry.description_expected {
                let description = description_path(&entry.path);
                if !staged_paths.contains(description.as_str()) {
                    bail!(
                        "stage {stage_ref} reached ready without the expected description for {}",
                        entry.path
                    );
                }
                selected_entries.push(description);
            }
        }
        selected_entries.sort();
        let mut payload = json!({
            "stage_ref": stage_ref,
            "selected_entries": selected_entries,
            "stable_import_id": canonical_import_id.clone(),
            "generation_id": generation_id.clone()
        });
        if args.mirror && batch_index + 1 == state.batches.len() {
            payload["snapshot_paths"] = json!(complete_snapshot_paths(&state.manifest));
        }
        items.push(json!({
            "action": "create",
            "kind": "import_receipt",
            "payload": payload
        }));
    }
    let base_revision =
        base_revision.ok_or_else(|| anyhow!("atomic import generation contains no stages"))?;
    let response = client
        .post_json(
            "/v1/memory/save",
            &json!({
                "intent": "vault_import_atomic",
                "scope": args.scope,
                "root_refs": [],
                "source_refs": [],
                "base_corpus_revision": base_revision,
                "idempotency_key": format!(
                    "vault-generation:{}:{}",
                    short_hash(&state.vault_id),
                    &state.manifest.manifest_sha256[..32]
                ),
                "items": items
            }),
        )
        .await?;
    let data = unwrap_data(&response);
    let corpus_revision = data
        .get("corpus_revision")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("atomic import response lacks corpus_revision"))?
        .to_owned();
    let mut import_receipts = data
        .get("import_receipts")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if import_receipts.is_empty()
        && let Some(receipt) = data.get("import_receipt").and_then(Value::as_str)
    {
        import_receipts.push(receipt.to_owned());
    }
    if import_receipts.len() != state.batches.len() {
        bail!(
            "atomic import returned {} receipts for {} batches",
            import_receipts.len(),
            state.batches.len()
        );
    }
    for (batch, import_receipt) in state.batches.iter_mut().zip(import_receipts) {
        batch.import_receipt = Some(import_receipt);
        batch.corpus_revision = Some(corpus_revision.clone());
        batch.status = BatchStatus::Complete;
    }
    write_json_atomic(state_path, state)?;
    Ok(())
}

async fn process_resumable_entry(
    client: &ApiClient,
    root: &Path,
    args: &ImportArgs,
    state: &mut ImportState,
    batch_index: usize,
    entry: &InventoryEntry,
    stable_import_id: &str,
    state_path: &Path,
) -> Result<Value> {
    let upload_ref = if let Some(upload_ref) = &state.batches[batch_index].upload_ref {
        upload_ref.clone()
    } else {
        let created = client
            .post_json(
                "/v1/asset-uploads",
                &json!({
                    "scope": args.scope,
                    "path": entry.path,
                    "content_hash": format!("sha256:{}", entry.sha256),
                    "size_bytes": entry.size_bytes,
                    "media_type": entry.media_type,
                    "stable_import_id": stable_import_id,
                    "modified_unix_ns": entry.modified_unix_ns,
                    "mode": entry.mode,
                    "attachment_context": entry.attachment_context,
                    "describe_binary": args.describe_binaries
                }),
            )
            .await?;
        let upload_ref = value_str(unwrap_data(&created), "upload_ref")?.to_owned();
        state.batches[batch_index].upload_ref = Some(upload_ref.clone());
        write_json_atomic(state_path, state)?;
        upload_ref
    };
    let status = client
        .get_json(&format!("/v1/asset-uploads/{upload_ref}"), &[])
        .await?;
    let status_name = status
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("upload {upload_ref} response lacks status"))?;
    if matches!(status_name, "aborted" | "failed" | "expired") {
        bail!("upload {upload_ref} cannot resume from status {status_name}");
    }
    if status_name == "uploading" {
        let part_size = status
            .get("part_size_bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("upload {upload_ref} lacks part_size_bytes"))?;
        let uploaded: HashMap<u64, (&str, u64)> = status
            .get("uploaded_parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|part| {
                Some((
                    part.get("part_number")?.as_u64()?,
                    (
                        part.get("content_hash")?.as_str()?,
                        part.get("size_bytes")?.as_u64()?,
                    ),
                ))
            })
            .collect();
        let source_path = root.join(path_to_platform(&entry.path)?);
        let expected_metadata = std::fs::symlink_metadata(&source_path)?;
        ensure_inventory_entry_unchanged(&source_path, entry, &expected_metadata)?;
        let file = open_inventory_file(&source_path)?;
        let opened_metadata = file.metadata()?;
        ensure_same_regular_file(&source_path, &expected_metadata, &opened_metadata)?;
        ensure_inventory_entry_unchanged(&source_path, entry, &opened_metadata)?;
        let mut file = tokio::fs::File::from_std(file);
        let part_count = entry.size_bytes.div_ceil(part_size);
        for part_index in 0..part_count {
            let part_number = part_index + 1;
            let expected_size = if part_number == part_count {
                entry.size_bytes - part_size.saturating_mul(part_count.saturating_sub(1))
            } else {
                part_size
            };
            let mut buffer = vec![0_u8; usize::try_from(expected_size)?];
            file.read_exact(&mut buffer).await.with_context(|| {
                format!(
                    "{} changed or became unreadable during resumable upload",
                    source_path.display()
                )
            })?;
            let part_hash = hex::encode(Sha256::digest(&buffer));
            if let Some((prior_hash, prior_size)) = uploaded.get(&part_number) {
                if strip_sha256(prior_hash) != part_hash || *prior_size != expected_size {
                    bail!(
                        "resumable upload {upload_ref} part {part_number} conflicts with local bytes"
                    );
                }
                continue;
            }
            client
                .put_bytes(
                    &format!("/v1/asset-uploads/{upload_ref}/parts/{part_number}"),
                    &part_hash,
                    bytes::Bytes::from(buffer),
                )
                .await?;
        }
        let mut trailing = [0_u8; 1];
        if file.read(&mut trailing).await? != 0 {
            bail!("{} grew during resumable upload", source_path.display());
        }
    }
    client
        .post_json(
            &format!("/v1/asset-uploads/{upload_ref}/complete"),
            &json!({}),
        )
        .await
}

async fn poll_stage(client: &ApiClient, stage_ref: &str, timeout: Duration) -> Result<Value> {
    let started = std::time::Instant::now();
    loop {
        let status = client
            .get_json(&format!("/v1/stages/{stage_ref}"), &[])
            .await?;
        match status.get("status").and_then(Value::as_str) {
            Some("ready" | "promoted") => return Ok(status),
            Some("failed" | "expired") => {
                bail!("stage {stage_ref} ended in {}", status["status"]);
            }
            Some("inspecting") => {}
            Some(other) => bail!("stage {stage_ref} returned unknown status {other}"),
            None => bail!("stage {stage_ref} response lacks status"),
        }
        if started.elapsed() >= timeout {
            bail!("stage {stage_ref} did not become ready before the timeout");
        }
        sleep(Duration::from_secs(2)).await;
    }
}

fn verify_staged_entries(status: &Value, entries: &[InventoryEntry]) -> Result<()> {
    let staged: HashMap<&str, &Value> = status
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get("path")
                .and_then(Value::as_str)
                .map(|path| (path, entry))
        })
        .collect();
    for expected in entries {
        let actual = staged
            .get(expected.path.as_str())
            .ok_or_else(|| anyhow!("stage lost {}", expected.path))?;
        let hash = actual
            .get("content_hash")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("stage lacks hash for {}", expected.path))?;
        let size = actual
            .get("size_bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("stage lacks size for {}", expected.path))?;
        if strip_sha256(hash) != expected.sha256 || size != expected.size_bytes {
            bail!("stage integrity mismatch for {}", expected.path);
        }
    }
    Ok(())
}

#[derive(Debug)]
struct CurrentVaultAsset {
    sha256: String,
    size_bytes: u64,
    description_available: bool,
    description_status: Option<String>,
    description_method: Option<String>,
}

#[derive(Debug, Serialize)]
struct ImportVerification {
    files: usize,
    bytes: u64,
    descriptions_expected: usize,
    descriptions_available: usize,
    descriptions_complete: usize,
    descriptions_needing_review: usize,
    description_methods: BTreeMap<String, usize>,
    attention_required: bool,
}

async fn verify_import(
    client: &ApiClient,
    manifest: &VaultManifest,
    scope: &str,
    mirror: bool,
) -> Result<ImportVerification> {
    let assets = current_vault_assets(client, manifest, scope).await?;
    let expected_paths = manifest
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<BTreeSet<_>>();
    if mirror && assets.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected_paths {
        bail!("mirror verification found unexpected or missing current paths");
    }
    let mut verified_bytes = 0_u64;
    let mut verified_descriptions = 0_usize;
    let mut descriptions_complete = 0_usize;
    let mut descriptions_needing_review = 0_usize;
    let mut description_methods = BTreeMap::<String, usize>::new();
    for entry in &manifest.entries {
        let actual = assets
            .get(&entry.path)
            .ok_or_else(|| anyhow!("current corpus is missing imported asset {}", entry.path))?;
        if actual.sha256 != entry.sha256 || actual.size_bytes != entry.size_bytes {
            bail!("current corpus asset does not match {}", entry.path);
        }
        verified_bytes = verified_bytes.saturating_add(actual.size_bytes);
        if entry.description_expected {
            if !actual.description_available {
                bail!(
                    "current corpus is missing the generated description for {}",
                    entry.path
                );
            }
            verified_descriptions = verified_descriptions.saturating_add(1);
            if actual.description_status.as_deref() == Some("complete") {
                descriptions_complete = descriptions_complete.saturating_add(1);
            } else {
                descriptions_needing_review = descriptions_needing_review.saturating_add(1);
            }
            let method = actual
                .description_method
                .as_deref()
                .unwrap_or("unknown")
                .to_owned();
            *description_methods.entry(method).or_default() += 1;
        }
    }
    if verified_bytes != manifest.totals.bytes
        || verified_descriptions != manifest.totals.descriptions_expected
    {
        bail!("import verification totals do not match the inventory manifest");
    }
    Ok(ImportVerification {
        files: manifest.entries.len(),
        bytes: verified_bytes,
        descriptions_expected: manifest.totals.descriptions_expected,
        descriptions_available: verified_descriptions,
        descriptions_complete,
        descriptions_needing_review,
        description_methods,
        attention_required: descriptions_needing_review > 0,
    })
}

async fn current_vault_assets(
    client: &ApiClient,
    manifest: &VaultManifest,
    scope: &str,
) -> Result<HashMap<String, CurrentVaultAsset>> {
    let opened = client
        .post_json(
            "/v1/memory/open",
            &json!({
                "task": format!("Verify vault import {}", manifest.vault_id),
                "hints": {"authorization_scope": scope},
                "mode": "exploration",
                "token_budget": 1024
            }),
        )
        .await?;
    let session_id = opened
        .get("session_id")
        .and_then(Value::as_str)
        .or_else(|| {
            unwrap_data(&opened)
                .get("session_id")
                .and_then(Value::as_str)
        })
        .ok_or_else(|| anyhow!("verification session did not open"))?;
    let stable_import_id = format!("vault:{}", manifest.vault_id);
    let response = client
        .get_json(
            "/v1/vault/manifest",
            &[
                ("session_id", session_id),
                ("stable_import_id", stable_import_id.as_str()),
                ("include_history", "false"),
                ("include_scope_memory", "false"),
            ],
        )
        .await?;
    current_vault_assets_from_manifest(&response)
}

fn current_vault_assets_from_manifest(
    response: &Value,
) -> Result<HashMap<String, CurrentVaultAsset>> {
    let entries = unwrap_data(response)
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("vault manifest response lacks entries"))?;
    let mut descriptions = HashMap::<String, (Option<String>, Option<String>)>::new();
    for entry in entries {
        if entry.get("source_kind").and_then(Value::as_str) != Some("derived_asset_description")
            || entry.get("active").and_then(Value::as_bool) != Some(true)
        {
            continue;
        }
        let original_path = entry
            .get("original_path")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("generated description lacks original_path"))?;
        let description = (
            entry
                .get("description_status")
                .and_then(Value::as_str)
                .map(str::to_owned),
            entry
                .get("description_method")
                .and_then(Value::as_str)
                .map(str::to_owned),
        );
        if descriptions
            .insert(original_path.to_owned(), description)
            .is_some()
        {
            bail!("current corpus contains duplicate description for {original_path}");
        }
    }

    let mut assets = HashMap::<String, CurrentVaultAsset>::new();
    for entry in entries {
        if entry.get("source_kind").and_then(Value::as_str) != Some("content_pack")
            || entry.get("active").and_then(Value::as_bool) != Some(true)
        {
            continue;
        }
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("vault manifest source lacks path"))?;
        let hash = entry
            .get("content_hash")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("vault manifest source lacks content_hash"))?;
        let size_bytes = entry
            .get("size_bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("vault manifest source lacks size_bytes"))?;
        let description = descriptions.get(path);
        let current = CurrentVaultAsset {
            sha256: strip_sha256(hash).to_owned(),
            size_bytes,
            description_available: description.is_some(),
            description_status: description.and_then(|value| value.0.clone()),
            description_method: description.and_then(|value| value.1.clone()),
        };
        if assets.insert(path.to_owned(), current).is_some() {
            bail!("current corpus contains duplicate active path {path}");
        }
    }
    Ok(assets)
}

fn complete_snapshot_paths(manifest: &VaultManifest) -> Vec<String> {
    let mut paths = Vec::with_capacity(
        manifest
            .entries
            .len()
            .saturating_add(manifest.totals.descriptions_expected),
    );
    for entry in &manifest.entries {
        paths.push(entry.path.clone());
        if entry.description_expected {
            paths.push(description_path(&entry.path));
        }
    }
    paths.sort();
    paths
}

fn removed_paths(previous: Option<&ImportState>, manifest: &VaultManifest) -> Vec<String> {
    let current: std::collections::HashSet<&str> = manifest
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    let mut removed = previous
        .filter(|state| state.completed_at_unix_seconds.is_some())
        .into_iter()
        .flat_map(|state| state.manifest.entries.iter())
        .filter(|entry| !current.contains(entry.path.as_str()))
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    removed.sort();
    removed.dedup();
    removed
}

fn mirror_preview_hash(manifest: &VaultManifest, removed_paths: &[String]) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&json!({
        "vault_id": manifest.vault_id,
        "scope": manifest.scope,
        "manifest_sha256": manifest.manifest_sha256,
        "removed_paths": removed_paths
    }))?)))
}

fn description_path(path: &str) -> String {
    let digest = hex::encode(Sha256::digest(path.as_bytes()));
    format!(".carrystate/generated/descriptions/{digest}.md")
}

fn is_generated_description_path(path: &str) -> bool {
    let components = path.split('/').collect::<Vec<_>>();
    components.windows(3).any(|window| {
        window[0].eq_ignore_ascii_case(".carrystate")
            && window[1].eq_ignore_ascii_case("generated")
            && window[2].eq_ignore_ascii_case("descriptions")
    })
}

fn server_marks_unsupported(path: &str, bytes: &[u8]) -> bool {
    let lower = path.to_ascii_lowercase();
    let archive = [
        ".zip", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz",
    ]
    .iter()
    .any(|extension| lower.ends_with(extension));
    archive || std::str::from_utf8(bytes).is_err() || bytes.contains(&0)
}

fn detect_media_type(path: &Path, prefix: &[u8]) -> String {
    infer::get(prefix)
        .map(|kind| kind.mime_type().to_owned())
        .or_else(|| {
            mime_guess::from_path(path)
                .first_raw()
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "application/octet-stream".to_owned())
}

fn inspect_file(
    path: &Path,
    expected_metadata: &std::fs::Metadata,
) -> Result<(String, u64, Vec<u8>)> {
    let mut file = open_inventory_file(path)?;
    let opened_metadata = file.metadata()?;
    ensure_same_regular_file(path, expected_metadata, &opened_metadata)?;
    let mut hasher = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut prefix =
        Vec::with_capacity(usize::try_from(MAX_CONTEXT_MARKDOWN_BYTES).unwrap_or(256 * 1024));
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .with_context(|| format!("could not read {}", path.display()))?;
        if count == 0 {
            break;
        }
        size_bytes = size_bytes
            .checked_add(u64::try_from(count)?)
            .ok_or_else(|| anyhow!("file size overflow for {}", path.display()))?;
        hasher.update(&buffer[..count]);
        if prefix.len() < prefix.capacity() {
            let remaining = prefix.capacity() - prefix.len();
            prefix.extend_from_slice(&buffer[..count.min(remaining)]);
        }
    }
    let final_metadata = file.metadata()?;
    let path_metadata = std::fs::symlink_metadata(path)?;
    ensure_same_regular_file(path, &opened_metadata, &final_metadata)?;
    ensure_same_regular_file(path, &opened_metadata, &path_metadata)?;
    if size_bytes != opened_metadata.len() {
        bail!("{} changed size while it was inventoried", path.display());
    }
    Ok((hex::encode(hasher.finalize()), size_bytes, prefix))
}

#[cfg(unix)]
fn open_inventory_file(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = StdOpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    options
        .open(path)
        .with_context(|| format!("could not securely open {}", path.display()))
}

#[cfg(not(unix))]
fn open_inventory_file(path: &Path) -> Result<File> {
    File::open(path).with_context(|| format!("could not open {}", path.display()))
}

#[cfg(unix)]
fn ensure_same_regular_file(
    path: &Path,
    expected: &std::fs::Metadata,
    actual: &std::fs::Metadata,
) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if !actual.is_file()
        || expected.dev() != actual.dev()
        || expected.ino() != actual.ino()
        || expected.len() != actual.len()
        || expected.mtime() != actual.mtime()
        || expected.mtime_nsec() != actual.mtime_nsec()
    {
        bail!("{} changed while it was inventoried", path.display());
    }
    Ok(())
}

fn ensure_inventory_entry_unchanged(
    path: &Path,
    expected: &InventoryEntry,
    actual: &std::fs::Metadata,
) -> Result<()> {
    if !actual.is_file()
        || actual.len() != expected.size_bytes
        || expected
            .observed_modified_unix_ns
            .is_some_and(|value| actual.modified().ok().and_then(system_time_ns) != Some(value))
        || expected
            .observed_mode
            .is_some_and(|value| file_mode(actual) != Some(value))
        || expected
            .device_id
            .is_some_and(|value| file_device_id(actual) != Some(value))
        || expected
            .inode
            .is_some_and(|value| file_inode(actual) != Some(value))
    {
        bail!(
            "{} changed after inventory; rerun the import so no unintended bytes are uploaded",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_same_regular_file(
    path: &Path,
    expected: &std::fs::Metadata,
    actual: &std::fs::Metadata,
) -> Result<()> {
    if !actual.is_file()
        || expected.len() != actual.len()
        || expected.modified().ok() != actual.modified().ok()
    {
        bail!("{} changed while it was inventoried", path.display());
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root)?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| anyhow!("vault contains a non-UTF-8 path"))?,
            ),
            _ => bail!("vault path is not a safe relative path"),
        }
    }
    Ok(parts.join("/"))
}

fn path_to_platform(path: &str) -> Result<PathBuf> {
    validate_relative_path(path)?;
    Ok(path.split('/').collect())
}

fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 1_024
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path.contains("!/")
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        bail!("unsafe or unsupported relative path: {path:?}");
    }
    Ok(())
}

fn build_globs(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder
            .add(Glob::new(pattern).with_context(|| format!("invalid glob pattern {pattern:?}"))?);
    }
    Ok(Some(builder.build()?))
}

fn batch_hash(entries: &[InventoryEntry]) -> String {
    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update((entry.path.len() as u64).to_be_bytes());
        hasher.update(entry.path.as_bytes());
        hasher.update(entry.size_bytes.to_be_bytes());
        hasher.update(entry.sha256.as_bytes());
        let context = serde_json::to_vec(&entry.attachment_context).unwrap_or_default();
        hasher.update((context.len() as u64).to_be_bytes());
        hasher.update(context);
    }
    hex::encode(hasher.finalize())
}

fn derived_vault_id(root: &Path) -> String {
    let basename = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("vault");
    let safe = basename
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let digest = hex::encode(Sha256::digest(root.as_os_str().as_encoded_bytes()));
    format!("{safe}-{}", &digest[..12])
}

fn validate_vault_id(vault_id: &str) -> Result<()> {
    if vault_id.is_empty()
        || vault_id.len() > 200
        || vault_id.chars().any(|character| character.is_control())
    {
        bail!("vault_id must be 1 to 200 printable characters");
    }
    Ok(())
}

fn short_hash(value: &str) -> String {
    let digest = hex::encode(Sha256::digest(value.as_bytes()));
    digest[..16].to_owned()
}

fn default_state_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".carrystate/imports")
}

fn read_state(path: &Path) -> Result<Option<ImportState>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
                format!("invalid import state {}", path.display())
            })?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".carrystate-state-{}.tmp", Uuid::now_v7()));
    let mut options = StdOpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    sync_directory(parent)?;
    Ok(())
}

struct StateLock {
    _file: File,
}

impl StateLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        let mut options = StdOpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&path)
            .with_context(|| format!("could not open import lock {}", path.display()))?;
        file.try_lock_exclusive()
            .with_context(|| format!("another import is using {}", path.display()))?;
        file.set_len(0)?;
        writeln!(file, "pid={}", std::process::id())?;
        file.sync_all()?;
        Ok(Self { _file: file })
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn system_time_ns(time: SystemTime) -> Option<u64> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
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

#[cfg(not(unix))]
fn file_mode(_: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(path: &str, size: u64, hash: &str) -> InventoryEntry {
        InventoryEntry {
            path: path.to_owned(),
            size_bytes: size,
            sha256: hash.repeat(64 / hash.len()),
            media_type: "application/octet-stream".to_owned(),
            modified_unix_ns: None,
            mode: None,
            observed_modified_unix_ns: None,
            observed_mode: None,
            device_id: None,
            inode: None,
            description_expected: false,
            attachment_context: Vec::new(),
        }
    }

    #[test]
    fn batching_is_stable_and_bounded() {
        let entries = vec![entry("a", 4, "a"), entry("b", 4, "b"), entry("c", 4, "c")];
        let batches = build_batches(&entries, 2, 8).unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(
            batches[0]
                .iter()
                .map(|entry| &entry.path)
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(batches[1][0].path, "c");
    }

    #[test]
    fn snapshot_includes_deterministic_description_companions() {
        let mut binary = entry("Trips/receipt.jpg", 10, "a");
        binary.description_expected = true;
        let manifest = VaultManifest {
            format: MANIFEST_FORMAT.to_owned(),
            vault_id: "test".to_owned(),
            scope: "scope:test".to_owned(),
            root: "/tmp/test".to_owned(),
            describe_binaries: true,
            entries: vec![binary],
            ignored: vec![],
            totals: InventoryTotals {
                files: 1,
                bytes: 10,
                ignored: 0,
                descriptions_expected: 1,
            },
            manifest_sha256: "0".repeat(64),
        };
        let paths = complete_snapshot_paths(&manifest);
        assert_eq!(paths.len(), 2);
        assert!(paths[0].starts_with(".carrystate/generated/descriptions/"));
        assert_eq!(paths[1], "Trips/receipt.jpg");
    }

    #[test]
    fn current_vault_assets_use_manifest_descriptions_without_usage_rankings() {
        let source_path = "Trips/receipt.jpg";
        let response = json!({
            "format": "carrystate-vault-manifest@v1",
            "entries": [
                {
                    "path": source_path,
                    "source_kind": "content_pack",
                    "content_hash": format!("sha256:{}", "a".repeat(64)),
                    "size_bytes": 10,
                    "active": true
                },
                {
                    "path": description_path(source_path),
                    "original_path": source_path,
                    "source_kind": "derived_asset_description",
                    "description_status": "complete",
                    "description_method": "openai_responses",
                    "content_hash": format!("sha256:{}", "b".repeat(64)),
                    "size_bytes": 20,
                    "active": true
                }
            ]
        });

        let assets = current_vault_assets_from_manifest(&response).unwrap();
        let source = assets.get(source_path).unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(source.sha256, "a".repeat(64));
        assert_eq!(source.size_bytes, 10);
        assert!(source.description_available);
        assert_eq!(source.description_status.as_deref(), Some("complete"));
        assert_eq!(
            source.description_method.as_deref(),
            Some("openai_responses")
        );
    }

    #[test]
    fn generated_description_directories_are_reserved_at_any_vault_depth() {
        assert!(is_generated_description_path(
            ".carrystate/generated/descriptions/fixture.md"
        ));
        assert!(is_generated_description_path(
            "nested/.carrystate/generated/descriptions/fixture.md"
        ));
        assert!(is_generated_description_path(
            "nested/.CARRYSTATE/Generated/Descriptions/fixture.md"
        ));
        assert!(!is_generated_description_path(
            ".carrystate/generated/user-authored.md"
        ));
        assert!(!is_generated_description_path(
            "notes/generated/descriptions/fixture.md"
        ));
    }

    #[test]
    fn metadata_only_changes_are_imported_as_new_logical_versions() {
        let mut prior_entry = entry("receipt.png", 10, "a");
        prior_entry.modified_unix_ns = Some(1);
        prior_entry.mode = Some(0o644);
        let prior_manifest = VaultManifest {
            format: MANIFEST_FORMAT.to_owned(),
            vault_id: "vault".to_owned(),
            scope: "scope:test".to_owned(),
            root: "/tmp/vault".to_owned(),
            describe_binaries: true,
            entries: vec![prior_entry.clone()],
            ignored: vec![],
            totals: InventoryTotals {
                files: 1,
                bytes: 10,
                ignored: 0,
                descriptions_expected: 0,
            },
            manifest_sha256: "1".repeat(64),
        };
        let previous = ImportState {
            format: STATE_FORMAT.to_owned(),
            vault_id: "vault".to_owned(),
            scope: "scope:test".to_owned(),
            mirror: false,
            manifest: prior_manifest,
            batches: vec![],
            completed_at_unix_seconds: Some(1),
        };
        let mut changed_entry = prior_entry;
        changed_entry.modified_unix_ns = Some(2);
        let changed_manifest = VaultManifest {
            manifest_sha256: "2".repeat(64),
            entries: vec![changed_entry],
            ..previous.manifest.clone()
        };

        let state = prepare_state(Some(&previous), changed_manifest, 200, 1024, false).unwrap();
        assert_eq!(state.batches.len(), 1);
        assert_eq!(state.batches[0].paths, ["receipt.png"]);
    }

    #[test]
    fn portable_manifest_identity_ignores_machine_local_file_ids() {
        let mut first = entry("receipt.png", 10, "a");
        first.device_id = Some(1);
        first.inode = Some(2);
        first.observed_modified_unix_ns = Some(3);
        first.observed_mode = Some(0o644);
        let mut relocated = first.clone();
        relocated.device_id = Some(9);
        relocated.inode = Some(10);
        relocated.observed_modified_unix_ns = Some(11);
        relocated.observed_mode = Some(0o600);

        assert_eq!(
            portable_manifest_entry(&first),
            portable_manifest_entry(&relocated)
        );
    }

    #[test]
    fn portable_export_metadata_restores_filesystem_values_from_exact_bytes() {
        let temporary = tempdir().unwrap();
        let sources = temporary.path().join("sources");
        std::fs::create_dir(&sources).unwrap();
        let manifest = serde_json::to_vec(&json!({
            "format": "carrystate-portable-vault@v1",
            "entries": [{
                "path": "sources/Trips/receipt.png",
                "content_hash": format!("sha256:{}", "a".repeat(64)),
                "size_bytes": 10,
                "media_type": "image/png",
                "modified_unix_ns": 123,
                "mode": 0o640
            }]
        }))
        .unwrap();
        std::fs::write(temporary.path().join("manifest.json"), &manifest).unwrap();
        std::fs::write(
            temporary.path().join("CHECKSUMS.sha256"),
            format!(
                "{}  manifest.json\n",
                hex::encode(Sha256::digest(&manifest))
            ),
        )
        .unwrap();

        let metadata = load_portable_export_metadata(&sources).unwrap();
        let receipt = metadata.get("Trips/receipt.png").unwrap();
        assert_eq!(receipt.sha256, "a".repeat(64));
        assert_eq!(receipt.size_bytes, 10);
        assert_eq!(receipt.media_type, "image/png");
        assert_eq!(receipt.modified_unix_ns, Some(123));
        assert_eq!(receipt.mode, Some(0o640));
    }

    #[test]
    fn portable_export_metadata_rejects_a_manifest_changed_after_export() {
        let temporary = tempdir().unwrap();
        let sources = temporary.path().join("sources");
        std::fs::create_dir(&sources).unwrap();
        let manifest_path = temporary.path().join("manifest.json");
        let original = br#"{"format":"carrystate-portable-vault@v1","entries":[]}"#;
        std::fs::write(&manifest_path, original).unwrap();
        std::fs::write(
            temporary.path().join("CHECKSUMS.sha256"),
            format!("{}  manifest.json\n", hex::encode(Sha256::digest(original))),
        )
        .unwrap();
        std::fs::write(
            &manifest_path,
            br#"{"format":"carrystate-portable-vault@v1","entries":[{}]}"#,
        )
        .unwrap();

        let error = load_portable_export_metadata(&sources).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not match CHECKSUMS.sha256")
        );
    }

    #[test]
    fn path_validation_rejects_archive_and_parent_escapes() {
        assert!(validate_relative_path("../escape").is_err());
        assert!(validate_relative_path("archive.zip!/member").is_err());
        assert!(validate_relative_path("safe/path.md").is_ok());
    }

    #[test]
    fn attachment_context_supports_obsidian_and_markdown_local_links() {
        let mut entries = vec![
            entry("Trips/plan.md", 10, "a"),
            entry("Trips/receipt.jpg", 10, "b"),
            entry("Trips/room.png", 10, "c"),
            entry("Maps/route.png", 10, "d"),
        ];
        let sources = vec![MarkdownSource {
            path: "Trips/plan.md".to_owned(),
            text: "Expense backup: ![[receipt.jpg|meal receipt]].\n\n\
                   Room reference: ![current room](room.png).\n\n\
                   Route overview: [[Maps/route.png]]."
                .to_owned(),
        }];
        attach_markdown_context(&mut entries, &sources);

        let by_path = entries
            .iter()
            .map(|entry| (entry.path.as_str(), &entry.attachment_context))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(by_path["Trips/receipt.jpg"].len(), 1);
        assert!(
            by_path["Trips/receipt.jpg"][0]
                .excerpt
                .contains("Expense backup")
        );
        assert_eq!(by_path["Trips/room.png"].len(), 1);
        assert!(
            by_path["Trips/room.png"][0]
                .excerpt
                .contains("Room reference")
        );
        assert_eq!(by_path["Maps/route.png"].len(), 1);
        assert_eq!(
            by_path["Maps/route.png"][0].source_markdown_path,
            "Trips/plan.md"
        );
    }

    #[test]
    fn attachment_context_rejects_urls_traversal_anchors_and_ambiguity() {
        let mut entries = vec![
            entry("Notes/plan.md", 10, "a"),
            entry("Notes/local.png", 10, "b"),
            entry("outside.png", 10, "c"),
            entry("A/shared.png", 10, "d"),
            entry("B/shared.png", 10, "e"),
        ];
        let sources = vec![MarkdownSource {
            path: "Notes/plan.md".to_owned(),
            text: "![url](https://example.com/local.png)\n\
                   ![parent](../outside.png)\n\
                   ![absolute](/outside.png)\n\
                   ![anchor](local.png#crop)\n\
                   [[shared.png]]\n\
                   [[missing.png]]"
                .to_owned(),
        }];
        attach_markdown_context(&mut entries, &sources);

        assert!(
            entries
                .iter()
                .all(|entry| entry.attachment_context.is_empty())
        );
    }

    #[test]
    fn attachment_context_is_deduplicated_and_strictly_bounded() {
        let mut entries = vec![entry("plan.md", 10, "a"), entry("target.bin", 10, "b")];
        let mut text = "same paragraph [target](target.bin)\n\
                        same paragraph [target](target.bin)"
            .to_owned();
        for index in 0..32 {
            text.push_str(&format!(
                "\n\nContext {index}: {} [target](target.bin)",
                "x".repeat(300)
            ));
        }
        let sources = vec![MarkdownSource {
            path: "plan.md".to_owned(),
            text,
        }];
        attach_markdown_context(&mut entries, &sources);

        let contexts = &entries
            .iter()
            .find(|entry| entry.path == "target.bin")
            .unwrap()
            .attachment_context;
        assert_eq!(contexts.len(), MAX_CONTEXTS_PER_TARGET);
        assert!(
            contexts
                .iter()
                .all(|context| context.excerpt.chars().count() <= MAX_CONTEXT_EXCERPT_CHARS)
        );
        assert!(
            contexts
                .iter()
                .map(|context| context.excerpt.chars().count())
                .sum::<usize>()
                <= MAX_CONTEXT_TOTAL_CHARS_PER_TARGET
        );
        let unique = contexts
            .iter()
            .map(|context| (&context.source_markdown_path, &context.excerpt))
            .collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), contexts.len());
    }

    #[test]
    fn markdown_scanner_ignores_code_and_caps_reference_count() {
        let mut text = "`[inline](target.bin)`\n```\n[target](target.bin)\n```\n".to_owned();
        for index in 0..(MAX_CONTEXT_LINKS_PER_MARKDOWN + 20) {
            text.push_str(&format!("[{index}](target.bin)\n"));
        }
        let links = markdown_link_occurrences(&text);
        assert_eq!(links.len(), MAX_CONTEXT_LINKS_PER_MARKDOWN);
        assert!(links.iter().all(|link| link.target == "target.bin"));
    }

    #[test]
    fn context_scan_and_batch_identity_include_strict_bounds() {
        assert!(should_scan_markdown_context(
            "note.MD",
            MAX_CONTEXT_MARKDOWN_BYTES
        ));
        assert!(!should_scan_markdown_context(
            "note.md",
            MAX_CONTEXT_MARKDOWN_BYTES + 1
        ));
        assert!(!should_scan_markdown_context(
            "note.txt",
            MAX_CONTEXT_MARKDOWN_BYTES
        ));

        let left = entry("target.bin", 10, "a");
        let mut right = left.clone();
        right.attachment_context.push(AttachmentContext {
            source_markdown_path: "note.md".to_owned(),
            excerpt: "new context".to_owned(),
        });
        assert_ne!(batch_hash(&[left]), batch_hash(&[right]));
    }
}
