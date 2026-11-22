//! **A model's forecast on the chart, built by hand**: the `forecast_spec` document step by step — a time
//! form (an interval) or an element form (a chart built by price: `renko`, `kagi`, `line_break`, `point_figure`,
//! `range_bars` — no interval, its rule in `form.elements`) — put through `forecast_put`; and an evaluation's sheet
//! waited on: `forecast_eval_make`'s answer is the run at once, or an ask the person answers in Bentrix's window,
//! watched on the `bentrix://forecasts/eval` event. A forecast never names its arrival: Bentrix stamps `put_ms`.

use serde_json::{json, Map, Value};

use crate::Error;

/// What a forecast is put through: [`crate::Client`], or a test's script.
pub trait Caller {
    fn call(&self, name: &str, args: Value) -> Result<Value, Error>;
}

impl Caller for crate::Client {
    fn call(&self, name: &str, args: Value) -> Result<Value, Error> {
        crate::Client::call(self, name, args)
    }
}

/// **One bar of a forecast**; everything but `close` left out when `None` or empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub close: f64,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub volume: Option<f64>,
    /// `(lo, hi, p)`.
    pub bands: Vec<(f64, f64, Option<f64>)>,
    /// By level, as the document writes it (`"0.05"`).
    pub quantiles: Vec<(String, f64)>,
    pub samples: Vec<f64>,
    /// When the step is expected — on every step or none; what places an element form on a chart by time.
    pub expected_ms: Option<i64>,
}

impl Step {
    pub fn close(close: f64) -> Step {
        Step { close, ..Step::default() }
    }

    fn value(&self) -> Value {
        let mut s = Map::new();
        s.insert("close".into(), json!(self.close));
        for (k, v) in [("open", self.open), ("high", self.high), ("low", self.low), ("volume", self.volume)] {
            if let Some(v) = v {
                s.insert(k.into(), json!(v));
            }
        }
        if let Some(t) = self.expected_ms {
            s.insert("expected_ms".into(), json!(t));
        }
        if !self.bands.is_empty() {
            s.insert("bands".into(), Value::Array(self.bands.iter().map(|(lo, hi, p)| p.map_or_else(|| json!({"lo": lo, "hi": hi}), |p| json!({"lo": lo, "hi": hi, "p": p}))).collect()));
        }
        if !self.quantiles.is_empty() {
            s.insert("quantiles".into(), Value::Object(self.quantiles.iter().map(|(k, v)| (k.clone(), json!(v))).collect()));
        }
        if !self.samples.is_empty() {
            s.insert("samples".into(), json!(self.samples));
        }
        Value::Object(s)
    }
}

/// **The `forecast_spec` document.** `anchor_ms` is the open of the last closed bar (or the moment of the last closed
/// element) it is made at; step 1 is the one after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Forecast {
    pub model: String,
    pub symbol: String,
    pub chart_type: String,
    /// A time form's interval; `None` for an element form.
    pub timeframe: Option<String>,
    /// An element form's rule, where it is not the chart's own.
    pub elements: Option<Value>,
    pub anchor_ms: i64,
    pub epoch: u32,
    pub note: Option<String>,
    pub look: Option<Value>,
    pub steps: Vec<Step>,
}

impl Forecast {
    /// A forecast of `timeframe`'s bars, drawn as candles.
    pub fn new(model: &str, symbol: &str, timeframe: &str, anchor_ms: i64) -> Forecast {
        Forecast { model: model.into(), symbol: symbol.into(), chart_type: "candlestick".into(), timeframe: Some(timeframe.into()), elements: None, anchor_ms, epoch: 0, note: None, look: None, steps: Vec::new() }
    }

    /// A forecast of a chart built by price: no interval, `elements` its rule (`None` for the chart's own).
    pub fn of_elements(model: &str, symbol: &str, chart_type: &str, anchor_ms: i64, elements: Option<Value>) -> Forecast {
        Forecast { chart_type: chart_type.into(), timeframe: None, elements, ..Forecast::new(model, symbol, "", anchor_ms) }
    }

