//! **Bentrix's MCP client for Rust**:
//! every capability of a running Bentrix over its Model Context Protocol endpoint. This file is the
//! hand-written half — the connection and everything that does not change when a capability is added;
//! `generated.rs` is the rest, from the capability catalogue (`bentrix-capabilities` `rssdk`).
//!
//! It speaks MCP revision 2026-07-28 over Streamable HTTP: every request is its own POST carrying the
//! protocol version, the client's identity and its (empty) capabilities in `_meta` — the same wire as
//! the Python client's `_runtime.py` and the JavaScript client's `client.js`, field for field. The
//! endpoint is always `http://127.0.0.1:<port>` (the endpoint file refuses anything else), and the
//! server answers one request per connection and closes it, so HTTP here is `std::net` and a few
//! lines: no TLS, no async runtime, `serde_json` the one dependency.
//!
//! ```no_run
//! let b = bentrix::connect(Some("my-agent"))?;
//! let state = b.workspace().get_state(bentrix::WorkspaceGetStateArgs::new())?;
//! let bench = state["laboratories"][0]["benches"][0]["uri"].as_str().unwrap_or_default().to_string();
//! let made = b.strategy().env_make(bentrix::StrategyEnvMakeArgs::new(bench).window(8))?;
//! # Ok::<(), bentrix::Error>(())
//! ```

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::Instant;

use serde_json::{json, Map, Value};

#[rustfmt::skip]
#[allow(clippy::new_without_default, clippy::too_many_arguments)]
mod generated;
#[rustfmt::skip]
#[allow(clippy::new_without_default)]
mod bench;
pub mod forecast;
pub mod output;
pub use generated::*;
pub use bench::*;

pub const PROTOCOL_VERSION: &str = "2026-07-28";
pub const CLIENT_NAME: &str = "bentrix-rust-sdk";
pub const CLIENT_VERSION: &str = "0.1.0";
pub const ENDPOINT_FILE: &str = "endpoint.json";
/// A call that asks the server to wait is given that long and this much more (found 2026-11-02:
/// `wait_for` beyond the client's own timeout ended in the client before the server could answer).
pub const LONG_POLL_MARGIN: Duration = Duration::from_secs(15);
pub const SECRET_FILE: &str = "local.secret";

/// **Busy, try again**: the JSON-RPC code Bentrix answers when it refused a request BEFORE doing
/// anything with it — one program's 20 calls a second spent, too many connections or streams, a task
/// it could not read in time. Nothing ran, so the same request is sent again after the wait the answer
/// names (`retryAfterMs`, a second when none), at most [`BUSY_RETRIES`] times and each wait at most
/// [`BUSY_WAIT_MAX`]; then the refusal is returned. An order is never sent twice by this.
pub const BUSY: i64 = -40001;
pub const BUSY_RETRIES: u32 = 10;
pub const BUSY_WAIT_MAX: Duration = Duration::from_secs(5);

/// The wait a busy answer's `data` names, bounded.
fn busy_wait(data: &Value) -> Duration {
    let ms = data["retryAfterMs"].as_f64().filter(|ms| *ms > 0.0).unwrap_or(1000.0);
    Duration::from_secs_f64(ms / 1000.0).min(BUSY_WAIT_MAX)
}

/// Why a call failed, in Bentrix's own words where it gave them.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// A JSON-RPC error: the request itself was refused.
    Rpc { code: i64, message: String },
    /// A tool ran and answered `isError`: the product's own sentence, verbatim; `machine` is the
    /// server's word for the kind (`busy`, `expired`, `insufficient_scope`...) when it gave one.
    Tool { tool: String, message: String, machine: Option<String> },
    /// **A stream's clock went back behind its cursor** (a laboratory's rewind): what came after it
    /// will come again. [`Watch`] ends with this rather than handing those items twice as if new;
    /// `cursor` is where to read on from.
    Rewound { uri: String, cursor: Option<String> },
    NotRunning(String),
    PairingRefused(String),
    IncompatibleCatalogue(String),
    JobFailed(String),
    /// The connection, or anything else.
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Rpc { code, message } => write!(f, "[{code}] {message}"),
            Error::Tool { tool, message, .. } => write!(f, "{tool}: {message}"),
            Error::Rewound { uri, cursor } => write!(f, "{uri}: the workbench's clock went back; read on from {}", cursor.as_deref().unwrap_or("the present")),
            Error::NotRunning(m) | Error::PairingRefused(m) | Error::IncompatibleCatalogue(m) | Error::JobFailed(m) | Error::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

