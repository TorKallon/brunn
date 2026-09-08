//! Operator replay of model-selected public lookups through the real verifier.
//! No Brunn credentials, model call, source selection, or production write.
use brunn::dreamer::discovery;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<_> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        return Err("supply saved dream.location.discovery.v1 JSON outputs".into());
    }
    let mut replays = Vec::new();
    for path in paths {
        let source =
            discovery::parse(&std::fs::read_to_string(&path)?).map_err(std::io::Error::other)?;
        let (verified, failures) = discovery::verify_lookups(&source.lookups).await;
        replays.push(json!({"input":path,"requested":source.lookups.len(),"verified":verified,"failures":failures}));
    }
    println!("{}", serde_json::to_string_pretty(&replays)?);
    Ok(())
}
