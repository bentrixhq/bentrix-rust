//! The Rust MCP client's walk against a running BENTRIX (`bentrix --serve` over the CI tape): it finds
//! the application and pairs with nothing typed, reads its state through the generated surface, reads a
//! resource by its template, and runs the same Gym episode every binding runs — through the agents'
//! tools, over the wire — printed as JSON for sdk/python/tests/test_crosslang.py.
//!
//!     LOCALAPPDATA=<the serve's> cargo run -p bentrix-mcp-client --example mcpwalk

use bentrix_mcp_client as mcp;
use serde_json::{json, Value};

fn check(held: bool, what: &str) -> Result<(), mcp::Error> {
    if held {
        Ok(())
    } else {
        Err(mcp::Error::Other(format!("FAILED: {what}")))
    }
}

fn main() -> Result<(), mcp::Error> {
    let b = mcp::connect(Some("rust-walk"))?;
    let state = b.workspace().get_state(mcp::WorkspaceGetStateArgs::new())?;
    let bench = state["laboratories"][0]["benches"][0]["uri"].as_str().unwrap_or_default().to_string();
    let p: Vec<&str> = bench.trim_start_matches("bentrix://").split('/').collect();
    let book = b.trade().book(mcp::TradeBookArgs::new(p[0], p[1], p[3]))?;
    check(book["positions"].is_array(), &format!("the book did not read: {book}"))?;
    let said = b.call("no_such_tool", Value::Null).err().map(|e| e.to_string()).unwrap_or_default();
    check(said.contains("no_such_tool"), &format!("a refusal did not say its tool: {said}"))?;

    let made = b.strategy().env_make(mcp::StrategyEnvMakeArgs::new(bench.clone()).interval("H1").window(8).episode("window").episode_bars(12))?;
    let env = made["env"].as_str().unwrap_or_default().to_string();
    let start = b.strategy().env_reset(mcp::StrategyEnvResetArgs::new(env.clone()).seed(5))?;
    check(start["obs"]["market"].as_array().map(Vec::len) == Some(8), "the observation is not shaped as declared")?;
    let mut rewards = Vec::new();
    let mut hashes = vec![start["info"]["ledger_hash"].clone()];
    for a in [1, 1, 2, 0, 2, 2, 1, 0, 1, 2, 0, 1] {
        let s = b.strategy().env_step(mcp::StrategyEnvStepArgs::new(env.clone(), a))?;
        rewards.push(s["reward"].as_f64().unwrap_or(f64::NAN));
        hashes.push(s["info"]["ledger_hash"].clone());
        if s["terminated"] == true || s["truncated"] == true {
            break;
        }
    }
    b.strategy().env_close(mcp::StrategyEnvCloseArgs::new(env.clone()))?;
    // A tool's own refusal (isError), not a request the wire refused.
    let refusal = b.strategy().env_step(mcp::StrategyEnvStepArgs::new(env, 0)).err();
    check(matches!(&refusal, Some(mcp::Error::Tool { message, .. }) if message.contains("No environment")), &format!("a closed environment's step was not refused in its words: {refusal:?}"))?;
    check(hashes.iter().any(|h| *h != hashes[0]), "the episode traded nothing")?;
    // serde_json writes an f64 in the shortest digits that read back as the same double.
    println!("{}", json!({"start_ms": start["info"]["start_ms"], "rewards": rewards, "hashes": hashes}));
    println!("PASS");
    Ok(())
}
