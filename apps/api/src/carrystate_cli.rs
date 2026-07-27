use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use futures::StreamExt;
use reqwest::{Body, Client, Method, Response, Url, header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
};
use uuid::Uuid;

use crate::{carrystate_export, carrystate_import};

#[derive(Debug, Parser)]
#[command(
    name = "carrystate",
    version,
    about = "Straylight workspace and binary client"
)]
pub struct Cli {
    #[arg(long, env = "CARRYSTATE_API_URL")]
    api_url: Option<String>,
    #[arg(long, env = "CARRYSTATE_API_TOKEN", hide_env_values = true)]
    token: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(alias = "vault")]
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    Asset {
        #[command(subcommand)]
        command: AssetCommand,
    },
}

#[derive(Debug, Subcommand)]
enum WorkspaceCommand {
    Import(carrystate_import::ImportArgs),
    Export(carrystate_export::ExportArgs),
}

#[derive(Debug, Subcommand)]
enum AssetCommand {
    Metadata(AssetMetadataArgs),
    Fetch(AssetFetchArgs),
}

#[derive(Debug, Clone, Args)]
struct AssetMetadataArgs {
    #[arg(long)]
    session_id: String,
    #[arg(long)]
    asset_ref: String,
}

#[derive(Debug, Clone, Args)]
struct AssetFetchArgs {
    #[arg(long)]
    session_id: String,
    #[arg(long)]
    asset_ref: String,
    #[arg(long)]
    version: i32,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
    base_url: Url,
    token: String,
}

impl ApiClient {
    pub fn new(base_url: &str, token: String) -> Result<Self> {
        let mut base_url = Url::parse(base_url)
            .with_context(|| format!("invalid CarryState API URL: {base_url}"))?;
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(15 * 60))
            .user_agent(concat!("carrystate-cli/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            client,
            base_url,
            token,
        })
    }

    pub fn url(&self, path: &str) -> Result<Url> {
        self.base_url
            .join(path.trim_start_matches('/'))
            .with_context(|| format!("invalid API path: {path}"))
    }

    pub fn endpoint(&self) -> &str {
        self.base_url.as_str()
    }

    pub fn authorized(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&self.token)
            .header(header::ACCEPT, "application/json")
    }

    pub async fn get_json(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        let mut url = self.url(path)?;
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        decode_json(self.authorized(Method::GET, url).send().await?).await
    }

    pub async fn post_json(&self, path: &str, body: &Value) -> Result<Value> {
        let url = self.url(path)?;
        decode_json(self.authorized(Method::POST, url).json(body).send().await?).await
    }

    pub async fn delete_json(&self, path: &str) -> Result<Value> {
        let url = self.url(path)?;
        decode_json(self.authorized(Method::DELETE, url).send().await?).await
    }

    pub async fn post_multipart(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<Value> {
        let url = self.url(path)?;
        decode_json(
            self.authorized(Method::POST, url)
                .multipart(form)
                .send()
                .await?,
        )
        .await
    }

    pub async fn put_bytes(
        &self,
        path: &str,
        content_hash: &str,
        bytes: bytes::Bytes,
    ) -> Result<Value> {
        let url = self.url(path)?;
        decode_json(
            self.authorized(Method::PUT, url)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header("x-carrystate-part-sha256", strip_sha256(content_hash))
                .body(bytes)
                .send()
                .await?,
        )
        .await
    }

    pub async fn put_stream(
        &self,
        path: &str,
        query: &[(&str, &str)],
        media_type: &str,
        size_bytes: u64,
        body: Body,
    ) -> Result<Value> {
        let mut url = self.url(path)?;
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        decode_json(
            self.authorized(Method::PUT, url)
                .header(header::CONTENT_TYPE, media_type)
                .header(header::CONTENT_LENGTH, size_bytes)
                .body(body)
                .send()
                .await?,
        )
        .await
    }

    pub async fn download_verified(
        &self,
        path: &str,
        query: &[(&str, &str)],
        output: &Path,
        expected_hash: &str,
        expected_size: Option<u64>,
    ) -> Result<VerifiedFile> {
        let mut url = self.url(path)?;
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        let response = self.authorized(Method::GET, url).send().await?;
        if !response.status().is_success() {
            return Err(response_error(response).await);
        }
        let header_hash = response
            .headers()
            .get("x-carrystate-sha256")
            .and_then(|value| value.to_str().ok())
            .map(strip_sha256);
        if let Some(header_hash) = header_hash
            && header_hash != strip_sha256(expected_hash)
        {
            bail!("the API content hash header does not match asset metadata");
        }
        write_verified_response(response, output, expected_hash, expected_size).await
    }
}

