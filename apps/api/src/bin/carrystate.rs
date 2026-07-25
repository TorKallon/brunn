#[tokio::main]
async fn main() {
    if let Err(error) = straylight::carrystate_cli::run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