/// `host:port` and the path of an `http://` URL.
fn split_url(url: &str) -> Result<(String, String), Error> {
    let rest = url.strip_prefix("http://").ok_or_else(|| Error::Other(format!("{url} is not an http:// URL")))?;
    match rest.find('/') {
        Some(i) => Ok((rest[..i].to_string(), rest[i..].to_string())),
        None => Ok((rest.to_string(), "/".to_string())),
    }
}

/// One POST: `(status, content type, body)`. The server closes the connection after its answer, so
/// the body is everything to the close (cut at a `Content-Length` when one is given).
fn post(url: &str, body: &str, headers: &[(&str, String)], timeout: Duration) -> Result<(u16, String, String), Error> {
    let (host, path) = split_url(url)?;
    let io = |e: std::io::Error| Error::Other(format!("{url}: {e}"));
    let mut s = TcpStream::connect(&host).map_err(io)?;
    s.set_read_timeout(Some(timeout)).map_err(io)?;
    let mut req = format!("POST {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).map_err(io)?;
    s.write_all(body.as_bytes()).map_err(io)?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(io)?;
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| Error::Other(format!("{url}: a response with no head")))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut payload = raw[split + 4..].to_vec();
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let header = |name: &str| head.lines().skip(1).find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()));
    if let Some(n) = header("content-length").and_then(|n| n.parse::<usize>().ok()) {
        payload.truncate(n);
    } else if header("transfer-encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
        payload = dechunk(&payload);
    }
    Ok((status, header("content-type").unwrap_or_default(), String::from_utf8_lossy(&payload).to_string()))
}

/// A chunked body, reassembled.
fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = b.windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(String::from_utf8_lossy(&b[..i]).split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        if size == 0 || b.len() < i + 2 + size {
            break;
        }
        out.extend_from_slice(&b[i + 2..i + 2 + size]);
        b = &b[(i + 4 + size).min(b.len())..];
    }
    out
}

