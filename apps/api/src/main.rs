use std::path::PathBuf;

use clap::{Parser, Subcommand};
use straylight::{
    AppState, Config, db,
    logging::UdpLogMakeWriter,
    object_store::{ObjectStore, backup as object_backup},
    operator_service,
    request_query_count::QueryCountLayer,
    router, telemetry, worker,
};
use tokio::net::TcpListener;
use tracing_subscriber::{
    EnvFilter, Layer as _, filter::filter_fn, fmt::writer::MakeWriterExt, layer::SubscriberExt,
    util::SubscriberInitExt,
};

#[derive(Debug, Parser)]
#[command(name = "straylight", about = "Agent-first durable context service")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve,
    Worker,
    Migrate,
    Healthcheck,
    ObjectStoreCheck,
    ObjectStoreBackup {
        #[command(subcommand)]
        command: ObjectStoreBackupCommand,
    },
    Operator {
        #[command(subcommand)]
        command: OperatorCommand,
    },
}

#[derive(Debug, Subcommand)]
enum OperatorCommand {
    ProvisionUser {
        #[arg(long)]
        external_ref: String,
        #[arg(long)]
        display_name: String,
        #[arg(long, default_value = "Initial owner")]
        credential_name: String,
    },
    RecoverUser {
        #[arg(long)]
        user_id: String,
        #[arg(long, default_value = "Recovered owner")]
        credential_name: String,
        #[arg(long)]
        revoke_existing_owner_credentials: bool,
    },
    RecordBackupWatermark {
        #[arg(long)]
        oldest_retained_created_at: String,
        #[arg(long)]
        receipt_sha256: String,
        #[arg(long)]
        source: String,
    },
}

