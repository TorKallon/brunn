use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs::OpenOptions,
    io::Write,
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

use crate::carrystate_cli::{ApiClient, strip_sha256};

const PORTABLE_FORMAT: &str = "carrystate-portable-vault@v1";

#[derive(Debug, Clone, Args)]
pub struct ExportArgs {
    #[arg(long)]
    pub output: PathBuf,
    #[arg(long)]
    pub vault_id: String,
    #[arg(long, default_value = "scope:root")]
    pub scope: String,
    #[arg(long)]
    pub as_of: Option<String>,
    #[arg(long)]
    pub history: bool,
    /// Include every active typed memory record in the selected scope.
    ///
    /// This is intentionally opt-in because typed records are scope-wide and
    /// cannot yet be attributed losslessly to one imported vault.
    #[arg(long)]
    pub include_scope_memory: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ImportMetadata {
    modified_unix_ns: Option<u64>,
    mode: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
struct VaultEntry {
    path: Option<String>,
    original_path: Option<String>,
    category: String,
    source_ref: String,
    source_kind: String,
    asset_ref: String,
    version: i32,
    content_hash: String,
    size_bytes: u64,
    media_type: String,
    active: bool,
    #[serde(default)]
    import_metadata: Option<ImportMetadata>,
}

#[derive(Clone, Debug, Deserialize)]
struct VaultManifest {
    format: String,
    stable_import_id: String,
    corpus_revision: String,
    include_history: bool,
    #[serde(default)]
    include_scope_memory: bool,
    entries: Vec<VaultEntry>,
    #[serde(default)]
    native_records: Vec<NativeRecord>,
}

#[derive(Clone, Debug, Deserialize)]
struct NativeRecord {
    record_ref: String,
    record_kind: String,
    record_version: Option<i32>,
    title: String,
    payload: Value,
}

#[derive(Clone, Debug, Serialize)]
struct PortableEntry {
    path: String,
    content_hash: String,
    size_bytes: u64,
    media_type: String,
    source_ref: String,
    source_kind: String,
    asset_ref: String,
    asset_version: i32,
    active: bool,
    modified_unix_ns: Option<u64>,
    mode: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
struct PortableNativeRecord {
    path: String,
    record_ref: String,
    record_kind: String,
    record_version: Option<i32>,
    content_hash: String,
    size_bytes: u64,
}

pub async fn run(client: &ApiClient, args: ExportArgs) -> Result<()> {
    validate_identifier(&args.vault_id, "vault_id")?;
    validate_identifier(&args.scope, "scope")?;
    if args.output.exists() {
        bail!("refusing to overwrite {}", args.output.display());
    }
    let session_id = open_export_session(client, &args).await?;
    let stable_import_id = format!("vault:{}", args.vault_id);
    let include_history = if args.history { "true" } else { "false" };
    let manifest_value = client
        .get_json(
            "/v1/vault/manifest",
            &[
                ("session_id", session_id.as_str()),
                ("stable_import_id", stable_import_id.as_str()),
                ("include_history", include_history),
                (
                    "include_scope_memory",
                    if args.include_scope_memory {
                        "true"
                    } else {
                        "false"
                    },
                ),
            ],
        )
        .await?;
    let manifest: VaultManifest = serde_json::from_value(manifest_value)
        .context("CarryState returned an invalid vault manifest")?;
    if manifest.format != "carrystate-vault-manifest@v1"
        || manifest.stable_import_id != stable_import_id
        || manifest.include_history != args.history
        || manifest.include_scope_memory != args.include_scope_memory
    {
        bail!("CarryState returned a vault manifest for a different export request");
    }
    let parent = args.output.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let staging_path = parent.join(format!(".carrystate-export-{}.tmp", Uuid::now_v7()));
    std::fs::create_dir(&staging_path)?;
    set_private_directory(&staging_path)?;
    let mut staging = StagingDirectory {
        path: staging_path,
        published: false,
    };
    let mut outputs = output_plan(&manifest.entries, args.history)?;
    outputs.sort_by(|left, right| left.0.cmp(&right.0));
    validate_output_collisions(&outputs)?;
    let mut portable_entries = Vec::with_capacity(outputs.len());
    for (relative_output, entry) in outputs {
        let destination = safe_join(&staging.path, &relative_output)?;
        let query = [
            ("session_id", session_id.as_str()),
            ("stable_import_id", stable_import_id.as_str()),
        ];
        client
            .download_verified(
                &format!(
                    "/v1/vault/assets/{}/versions/{}/content",
                    entry.asset_ref, entry.version
                ),
                &query,
                &destination,
                &entry.content_hash,
                Some(entry.size_bytes),
            )
            .await
            .with_context(|| format!("could not export {}", relative_output))?;
        let mut metadata = entry.import_metadata.clone().unwrap_or_default();
        metadata.mode = metadata.mode.map(|mode| mode & 0o777);
        restore_file_metadata(&destination, &metadata)?;
        portable_entries.push(PortableEntry {
            path: relative_output,
            content_hash: format!("sha256:{}", strip_sha256(&entry.content_hash)),
            size_bytes: entry.size_bytes,
            media_type: entry.media_type,
            source_ref: entry.source_ref,
            source_kind: entry.source_kind,
            asset_ref: entry.asset_ref,
            asset_version: entry.version,
            active: entry.active,
            modified_unix_ns: metadata.modified_unix_ns,
            mode: metadata.mode,
        });
    }
    portable_entries.sort_by(|left, right| left.path.cmp(&right.path));
    let native_records = write_native_records(
        &staging.path,
        &manifest.native_records,
        &manifest.corpus_revision,
    )?;
    let portable_manifest = json!({
        "format": PORTABLE_FORMAT,
        "version": 1,
        "vault_id": &args.vault_id,
        "stable_import_id": &stable_import_id,
        "scope": &args.scope,
        "corpus_revision": &manifest.corpus_revision,
        "include_history": args.history,
        "include_scope_memory": args.include_scope_memory,
        "entries": &portable_entries,
        "native_records": &native_records
    });
    write_json(&staging.path.join("manifest.json"), &portable_manifest)?;
    write_checksums(&staging.path)?;
    sync_tree_directories(&staging.path)?;
    std::fs::rename(&staging.path, &args.output)?;
    sync_directory(parent)?;
    staging.published = true;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "complete",
            "output": args.output,
            "vault_id": args.vault_id,
            "scope": args.scope,
            "corpus_revision": manifest.corpus_revision,
            "files": portable_entries.len(),
            "bytes": portable_entries.iter().map(|entry| entry.size_bytes).sum::<u64>(),
            "native_records": native_records.len(),
            "include_scope_memory": args.include_scope_memory,
            "include_history": args.history
        }))?
    );
    Ok(())
}