    /// One more bar.
    pub fn step(mut self, s: Step) -> Forecast {
        self.steps.push(s);
        self
    }

    /// The document.
    pub fn spec(&self) -> Value {
        let mut form = Map::new();
        form.insert("chart_type".into(), json!(self.chart_type));
        if let Some(tf) = &self.timeframe {
            form.insert("timeframe".into(), json!(tf));
        }
        if let Some(e) = &self.elements {
            form.insert("elements".into(), e.clone());
        }
        let mut d = json!({"schema_version": 1, "model": self.model, "symbol": self.symbol, "form": form,
            "anchor": {"time_ms": self.anchor_ms}, "steps": self.steps.iter().map(Step::value).collect::<Vec<_>>(), "epoch": self.epoch});
        if let Some(n) = &self.note {
            d["note"] = json!(n);
        }
        if let Some(l) = &self.look {
            d["look"] = l.clone();
        }
        d
    }

    /// `forecast_put` on the workbench `bench`: its arrival, its state, where it stands on each chart.
    pub fn put(&self, c: &dyn Caller, bench: &str) -> Result<Value, Error> {
        c.call("forecast_put", json!({"bench": bench, "forecast": self.spec()}))
    }
}

/// `forecast_put_many`: several markets and models at one instant.
pub fn put_many(c: &dyn Caller, bench: &str, forecasts: &[Forecast], all_or_nothing: bool) -> Result<Value, Error> {
    c.call("forecast_put_many", json!({"bench": bench, "forecasts": forecasts.iter().map(Forecast::spec).collect::<Vec<_>>(), "all_or_nothing": all_or_nothing}))
}

/// The event an ask's moves are carried on.
pub const EVAL_EVENT: &str = "bentrix://forecasts/eval";

/// **How an evaluation's sheet was answered**, when it was not a run.
#[derive(Clone, Debug, PartialEq)]
pub enum EvalAnswer {
    /// The person's *No*. Not a failure: nothing was made or fetched.
    DeclinedByThePerson { ask: String },
    /// The laboratory over the span was not made, or the run did not start on it — with Bentrix's why.
    EvalFailed { ask: String, why: String },
    /// No move of the ask in `idle_limit` waits.
    NoAnswer { ask: String },
    /// The call itself failed.
    Call(Error),
}

