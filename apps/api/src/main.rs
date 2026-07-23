use clap::{Parser, Subcommand};
use straylight::{AppState, Config, db, router, worker};
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let cli = Cli::parse();
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
            let listener = TcpListener::bind(bind).await?;
            tracing::info!(%bind, "Straylight API listening");
            axum::serve(listener, router(state)).await?;
        }
        Command::Worker => {
            let state = AppState::connect(config).await?;
            worker::run(state).await?;
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