/// A header value, or the standard Base64 sentinel when plain ASCII cannot carry it.
fn header_value(v: &str) -> String {
    let safe = v.bytes().all(|c| (0x20..=0x7e).contains(&c)) && v == v.trim() && !(v.starts_with("=?base64?") && v.ends_with("?="));
    if safe {
        v.to_string()
    } else {
        format!("=?base64?{}?=", base64(v.as_bytes()))
    }
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in b.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            out.push(if k <= chunk.len() { T[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Leave out what is null, so the server's own default applies.
fn plain(args: Value) -> Value {
    match args {
        Value::Object(m) => Value::Object(m.into_iter().filter(|(_, v)| !v.is_null()).collect()),
        Value::Null => Value::Object(Map::new()),
        other => other,
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn escape(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// **A workbench's address stands for its three parts**: `bench` given as a
/// `bentrix://<home>/<owner>/bench/<name>` address fills `home`, `owner` and `bench`. Parts given as before are left
/// as they are.
fn from_address(mut values: Value) -> Value {
    let parts: Option<Vec<String>> = values
        .get("bench")
        .and_then(Value::as_str)
        .and_then(|s| s.strip_prefix("bentrix://"))
        .map(|rest| rest.split('/').map(str::to_owned).collect());
    if let (Some(p), Some(m)) = (parts, values.as_object_mut()) {
        if p.len() >= 4 && p[2] == "bench" {
            m.insert("home".into(), Value::from(p[0].clone()));
            m.insert("owner".into(), Value::from(p[1].clone()));
            m.insert("bench".into(), Value::from(p[3].clone()));
        }
    }
    values
}

/// The RFC 6570 subset the catalogue uses: `{name}` and `{?a,b}`. A null optional is left out.
fn expand_template(template: &str, vars: &Value) -> Result<String, Error> {
    let values = from_address(plain(vars.clone()));
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').map(|c| open + c).ok_or_else(|| Error::Other(format!("{template} is not a URI template")))?;
        let inner = &rest[open + 1..close];
        if let Some(op) = inner.chars().next().filter(|c| *c == '?' || *c == '&') {
            let pairs: Vec<String> = inner[1..].split(',').filter_map(|n| values.get(n).map(|v| format!("{n}={}", escape(&scalar(v))))).collect();
            if !pairs.is_empty() {
                out.push(op);
                out.push_str(&pairs.join("&"));
            }
        } else {
            let v = values.get(inner).ok_or_else(|| Error::Other(format!("the URI template {template} needs {inner}")))?;
            out.push_str(&escape(&scalar(v)));
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The final response on a request-scoped event stream; notifications before it are ignored.
fn read_sse(text: &str, id: u64) -> Result<Value, Error> {
    let mut data: Vec<&str> = Vec::new();
    for line in text.split('\n').chain(std::iter::once("")) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if !data.is_empty() {
                if let Ok(msg) = serde_json::from_str::<Value>(&data.join("\n")) {
                    if msg["id"].as_u64() == Some(id) && (msg.get("result").is_some() || msg.get("error").is_some()) {
                        return Ok(msg);
                    }
                }
            }
            data.clear();
        } else if let Some(d) = line.strip_prefix("data:") {
            data.push(d.trim_start());
        }
    }
    Err(Error::Other("the stream ended without a response".into()))
}

/// **One Bentrix endpoint**: every capability of its catalogue under `b.<domain>()`.
pub struct Client {
    pub url: String,
    pub catalogue_version: String,
    token: String,
    id: AtomicU64,
    timeout: Duration,
}

impl Client {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Client {
        Client { url: url.into(), catalogue_version: CATALOGUE_VERSION.to_string(), token: token.into(), id: AtomicU64::new(0), timeout: Duration::from_secs(60) }
    }

    fn rpc(&self, method: &str, params: Map<String, Value>, name: Option<&str>) -> Result<Value, Error> {
        self.rpc_timed(method, params, name, self.timeout)
    }

    /// One request, sent again while Bentrix answers busy ([`BUSY`]): it ran nothing.
    fn rpc_timed(&self, method: &str, params: Map<String, Value>, name: Option<&str>, timeout: Duration) -> Result<Value, Error> {
        let mut attempt = 0;
        loop {
            match self.rpc_once(method, params.clone(), name, timeout) {
                Err((e, Some(wait))) if attempt < BUSY_RETRIES => {
                    let _ = e;
                    attempt += 1;
                    std::thread::sleep(wait);
                }
                Err((e, _)) => return Err(e),
                Ok(v) => return Ok(v),
            }
        }
    }

    /// One request; a refusal carries the wait it names when it is busy.
    fn rpc_once(&self, method: &str, mut params: Map<String, Value>, name: Option<&str>, timeout: Duration) -> Result<Value, (Error, Option<Duration>)> {
        let id = self.id.fetch_add(1, Ordering::SeqCst) + 1;
        params.insert(
            "_meta".into(),
            json!({
                "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
                "io.modelcontextprotocol/clientInfo": {"name": CLIENT_NAME, "version": CLIENT_VERSION},
                "io.modelcontextprotocol/clientCapabilities": {},
            }),
        );
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        let mut headers = vec![
            ("Content-Type", "application/json".to_string()),
            ("Accept", "application/json, text/event-stream".to_string()),
            ("MCP-Protocol-Version", PROTOCOL_VERSION.to_string()),
            ("Mcp-Method", method.to_string()),
            ("Authorization", format!("Bearer {}", self.token)),
        ];
        if let Some(n) = name {
            headers.push(("Mcp-Name", header_value(n)));
        }
        let (status, ctype, text) = post(&self.url, &body, &headers, timeout.max(self.timeout)).map_err(|e| (e, None))?;
        let msg = if ctype.starts_with("text/event-stream") {
            read_sse(&text, id).map_err(|e| (e, None))?
        } else {
            serde_json::from_str::<Value>(&text).map_err(|_| (Error::Rpc { code: status as i64, message: format!("HTTP {status} with no JSON-RPC body") }, None))?
        };
        if let Some(err) = msg.get("error").filter(|e| !e.is_null()) {
            let code = err["code"].as_i64().unwrap_or(status as i64);
            let wait = (code == BUSY).then(|| busy_wait(&err["data"]));
            return Err((Error::Rpc { code, message: err["message"].as_str().unwrap_or_default().to_string() }, wait));
        }
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    }

    /// A tool by its wire name, its arguments a JSON object; null arguments are left out.
    pub fn call(&self, name: &str, args: Value) -> Result<Value, Error> {
        let mut params = Map::new();
        params.insert("name".into(), Value::from(name));
        let args = plain(args);
        let wait = args["timeout_ms"].as_f64().map(|ms| Duration::from_secs_f64(ms.max(0.0) / 1000.0) + LONG_POLL_MARGIN).unwrap_or_default();
        params.insert("arguments".into(), args);
        let res = self.rpc_timed("tools/call", params, Some(name), wait)?;
        if res["resultType"] == "input_required" {
            return Err(Error::Other(format!("{name} needs input this client cannot give; answer it in Bentrix's window")));
        }
        let text: String = res["content"].as_array().map(|parts| parts.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str()).collect()).unwrap_or_default();
        if res["isError"] == true {
            let machine = res["_meta"]["bentrix/machine"].as_str().map(str::to_owned);
            return Err(Error::Tool { tool: name.to_string(), message: if text.is_empty() { "the tool failed".into() } else { text }, machine });
        }
        if let Some(sc) = res.get("structuredContent").filter(|v| !v.is_null()) {
            return Ok(sc.clone());
        }
        Ok(if text.is_empty() { Value::Object(Map::new()) } else { serde_json::from_str(&text).unwrap_or_else(|_| Value::Object(Map::new())) })
    }

    /// A long operation: a [`Job`] whether the server answered at once or with a handle.
    pub fn call_job(&self, name: &str, args: Value) -> Result<Job<'_>, Error> {
        let raw = self.call(name, args)?;
        let handle = raw.as_object().filter(|m| m.len() == 1).and_then(|m| m.get("job")).and_then(Value::as_str).map(str::to_string);
        Ok(match handle {
            Some(h) => Job { c: self, handle: Some(h), result: Value::Null },
            None => Job { c: self, handle: None, result: raw },
        })
    }

    fn read_uri(&self, uri: &str) -> Result<Value, Error> {
        let mut params = Map::new();
        params.insert("uri".into(), Value::from(uri));
        let res = self.rpc("resources/read", params, Some(uri))?;
        let text = res["contents"][0]["text"].as_str().ok_or_else(|| Error::Other(format!("{uri} answered no text content")))?;
        serde_json::from_str(text).map_err(|e| Error::Other(format!("{uri}: {e}")))
    }

    /// A resource by its URI template and variables.
    pub fn read_resource(&self, template: &str, vars: Value) -> Result<Value, Error> {
        self.read_uri(&expand_template(template, &vars)?)
    }

    /// Every page of a cursor-paged resource, lazily: `next` is the URI of the following page.
    pub fn read_pages(&self, template: &str, vars: Value) -> Pages<'_> {
        match expand_template(template, &vars) {
            Ok(uri) => Pages { c: self, next: Some(uri), failed: None },
            Err(e) => Pages { c: self, next: None, failed: Some(e) },
        }
    }

    /// Items of a stream, forever, lazily: `wait_for` long-polls, a page is `{items, next}`.
    pub fn watch(&self, template: &str, vars: Value, after: Option<String>, timeout_ms: u64) -> Watch<'_> {
        let uri = expand_template(template, &vars);
        Watch { c: self, uri, cursor: after, timeout_ms: if timeout_ms == 0 { 30_000 } else { timeout_ms }, pending: Vec::new(), rewound: false, over: false }
    }

    /// Refuse a server on another major line of the catalogue.
    pub fn verify_catalogue(&self) -> Result<String, Error> {
        let server = self.call("agent_whoami", Value::Null)?["catalogue_version"].as_str().unwrap_or_default().to_string();
        let major = |v: &str| -> Result<u64, Error> {
            let p: Vec<&str> = v.split('.').collect();
            match p.as_slice() {
                [a, b, c] if [a, b, c].iter().all(|x| !x.is_empty() && x.bytes().all(|d| d.is_ascii_digit())) => a.parse().map_err(|_| Error::IncompatibleCatalogue(format!("{v} is not a version major.minor.patch"))),
                _ => Err(Error::IncompatibleCatalogue(format!("{v} is not a version major.minor.patch"))),
            }
        };
        if major(&server)? != major(&self.catalogue_version)? {
            return Err(Error::IncompatibleCatalogue(format!("this SDK was generated from catalogue {}; the server's is {server}", self.catalogue_version)));
        }
        Ok(server)
    }
}

/// A long operation: [`Job::wait`] answers its result once it completes (the person may act first).
pub struct Job<'a> {
    c: &'a Client,
    pub handle: Option<String>,
    result: Value,
}

impl Job<'_> {
    pub fn status(&self) -> Result<Value, Error> {
        match &self.handle {
            None => Ok(json!({"status": "completed"})),
            Some(h) => self.c.call("job_get", json!({"job": h})),
        }
    }

    pub fn cancel(&self) -> Result<(), Error> {
        if let Some(h) = &self.handle {
            self.c.call("job_cancel", json!({"job": h}))?;
        }
        Ok(())
    }

    /// Poll until the job completes or fails; `None` waits as long as it takes.
    pub fn wait(&self, timeout: Option<Duration>, poll: Duration) -> Result<Value, Error> {
        let Some(h) = &self.handle else { return Ok(self.result.clone()) };
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let st = self.status()?;
            match st["status"].as_str() {
                Some("completed") => return Ok(st.get("result").cloned().filter(|r| !r.is_null()).unwrap_or_else(|| Value::Object(Map::new()))),
                Some(s @ ("failed" | "cancelled")) => return Err(Error::JobFailed(format!("job {h} {s}: {}", st["message"].as_str().unwrap_or_default()))),
                other => {
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        return Err(Error::Other(format!("job {h} is still {}", other.unwrap_or("?"))));
                    }
                }
            }
            std::thread::sleep(poll);
        }
    }
}