fn write_native_records(
    root: &Path,
    records: &[NativeRecord],
    corpus_revision: &str,
) -> Result<Vec<PortableNativeRecord>> {
    let mut seen = BTreeSet::new();
    let mut output = Vec::with_capacity(records.len());
    for record in records {
        validate_record_kind(&record.record_kind)?;
        let id = record
            .record_ref
            .strip_prefix(&format!("{}:", record.record_kind))
            .ok_or_else(|| anyhow!("native record reference does not match its kind"))?;
        let id = Uuid::parse_str(id).context("native record reference is not a UUID")?;
        let relative = format!("workspace/memory/{}/{}.md", record.record_kind, id);
        validate_relative_path(&relative)?;
        let collision_key = relative.nfc().collect::<String>().to_lowercase();
        if !seen.insert(collision_key) {
            bail!("duplicate native record export path: {relative}");
        }
        let bytes = render_native_record(record, corpus_revision)?;
        let destination = safe_join(root, &relative)?;
        write_bytes(&destination, &bytes)?;
        output.push(PortableNativeRecord {
            path: relative,
            record_ref: record.record_ref.clone(),
            record_kind: record.record_kind.clone(),
            record_version: record.record_version,
            content_hash: format!("sha256:{}", hash_bytes(&bytes)),
            size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
    }
    output.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(output)
}

fn render_native_record(record: &NativeRecord, corpus_revision: &str) -> Result<Vec<u8>> {
    let payload = serde_json::to_string_pretty(&record.payload)?;
    let fence = "`".repeat(longest_backtick_run(&payload).saturating_add(1).max(3));
    let title = record
        .title
        .replace(['\r', '\n'], " ")
        .chars()
        .take(240)
        .collect::<String>();
    let version = record
        .record_version
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_owned());
    Ok(format!(
        "---\n\
         carrystate_format: native-record@v1\n\
         record_ref: \"{}\"\n\
         record_kind: \"{}\"\n\
         record_version: {}\n\
         corpus_revision: \"{}\"\n\
         ---\n\
         # {}\n\n\
         This is a deterministic export of the complete structured CarryState record at the pinned corpus revision.\n\n\
         {fence}json\n{payload}\n{fence}\n",
        record.record_ref,
        record.record_kind,
        version,
        corpus_revision,
        if title.is_empty() { "Untitled record" } else { &title }
    )
    .into_bytes())
}

fn longest_backtick_run(value: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for character in value.chars() {
        if character == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

fn validate_record_kind(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        bail!("native record kind is unsafe: {value:?}");
    }
    Ok(())
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

async fn open_export_session(client: &ApiClient, args: &ExportArgs) -> Result<String> {
    let mut request = json!({
        "task": format!("Export vault {}", args.vault_id),
        "hints": {"authorization_scope": args.scope},
        "mode": "exploration",
        "token_budget": 1024
    });
    if let Some(as_of) = &args.as_of {
        request["as_of"] = Value::String(as_of.clone());
    }
    let opened = client.post_json("/v1/memory/open", &request).await?;
    opened
        .get("session_id")
        .and_then(Value::as_str)
        .or_else(|| {
            opened
                .get("data")
                .and_then(|data| data.get("session_id"))
                .and_then(Value::as_str)
        })
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("vault export session did not open"))
}

fn output_plan(entries: &[VaultEntry], include_history: bool) -> Result<Vec<(String, VaultEntry)>> {
    let mut seen = HashSet::new();
    let mut outputs = Vec::new();
    for entry in entries {
        let source_path = entry
            .path
            .as_deref()
            .ok_or_else(|| anyhow!("vault manifest entry has no path"))?;
        validate_relative_path(source_path)?;
        let identity = (
            entry.source_ref.clone(),
            entry.asset_ref.clone(),
            entry.version,
            entry.active,
        );
        if !seen.insert(identity) {
            continue;
        }
        let current_path = if entry.category == "generated_description" {
            let described_path = entry.original_path.as_deref().unwrap_or(source_path);
            validate_relative_path(described_path)?;
            format!("workspace/assets/{described_path}.md")
        } else {
            format!("sources/{source_path}")
        };
        let output_path = if entry.active {
            current_path
        } else if include_history {
            let digest = strip_sha256(&entry.content_hash);
            format!(
                "history/{}/v{}-{}",
                current_path,
                entry.version,
                &digest[..digest.len().min(16)]
            )
        } else {
            continue;
        };
        validate_relative_path(&output_path)?;
        outputs.push((output_path, entry.clone()));
    }
    Ok(outputs)
}

fn validate_output_collisions(outputs: &[(String, VaultEntry)]) -> Result<()> {
    let mut portable = BTreeMap::<String, (&str, &str)>::new();
    for (path, entry) in outputs {
        let key = path.nfc().collect::<String>().to_lowercase();
        if let Some((prior_path, prior_hash)) =
            portable.insert(key, (path.as_str(), entry.content_hash.as_str()))
            && (prior_path != path || prior_hash != entry.content_hash)
        {
            bail!("portable path collision between {prior_path:?} and {path:?}");
        }
    }
    Ok(())
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
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
        let absolute = safe_join(root, &relative)?;
        let digest = hash_file(&absolute)?;
        writeln!(output, "{digest}  {relative}")?;
    }
    output.sync_all()?;
    Ok(())
}

fn collect_files(root: &Path, directory: &Path, files: &mut BTreeSet<String>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = entry.file_type()?;
        if metadata.is_symlink() {
            bail!("export staging unexpectedly contains a symbolic link");
        }
        if metadata.is_dir() {
            collect_files(root, &entry.path(), files)?;
        } else if metadata.is_file() {
            let relative = entry.path().strip_prefix(root)?.to_path_buf();
            files.insert(platform_to_relative(&relative)?);
        } else {
            bail!("export staging unexpectedly contains a special file");
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
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

fn restore_file_metadata(path: &Path, metadata: &ImportMetadata) -> Result<()> {
    if let Some(modified_unix_ns) = metadata.modified_unix_ns {
        let seconds = i64::try_from(modified_unix_ns / 1_000_000_000)
            .context("file modification time exceeds platform range")?;
        let nanoseconds = u32::try_from(modified_unix_ns % 1_000_000_000)?;
        filetime::set_file_mtime(path, FileTime::from_unix_time(seconds, nanoseconds))?;
    }
    #[cfg(unix)]
    if let Some(mode) = metadata.mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777))?;
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
    let canonical_parent = parent.canonicalize()?;
    if !canonical_parent.starts_with(root.canonicalize()?) {
        bail!("export path escaped its destination");
    }
    Ok(destination)
}

fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        bail!("unsafe portable path: {path:?}");
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
    Ok(parts.join("/"))
}