/// **The run to drive**, from `forecast_eval_make`'s answer: its `eval` at once, or — over days the workbench's
/// laboratory does not hold — the person's answer on the sheet, watched on [`EVAL_EVENT`]. No tool answers the sheet.
/// `idle_limit` waits in a row with no move of this ask end the watch (never, when `None`).
pub fn eval_started(c: &dyn Caller, made: &Value, timeout_ms: u64, idle_limit: Option<u32>) -> Result<u64, EvalAnswer> {
    if let Some(e) = made.get("eval").and_then(Value::as_u64) {
        return Ok(e);
    }
    let ask = made["ask"].as_str().unwrap_or_default().to_owned();
    let watch = made["watch"].as_str().unwrap_or(EVAL_EVENT);
    let mut cursor = watch.split_once("after=").map_or("0", |(_, a)| a.split('&').next().unwrap_or("0")).to_owned();
    let uri = watch.split(['{', '?']).next().unwrap_or(EVAL_EVENT).to_owned();
    let mut idle = 0;
    while idle_limit.is_none_or(|n| idle < n) {
        let page = c.call("wait_for", json!({"uri": uri, "after": cursor, "timeout_ms": timeout_ms})).map_err(EvalAnswer::Call)?;
        if let Some(n) = page["next"].as_str() {
            cursor = n.to_owned();
        }
        let mut moved = false;
        for item in page["items"].as_array().into_iter().flatten().filter(|i| i["ask"] == ask.as_str()) {
            moved = true;
            match item["state"].as_str() {
                Some("started") => return item["eval"].as_u64().ok_or_else(|| EvalAnswer::EvalFailed { ask: ask.clone(), why: "the run answered no eval".into() }),
                Some("declined") => return Err(EvalAnswer::DeclinedByThePerson { ask }),
                Some("failed") => return Err(EvalAnswer::EvalFailed { why: item["why"].as_str().unwrap_or("the evaluation did not start").to_owned(), ask }),
                _ => {}
            }
        }
        idle = if moved { 0 } else { idle + 1 };
    }
    Err(EvalAnswer::NoAnswer { ask })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Sheet {
        pages: RefCell<Vec<Vec<Value>>>,
        asked: RefCell<Vec<String>>,
    }

    impl Caller for Sheet {
        fn call(&self, name: &str, args: Value) -> Result<Value, Error> {
            assert_eq!((name, args["uri"].as_str()), ("wait_for", Some(EVAL_EVENT)));
            let after = args["after"].as_str().unwrap().to_owned();
            self.asked.borrow_mut().push(after.clone());
            let mut pages = self.pages.borrow_mut();
            let items = if pages.is_empty() { Vec::new() } else { pages.remove(0) };
            Ok(json!({"items": items, "next": (after.parse::<usize>().unwrap() + items.len()).to_string()}))
        }
    }

    fn sheet(pages: Vec<Vec<Value>>) -> Sheet {
        Sheet { pages: RefCell::new(pages), asked: RefCell::new(Vec::new()) }
    }

    /// **An element forecast names no interval and carries its rule.** Mutation (2026-11-08): `spec` writing the
    /// timeframe as `""` when there is none — red, the element form named an interval.
    #[test]
    fn an_element_forecast_names_no_interval_and_carries_its_rule() {
        let d = Forecast::of_elements("bricks", "EURUSD", "renko", 1_700_000_000_000, Some(json!({"renko_box": 0.0005}))).step(Step { expected_ms: Some(1_700_000_600_000), ..Step::close(1.1005) }).spec();
        assert_eq!(d["form"], json!({"chart_type": "renko", "elements": {"renko_box": 0.0005}}));
        assert_eq!(d["steps"][0]["expected_ms"], 1_700_000_600_000_i64);
        assert_eq!(Forecast::new("m", "EURUSD", "H1", 0).step(Step::close(1.0)).spec()["form"], json!({"chart_type": "candlestick", "timeframe": "H1"}));
    }

    /// **The run, the decline and the failure are three answers.** Mutation (2026-11-08): `declined` read as a
    /// failure — red, a decline was EvalFailed.
    #[test]
    fn an_evals_sheet_is_waited_on_and_a_decline_is_not_a_failure() {
        assert_eq!(eval_started(&sheet(vec![]), &json!({"eval": 7}), 0, Some(1)), Ok(7));
        let ask = json!({"ask": "bentrix://ask/2", "watch": "bentrix://forecasts/eval?after=0"});
        let s = sheet(vec![
            vec![json!({"ask": "bentrix://ask/1", "state": "declined"}), json!({"ask": "bentrix://ask/2", "state": "asked"})],
            vec![json!({"ask": "bentrix://ask/2", "state": "made_laboratory"})],
            vec![json!({"ask": "bentrix://ask/2", "state": "started", "eval": 11})],
        ]);
        assert_eq!(eval_started(&s, &ask, 0, Some(3)), Ok(11));
        assert_eq!(*s.asked.borrow(), vec!["0", "2", "3"]);
        assert!(matches!(eval_started(&sheet(vec![vec![json!({"ask": "bentrix://ask/2", "state": "declined"})]]), &ask, 0, Some(3)), Err(EvalAnswer::DeclinedByThePerson { .. })));
        assert_eq!(
            eval_started(&sheet(vec![vec![json!({"ask": "bentrix://ask/2", "state": "failed", "why": "the laboratory was not made"})]]), &ask, 0, Some(3)),
            Err(EvalAnswer::EvalFailed { ask: "bentrix://ask/2".into(), why: "the laboratory was not made".into() })
        );
        assert!(matches!(eval_started(&sheet(vec![]), &ask, 0, Some(2)), Err(EvalAnswer::NoAnswer { .. })));
    }
}