/// Every page of a cursor-paged resource.
pub struct Pages<'a> {
    c: &'a Client,
    next: Option<String>,
    failed: Option<Error>,
}

impl Iterator for Pages<'_> {
    type Item = Result<Value, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(e) = self.failed.take() {
            return Some(Err(e));
        }
        let uri = self.next.take().filter(|u| !u.is_empty())?;
        Some(self.c.read_uri(&uri).inspect(|page| self.next = page["next"].as_str().map(str::to_string)))
    }
}

/// The items of a stream, forever.
pub struct Watch<'a> {
    c: &'a Client,
    uri: Result<String, Error>,
    cursor: Option<String>,
    timeout_ms: u64,
    pending: Vec<Value>,
    /// Said once the pending items are handed: the stream's clock went back.
    rewound: bool,
    /// Ended (a rewind said, or an error that is not passing): nothing more is read.
    over: bool,
}

/// A stream's call answered with one of these is asked again, after a pause.
const PASSING: [&str; 2] = ["busy", "expired"];
const PASSING_TRIES: u32 = 5;
const PASSING_PAUSE: Duration = Duration::from_millis(500);

impl Iterator for Watch<'_> {
    type Item = Result<Value, Error>;

    /// The next item. A call answered `busy` or `expired` is asked again (five times, half a second
    /// apart) rather than ending the stream; a page saying `rewound` ends it with
    /// [`Error::Rewound`] after the items before it, carrying the cursor to read on from (the strict
    /// audit of 2026-11-04, S3/S4). After an error the iterator is over.
    fn next(&mut self) -> Option<Self::Item> {
        if self.over {
            return None;
        }
        let uri = match &self.uri {
            Ok(u) => u.clone(),
            Err(e) => {
                self.over = true;
                return Some(Err(e.clone()));
            }
        };
        while self.pending.is_empty() {
            if self.rewound {
                self.over = true;
                return Some(Err(Error::Rewound { uri, cursor: self.cursor.clone() }));
            }
            let mut tries = 0;
            let page = loop {
                match self.c.call("wait_for", json!({"uri": uri, "timeout_ms": self.timeout_ms, "after": self.cursor})) {
                    Ok(p) => break p,
                    Err(Error::Tool { machine: Some(m), .. }) if PASSING.contains(&m.as_str()) && tries + 1 < PASSING_TRIES => {
                        tries += 1;
                        std::thread::sleep(PASSING_PAUSE);
                    }
                    Err(e) => {
                        self.over = true;
                        return Some(Err(e));
                    }
                }
            };
            self.pending = page["items"].as_array().cloned().unwrap_or_default();
            self.pending.reverse();
            if let Some(n) = page["next"].as_str() {
                self.cursor = Some(n.to_string());
            }
            self.rewound = page["rewound"] == true;
        }
        self.pending.pop().map(Ok)
    }
}

