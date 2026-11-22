# Bentrix SDK for Rust

The Rust client of [Bentrix](https://bentrix.net) — charts, a replay laboratory and trading through your own MetaTrader — for a program or an agent.

It finds the Bentrix running on this computer through the agents folder Bentrix writes at every start, pairs with it under the name you give — the name you see in ☰ › Settings › **Agents**, where the pairing's permissions are set — and calls every tool of Bentrix's capability catalogue over MCP. Nothing is typed: no address, no key.

The client is generated from the catalogue of the Bentrix release it came with, and checks when it connects that the running Bentrix serves the same major version.

## What you need

Rust (stable), and Bentrix open on the same computer.

## Install

The client is the crate `bentrix-mcp-client`. Add it to your `Cargo.toml`:

```toml
[dependencies]
bentrix-mcp-client = { git = "https://github.com/bentrixhq/bentrix-rust" }
serde_json = "1"
```

## Connect

```rust
use bentrix_mcp_client as mcp;

fn main() -> Result<(), mcp::Error> {
    let b = mcp::connect(Some("my-agent"))?;
    let state = b.workspace().get_state(mcp::WorkspaceGetStateArgs::new())?;
    let bench = state["laboratories"][0]["benches"][0]["uri"].as_str().unwrap_or_default().to_string();
    let p: Vec<&str> = bench.trim_start_matches("bentrix://").split('/').collect();
    let book = b.trade().book(mcp::TradeBookArgs::new(p[0], p[1], p[3]))?;
    println!("{}", book["positions"]);
    Ok(())
}
```

Every tool is a method of its domain (`b.workspace()`, `b.chart()`, `b.trade()` …) taking one arguments value built with
`new` and its setters. A refusal is `mcp::Error` carrying Bentrix's own sentence.

## Documentation

Every method, its arguments, the permission it needs and what it answers: **https://bentrix.net/en/docs/sdk/rust/**

## Licence

MIT — see [LICENSE](LICENSE). It covers this source code only. Bentrix itself is not covered by it: the Bentrix application is Bentrix's own and distributed under its own terms.
