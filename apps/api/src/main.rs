use clap::{Parser, Subcommand};
use straylight::{
    AppState, Config, db, object_store::ObjectStore, operator_service, router, telemetry, worker,
};
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

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
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().json())
        .init();
}