/// The folder Bentrix writes its endpoint and secret into — `user_dir(Data, "agents")`, as the
/// application has it.
pub fn agents_dir() -> PathBuf {
    if let Some(lad) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(lad).join("BENTRIX").join("agents");
    }
    match std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        Some(home) => PathBuf::from(home).join(".local").join("share").join("bentrix").join("agents"),
        None => PathBuf::from("bentrix-agents"),
    }
}

#[cfg(unix)]
fn alive(pid: i64) -> bool {
    // /proc where there is one; elsewhere the request itself says a stale endpoint is refused.
    let proc = Path::new("/proc");
    !proc.exists() || proc.join(pid.to_string()).exists()
}

#[cfg(not(unix))]
fn alive(_pid: i64) -> bool {
    true
}

/// The running application's URL, from its endpoint file.
pub fn read_endpoint(folder: &Path) -> Result<String, Error> {
    let file = folder.join(ENDPOINT_FILE);
    let doc: Value = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or_else(|| Error::NotRunning(format!("Bentrix is not running: there is no readable {ENDPOINT_FILE} in {}", folder.display())))?;
    let (Some(url), Some(pid)) = (doc["url"].as_str().filter(|u| u.starts_with("http://127.0.0.1:")), doc["pid"].as_i64()) else {
        return Err(Error::NotRunning(format!("Bentrix is not running: {} is not an endpoint file", file.display())));
    };
    if !alive(pid) {
        return Err(Error::NotRunning(format!("Bentrix is not running: {} is stale (process {pid} has ended)", file.display())));
    }
    Ok(url.to_string())
}