#[derive(Debug, Subcommand)]
enum ObjectStoreBackupCommand {
    Export {
        #[arg(long)]
        output: PathBuf,
    },
    Verify {
        #[arg(long)]
        input: PathBuf,
    },
    Restore {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        mapping: PathBuf,
    },
    CleanupRestore {
        #[arg(long)]
        mapping: PathBuf,
    },
    RemapDatabase {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        mapping: PathBuf,
    },
    PinDatabase,
    VerifyDatabase,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let cli = Cli::parse();
    let component = match &cli.command {
        Command::Serve => "api",
        Command::Worker => "worker",
        Command::Migrate => "migrate",
        Command::Healthcheck => "healthcheck",
        Command::ObjectStoreCheck => "operator",
        Command::ObjectStoreBackup { .. } => "operator",
        Command::Operator { .. } => "operator",
    };
    let metrics_enabled = match telemetry::init(component) {
        Ok(enabled) => enabled,
        Err(error) => {
            tracing::warn!(%error, component, "Datadog metrics exporter is disabled");
            false
        }
    };
    if matches!(&cli.command, Command::Healthcheck) {
        let url = std::env::var("STRAYLIGHT_HEALTHCHECK_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8080/health".to_owned());
        let response = reqwest::get(url).await?;
        if !response.status().is_success() {
            return Err(format!("healthcheck returned {}", response.status()).into());
        }
        return Ok(());
    }
    if let Command::ObjectStoreBackup {
        command: ObjectStoreBackupCommand::Verify { input },
    } = &cli.command
    {
        let manifest = object_backup::verify(input).await?;
        println!("{}", serde_json::to_string(&manifest.summary)?);
        return Ok(());
    }
    if let Command::ObjectStoreBackup {
        command: ObjectStoreBackupCommand::RemapDatabase { input, mapping },
    } = &cli.command
    {
        let config = Config::from_env()?;
        let store = ObjectStore::new(&config).await?;
        let verified =
            object_backup::verify_restore_for_database_remap(&store, input, mapping).await?;
        let database_url = Config::admin_database_url_from_env()?;
        let result = object_backup::remap_database(&database_url, &verified).await?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }
    let config = Config::from_env()?;
    match cli.command {
        Command::Migrate => {
            db::migrate_and_bootstrap(&config).await?;
            tracing::info!("database migrations and local bootstrap complete");
        }
        Command::Serve => {
            let bind = config.bind;
            let state = AppState::connect(config).await?;
            if metrics_enabled {
                telemetry::spawn_runtime_metrics(state.clone());
            }
            let listener = TcpListener::bind(bind).await?;
            tracing::info!(%bind, "Straylight API listening");
            axum::serve(listener, router(state)).await?;
        }
        Command::Worker => {
            let state = AppState::connect(config).await?;
            if metrics_enabled {
                telemetry::spawn_runtime_metrics(state.clone());
            }
            worker::run(state).await?;
        }
        Command::ObjectStoreCheck => {
            let result = ObjectStore::new(&config).await?.qualify().await?;
            println!("{}", serde_json::to_string(&result)?);
        }
        Command::ObjectStoreBackup { command } => {
            let store = ObjectStore::new(&config).await?;
            match command {
                ObjectStoreBackupCommand::Export { output } => {
                    let manifest = object_backup::export(&store, &output).await?;
                    println!("{}", serde_json::to_string(&manifest.summary)?);
                }
                ObjectStoreBackupCommand::Restore { input, mapping } => {
                    let result = object_backup::restore(&store, &input, &mapping).await?;
                    println!("{}", serde_json::to_string(&result.summary)?);
                }
                ObjectStoreBackupCommand::CleanupRestore { mapping } => {
                    object_backup::cleanup_restore(&store, &mapping).await?;
                    println!(
                        "{}",
                        serde_json::json!({"status": "clean", "mapping": mapping})
                    );
                }
                ObjectStoreBackupCommand::PinDatabase => {
                    let database_url = Config::admin_database_url_from_env()?;
                    let result =
                        object_backup::pin_legacy_database_references(&database_url, &store)
                            .await?;
                    println!("{}", serde_json::to_string(&result)?);
                }
                ObjectStoreBackupCommand::VerifyDatabase => {
                    let database_url = Config::admin_database_url_from_env()?;
                    let result =
                        object_backup::verify_database_references(&database_url, &store).await?;
                    println!("{}", serde_json::to_string(&result)?);
                }
                ObjectStoreBackupCommand::Verify { .. }
                | ObjectStoreBackupCommand::RemapDatabase { .. } => unreachable!(),
            }
        }
        Command::Operator { command } => {
            let database_url = Config::admin_database_url_from_env()?;
            let result = match command {
                OperatorCommand::ProvisionUser {
                    external_ref,
                    display_name,
                    credential_name,
                } => {
                    operator_service::provision_user(
                        &database_url,
                        &external_ref,
                        &display_name,
                        &credential_name,
                    )
                    .await?
                }
                OperatorCommand::RecoverUser {
                    user_id,
                    credential_name,
                    revoke_existing_owner_credentials,
                } => {
                    operator_service::recover_user(
                        &database_url,
                        &user_id,
                        &credential_name,
                        revoke_existing_owner_credentials,
                    )
                    .await?
                }
                OperatorCommand::RecordBackupWatermark {
                    oldest_retained_created_at,
                    receipt_sha256,
                    source,
                } => {
                    operator_service::record_backup_watermark(
                        &database_url,
                        &oldest_retained_created_at,
                        &receipt_sha256,
                        &source,
                    )
                    .await?
                }
            };
            println!("{}", serde_json::to_string(&result)?);
        }
        Command::Healthcheck => unreachable!(),
    }
    Ok(())
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn"));
    let query_count_layer =
        || QueryCountLayer.with_filter(filter_fn(|metadata| metadata.target() == "sqlx::query"));
    match UdpLogMakeWriter::from_env() {
        Ok(Some(datadog)) => tracing_subscriber::registry()
            .with(query_count_layer())
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(std::io::stdout.and(datadog))
                    .with_filter(filter),
            )
            .init(),
        Ok(None) => tracing_subscriber::registry()
            .with(query_count_layer())
            .with(tracing_subscriber::fmt::layer().json().with_filter(filter))
            .init(),
        Err(error) => {
            eprintln!("Datadog log forwarding is disabled: {error}");
            tracing_subscriber::registry()
                .with(query_count_layer())
                .with(tracing_subscriber::fmt::layer().json().with_filter(filter))
                .init();
        }
    }
}
