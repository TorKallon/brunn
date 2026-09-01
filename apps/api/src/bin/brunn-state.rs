#[tokio::main]
async fn main() {
    if let Err(error) = brunn::brunn_state_cli::run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