/// The secret Bentrix wrote at its start.
pub fn read_secret(folder: &Path) -> Result<String, Error> {
    let secret = std::fs::read_to_string(folder.join(SECRET_FILE)).map_err(|_| Error::PairingRefused(format!("there is no readable {SECRET_FILE} in {}: Bentrix writes it at every start", folder.display())))?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        return Err(Error::PairingRefused(format!("{SECRET_FILE} is empty")));
    }
    Ok(secret)
}

/// The name this process pairs under: `BENTRIX_AGENT_NAME`, else the program's name.
pub fn default_name() -> String {
    if let Some(n) = std::env::var("BENTRIX_AGENT_NAME").ok().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
        return n;
    }
    match std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string())) {
        Some(p) => format!("{CLIENT_NAME} ({p})"),
        None => CLIENT_NAME.to_string(),
    }
}

/// Present the secret to `url/pair` and answer the token. The secret travels in this one body.
pub fn pair(url: &str, secret: &str, name: &str) -> Result<String, Error> {
    let body = json!({"secret": secret, "client": {"name": name}}).to_string();
    let (status, _, text) = post(&format!("{}/pair", url.trim_end_matches('/')), &body, &[("Content-Type", "application/json".to_string())], Duration::from_secs(60))?;
    let doc: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        let code = doc["error"]["code"].as_str().unwrap_or_default();
        if code == "wrong_secret" {
            return Err(Error::PairingRefused("Bentrix refused the secret in the secret file: it writes a new secret at every start".into()));
        }
        return Err(Error::PairingRefused(format!("Bentrix answered the pairing request with HTTP {status} {code}: {}", doc["error"]["message"].as_str().unwrap_or_default())));
    }
    doc["token"].as_str().filter(|t| !t.is_empty()).map(str::to_string).ok_or_else(|| Error::PairingRefused("Bentrix answered the pairing request with no token".into()))
}