fn validate_identifier(value: &str, name: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 240 || value.chars().any(char::is_control) {
        bail!("{name} must contain 1 to 240 printable characters");
    }
    Ok(())
}

fn sync_tree_directories(root: &Path) -> Result<()> {
    let mut directories = Vec::new();
    for entry in walkdir::WalkDir::new(root).contents_first(true) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            directories.push(entry.path().to_path_buf());
        }
    }
    for directory in directories {
        sync_directory(&directory)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
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

    fn entry(path: &str, category: &str, active: bool, version: i32) -> VaultEntry {
        VaultEntry {
            path: Some(path.to_owned()),
            original_path: None,
            category: category.to_owned(),
            source_ref: format!("source:{version}"),
            source_kind: "content_pack".to_owned(),
            asset_ref: format!("asset:{version}"),
            version,
            content_hash: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 1,
            media_type: "application/octet-stream".to_owned(),
            active,
            import_metadata: None,
        }
    }

    #[test]
    fn current_and_history_paths_are_deterministic() {
        let entries = vec![
            entry("Trips/receipt.png", "source", true, 2),
            entry("Trips/receipt.png", "source", false, 1),
        ];
        let outputs = output_plan(&entries, true).unwrap();
        assert!(
            outputs
                .iter()
                .any(|(path, _)| path == "sources/Trips/receipt.png")
        );
        assert!(
            outputs
                .iter()
                .any(|(path, _)| { path.starts_with("history/sources/Trips/receipt.png/v1-") })
        );
    }

    #[test]
    fn unsafe_and_case_colliding_paths_fail_closed() {
        assert!(validate_relative_path("../escape").is_err());
        let outputs = vec![
            (
                "sources/A.txt".to_owned(),
                entry("A.txt", "source", true, 1),
            ),
            (
                "sources/a.txt".to_owned(),
                entry("a.txt", "source", true, 2),
            ),
        ];
        assert!(validate_output_collisions(&outputs).is_err());
    }

    #[test]
    fn native_record_render_is_deterministic_and_fence_safe() {
        let record = NativeRecord {
            record_ref: "claim:550e8400-e29b-41d4-a716-446655440000".to_owned(),
            record_kind: "claim".to_owned(),
            record_version: None,
            title: "Decision\nwith newline".to_owned(),
            payload: json!({"value": "literal ``` fence", "verified": true}),
        };
        let first = render_native_record(&record, "revision:test").unwrap();
        let second = render_native_record(&record, "revision:test").unwrap();
        assert_eq!(first, second);
        let text = String::from_utf8(first).unwrap();
        assert!(text.contains("# Decision with newline"));
        assert!(text.contains("````json"));
        assert!(text.contains("\"verified\": true"));
    }

    #[test]
    fn native_record_kinds_cannot_escape_the_export_tree() {
        assert!(validate_record_kind("checkpoint").is_ok());
        assert!(validate_record_kind("../checkpoint").is_err());
        assert!(validate_record_kind("CheckPoint").is_err());
    }
}
