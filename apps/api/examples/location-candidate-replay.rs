//! Offline preflight of an actual frozen admission and model candidate output.
use brunn::dreamer::prompt;
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("supply frozen-admission.json and dream.candidates.v1.json".into());
    }
    let admission: Value = serde_json::from_str(&std::fs::read_to_string(&args[0])?)?;
    let output: Value = serde_json::from_str(&std::fs::read_to_string(&args[1])?)?;
    let issues = prompt::location_submission_issues(&output, &admission);
    println!("{}", json!({"valid":issues.is_empty(),"issues":issues}));
    Ok(())
}