/// Find the running application, pair under `name` (`None`: [`default_name`]) and connect.
pub fn connect(name: Option<&str>) -> Result<Client, Error> {
    connect_in(&agents_dir(), name)
}

/// [`connect`] through a given agents folder.
pub fn connect_in(folder: &Path, name: Option<&str>) -> Result<Client, Error> {
    let url = read_endpoint(folder)?;
    let token = pair(&url, &read_secret(folder)?, &name.map(str::to_string).unwrap_or_else(default_name))?;
    Ok(Client::new(url, token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_is_expanded_and_an_absent_optional_left_out() {
        let v = json!({"home": "lab", "owner": "a b", "n": 3, "x": null});
        assert_eq!(expand_template("bentrix://{home}/{owner}/x{?n,x,y}", &v).unwrap(), "bentrix://lab/a%20b/x?n=3");
        assert!(expand_template("{missing}", &v).is_err());
    }

    /// **A passing refusal does not end a stream, and a rewind is said** (the strict audit of
    /// 2026-11-04, S3/S4): over a stub that answers `busy`, an item, `expired`, a rewind and then an
    /// item that must not be handed, the stream hands the first item and ends with `Rewound` at the
    /// rewind's cursor.
    ///
    /// Mutations (2026-11-04), each red: the retry removed (the `busy` ended the
    /// stream); the rewind not said (the item after it was handed).
    #[test]
    fn a_stream_asks_again_when_busy_and_ends_on_a_rewind_with_its_cursor() {
        use std::net::TcpListener;
        let pages = vec![
            json!({"err": "busy"}),
            json!({"items": [1], "next": "c1"}),
            json!({"err": "expired"}),
            json!({"items": [], "next": "c0", "rewound": true}),
            json!({"items": [2], "next": "c2"}),
        ];
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (page, conn) in pages.into_iter().zip(listener.incoming()) {
                let mut c = conn.unwrap();
                let mut buf = [0u8; 65536];
                let mut got = Vec::new();
                // The head and its body, by Content-Length.
                let end = |got: &[u8]| got.windows(4).position(|w| w == b"\r\n\r\n");
                loop {
                    let n = c.read(&mut buf).unwrap();
                    got.extend_from_slice(&buf[..n]);
                    if let Some(i) = end(&got) {
                        let head = String::from_utf8_lossy(&got[..i]).to_lowercase();
                        let len: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                        if got.len() >= i + 4 + len || n == 0 {
                            break;
                        }
                    }
                }
                let id: Value = serde_json::from_slice(&got[end(&got).unwrap() + 4..]).unwrap();
                let result = match page["err"].as_str() {
                    Some(m) => json!({"isError": true, "content": [{"type": "text", "text": m}], "_meta": {"bentrix/machine": m}}),
                    None => json!({"structuredContent": page}),
                };
                let body = json!({"jsonrpc": "2.0", "id": id["id"], "result": result}).to_string();
                let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        let client = Client::new(format!("http://127.0.0.1:{port}/mcp"), "tok");
        let got: Vec<Result<Value, Error>> = client.watch("bentrix://x", json!({}), None, 10).take(3).collect();
        assert_eq!(got.first(), Some(&Ok(json!(1))), "the busy answer ended the stream: {got:?}");
        assert_eq!(got.get(1), Some(&Err(Error::Rewound { uri: "bentrix://x".into(), cursor: Some("c0".into()) })), "the rewind was not said: {got:?}");
        assert_eq!(got.len(), 2, "the stream went on after the rewind: {got:?}");
    }

    #[test]
    fn base64_and_a_chunked_body() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(header_value("é"), "=?base64?w6k=?=");
        assert_eq!(dechunk(b"3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n"), b"abcde");
    }
}