#[derive(Debug)]
pub struct VerifiedFile {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub media_type: Option<String>,
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Workspace {
            command: WorkspaceCommand::Import(args),
        } if args.dry_run => carrystate_import::run(None, args).await,
        command => {
            let base_url = cli
                .api_url
                .or_else(|| env::var("STRAYLIGHT_API_URL").ok())
                .unwrap_or_else(|| "http://127.0.0.1:18110".to_owned());
            let token = cli
                .token
                .or_else(|| env::var("STRAYLIGHT_API_TOKEN").ok())
                .ok_or_else(|| {
                    anyhow!(
                        "CARRYSTATE_API_TOKEN is required (STRAYLIGHT_API_TOKEN is accepted as a legacy alias)"
                    )
                })?;
            let client = ApiClient::new(&base_url, token)?;
            match command {
                Command::Workspace {
                    command: WorkspaceCommand::Import(args),
                } => carrystate_import::run(Some(&client), args).await,
                Command::Workspace {
                    command: WorkspaceCommand::Export(args),
                } => carrystate_export::run(&client, args).await,
                Command::Asset {
                    command: AssetCommand::Metadata(args),
                } => asset_metadata(&client, args).await,
                Command::Asset {
                    command: AssetCommand::Fetch(args),
                } => asset_fetch(&client, args).await,
            }
        }
    }
}

async fn asset_metadata(client: &ApiClient, args: AssetMetadataArgs) -> Result<()> {
    let value = client
        .get_json(
            &format!("/v1/assets/{}", args.asset_ref),
            &[("session_id", args.session_id.as_str())],
        )
        .await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn asset_fetch(client: &ApiClient, args: AssetFetchArgs) -> Result<()> {
    if args.version <= 0 {
        bail!("asset version must be positive");
    }
    let metadata = client
        .get_json(
            &format!("/v1/assets/{}", args.asset_ref),
            &[("session_id", args.session_id.as_str())],
        )
        .await?;
    let expected_version = value_i64(&metadata, "version")?;
    if expected_version != i64::from(args.version) {
        bail!(
            "requested asset version {} is not the session-pinned version {}",
            args.version,
            expected_version
        );
    }
    let expected_hash = value_str(&metadata, "content_hash")?;
    let expected_size =
        u64::try_from(value_i64(&metadata, "size_bytes")?).context("asset size is negative")?;
    let file = client
        .download_verified(
            &format!(
                "/v1/assets/{}/versions/{}/content",
                args.asset_ref, args.version
            ),
            &[("session_id", args.session_id.as_str())],
            &args.output,
            expected_hash,
            Some(expected_size),
        )
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "asset_ref": args.asset_ref,
            "version": args.version,
            "path": file.path,
            "content_hash": format!("sha256:{}", file.sha256),
            "size_bytes": file.size_bytes,
            "media_type": file.media_type
        }))?
    );
    Ok(())
}

async fn decode_json(response: Response) -> Result<Value> {
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    let bytes = response.bytes().await?;
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).context("CarryState returned invalid JSON")
}

async fn response_error(response: Response) -> anyhow::Error {
    let status = response.status();
    let bytes = response.bytes().await.unwrap_or_default();
    let detail = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| String::from_utf8_lossy(&bytes[..bytes.len().min(2_000)]).into_owned());
    anyhow!("CarryState API returned HTTP {status}: {detail}")
}

async fn write_verified_response(
    response: Response,
    output: &Path,
    expected_hash: &str,
    expected_size: Option<u64>,
) -> Result<VerifiedFile> {
    if output.exists() {
        bail!("refusing to overwrite {}", output.display());
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).await?;
    let temporary = parent.join(format!(".carrystate-download-{}.part", Uuid::now_v7()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).await?;
    let media_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let mut hasher = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut stream = response.bytes_stream();
    let write_result: Result<()> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            size_bytes = size_bytes
                .checked_add(u64::try_from(chunk.len())?)
                .ok_or_else(|| anyhow!("download size overflow"))?;
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(())
    }
    .await;
    drop(file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    let actual_hash = hex::encode(hasher.finalize());
    if actual_hash != strip_sha256(expected_hash)
        || expected_size.is_some_and(|expected| expected != size_bytes)
    {
        let _ = fs::remove_file(&temporary).await;
        bail!(
            "download integrity check failed: expected sha256:{} / {:?} bytes, received sha256:{} / {} bytes",
            strip_sha256(expected_hash),
            expected_size,
            actual_hash,
            size_bytes
        );
    }
    fs::hard_link(&temporary, output)
        .await
        .with_context(|| format!("could not publish {}", output.display()))?;
    fs::remove_file(&temporary).await?;
    Ok(VerifiedFile {
        path: output.to_owned(),
        sha256: actual_hash,
        size_bytes,
        media_type,
    })
}

pub fn unwrap_data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

pub fn value_str<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("CarryState response lacks {field}"))
}

pub fn value_i64(value: &Value, field: &str) -> Result<i64> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("CarryState response lacks {field}"))
}

pub fn strip_sha256(value: &str) -> &str {
    value.strip_prefix("sha256:").unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_data_is_unwrapped_without_changing_raw_responses() {
        let wrapped = json!({"data": {"status": "ready"}});
        assert_eq!(unwrap_data(&wrapped)["status"], "ready");
        let raw = json!({"status": "ready"});
        assert_eq!(unwrap_data(&raw)["status"], "ready");
    }

    #[test]
    fn api_base_paths_are_preserved() {
        let client = ApiClient::new("https://straylight.example/api", "secret".to_owned()).unwrap();
        assert_eq!(
            client.url("/v1/workspace/manifest").unwrap().as_str(),
            "https://straylight.example/api/v1/workspace/manifest"
        );
    }
}
