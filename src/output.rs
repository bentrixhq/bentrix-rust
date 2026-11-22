//! **A model's output of any kind on the chart**: the `output_spec` document built by hand for the eleven
//! kinds beside a path (a path is [`crate::forecast::Forecast`]'s) — scalar, classes, quantiles, rank, position,
//! weights, event, levels, action, text, series — checked as Bentrix checks it, a value outside its declared range
//! refused in words and never clamped, and put through `output_put`; and the person's meaning (`output_meaning`) set
//! through `output_meaning_set`, never in a model's file. An output never names its arrival: Bentrix stamps `put_ms`.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::forecast::Caller;
use crate::Error;

/// The twelve kinds, as the document writes them.
pub const KINDS: [&str; 12] = ["path", "scalar", "classes", "quantiles", "rank", "position", "weights", "event", "levels", "action", "text", "series"];
/// The meaning's closed words.
pub const MEANING_WORDS: [&str; 9] = ["buy", "sell", "nothing", "up", "down", "flat", "in", "out", "close"];
/// An action's intents.
pub const INTENTS: [&str; 5] = ["buy", "sell", "close_buys", "close_sells", "nothing"];

fn refused(m: String) -> Error {
    Error::Other(m)
}

fn label_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn in_range(field: &str, v: f64, range: Option<(f64, f64)>) -> Result<(), Error> {
    if !v.is_finite() {
        return Err(refused(format!("{field} {v} is not a number")));
    }
    if let Some((lo, hi)) = range {
        if v < lo || v > hi {
            return Err(refused(format!("{field} the output answers {lo} to {hi}; {v} is outside it")));
        }
    }
    Ok(())
}

/// **One output's document**: its kind, its values as the document writes them, and the kind's own fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub kind: &'static str,
    pub model: String,
    pub name: String,
    pub symbol: String,
    pub anchor_ms: i64,
    pub values: Vec<Value>,
    /// `range`, `units`, `horizon`, `names`, `levels`, `universe`, `markets`, `long_only`, `exclusive`, `softmax`,
    /// `persistent`, `gross`, `note` — whichever the kind takes.
    pub fields: Map<String, Value>,
    pub epoch: u32,
    pub suggested_meaning: Option<Meaning>,
}

impl Output {
    fn new(kind: &'static str, model: &str, name: &str, symbol: &str, anchor_ms: i64, values: Vec<Value>) -> Output {
        Output { kind, model: model.into(), name: name.into(), symbol: symbol.into(), anchor_ms, values, fields: Map::new(), epoch: 0, suggested_meaning: None }
    }

    fn with(mut self, k: &str, v: Value) -> Output {
        self.fields.insert(k.into(), v);
        self
    }

    fn range(&self) -> Option<(f64, f64)> {
        let r = self.fields.get("range")?.as_array()?;
        Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?))
    }

    fn names(&self) -> Vec<String> {
        self.fields.get("names").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
    }

    /// A number per step; `range` its declared bounds, if any.
    pub fn scalar(model: &str, name: &str, symbol: &str, anchor_ms: i64, range: Option<(f64, f64)>, values: &[f64]) -> Output {
        let o = Output::new("scalar", model, name, symbol, anchor_ms, values.iter().map(|v| json!({"value": v})).collect());
        match range {
            Some((lo, hi)) => o.with("range", json!([lo, hi])),
            None => o,
        }
    }

    /// Probabilities over named classes, one map per step.
    pub fn classes(model: &str, name: &str, symbol: &str, anchor_ms: i64, names: &[&str], exclusive: bool, steps: &[BTreeMap<String, f64>]) -> Output {
        Output::new("classes", model, name, symbol, anchor_ms, steps.iter().map(|p| json!({"p": p})).collect()).with("names", json!(names)).with("exclusive", json!(exclusive))
    }

    /// Quantiles at `levels`, one map per step keyed as the document writes a level (`"0.1"`).
    pub fn quantiles(model: &str, name: &str, symbol: &str, anchor_ms: i64, levels: &[f64], steps: &[BTreeMap<String, f64>]) -> Output {
        Output::new("quantiles", model, name, symbol, anchor_ms, steps.iter().map(|q| json!({"q": q})).collect()).with("levels", json!(levels))
    }

    /// A rank in 0..1 among `universe` markets.
    pub fn rank(model: &str, name: &str, symbol: &str, anchor_ms: i64, universe: u32, value: f64) -> Output {
        Output::new("rank", model, name, symbol, anchor_ms, vec![json!({"value": value})]).with("universe", json!(universe))
    }

    /// A target position in `units` (`fraction`, `lots`, …).
    pub fn position(model: &str, name: &str, symbol: &str, anchor_ms: i64, units: &str, target: f64) -> Output {
        Output::new("position", model, name, symbol, anchor_ms, vec![json!({"target": target})]).with("units", json!(units))
    }

    /// Weights across markets.
    pub fn weights(model: &str, name: &str, symbol: &str, anchor_ms: i64, long_only: bool, w: &BTreeMap<String, f64>) -> Output {
        Output::new("weights", model, name, symbol, anchor_ms, vec![json!({"w": w})]).with("long_only", json!(long_only)).with("markets", json!(w.keys().collect::<Vec<_>>()))
    }

    /// An event's score in 0..1, expected within `within_steps` (0 for none).
    pub fn event(model: &str, name: &str, symbol: &str, anchor_ms: i64, score: f64, within_steps: u32) -> Output {
        let mut v = json!({"score": score});
        if within_steps > 0 {
            v["within_steps"] = json!(within_steps);
        }
        Output::new("event", model, name, symbol, anchor_ms, vec![v])
    }

    /// Named price levels (`stop`, `target`, …).
    pub fn levels(model: &str, name: &str, symbol: &str, anchor_ms: i64, levels: &BTreeMap<String, f64>) -> Output {
        Output::new("levels", model, name, symbol, anchor_ms, vec![json!({"levels": levels})])
    }

    /// An intent, with a size where it has one.
    pub fn action(model: &str, name: &str, symbol: &str, anchor_ms: i64, intent: &str, size: Option<f64>) -> Output {
        let mut v = json!({"intent": intent});
        if let Some(s) = size {
            v["size"] = json!(s);
        }
        Output::new("action", model, name, symbol, anchor_ms, vec![v])
    }

    /// A sentence.
    pub fn text(model: &str, name: &str, symbol: &str, anchor_ms: i64, text: &str) -> Output {
        Output::new("text", model, name, symbol, anchor_ms, vec![json!({"text": text})])
    }

    /// Named lines, one map per step.
    pub fn series(model: &str, name: &str, symbol: &str, anchor_ms: i64, names: &[&str], steps: &[BTreeMap<String, f64>]) -> Output {
        Output::new("series", model, name, symbol, anchor_ms, steps.iter().map(|l| json!({"lines": l})).collect()).with("names", json!(names))
    }

    /// The horizon: bars of the anchor's interval.
    pub fn steps(self, n: u32) -> Output {
        self.with("horizon", json!({"steps": n}))
    }
    /// The horizon in business days.
    pub fn business_days(self, n: u32) -> Output {
        self.with("horizon", json!({"business_days": n}))
    }
    /// Held until `levels` or `next_output`.
    pub fn until(self, what: &str) -> Output {
        self.with("horizon", json!({"until": what}))
    }

    /// Refuses what Bentrix would refuse, in its words.
    pub fn check(&self) -> Result<(), Error> {
        if !label_ok(&self.model) || !label_ok(&self.name) {
            return Err(refused("the model and the output are [a-z0-9_]{1,32}".into()));
        }
        if self.values.is_empty() {
            return Err(refused("values holds no value".into()));
        }
        let range = self.range();
        let names = self.names();
        let softmax = self.fields.get("softmax").and_then(Value::as_bool).unwrap_or(false);
        for (i, v) in self.values.iter().enumerate() {
            let f = format!("values[{i}]");
            let num = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
            match self.kind {
                "scalar" => in_range(&format!("{f}.value"), num("value"), range)?,
                "rank" => in_range(&format!("{f}.value"), num("value"), range.or(Some((0.0, 1.0))))?,
                "event" => in_range(&format!("{f}.score"), num("score"), Some((0.0, 1.0)))?,
                "position" => in_range(&format!("{f}.target"), num("target"), range)?,
                "classes" => {
                    if names.is_empty() {
                        return Err(refused("names a classes output names its classes".into()));
                    }
                    for (k, p) in v.get("p").and_then(Value::as_object).into_iter().flatten() {
                        if !names.contains(k) {
                            return Err(refused(format!("{f}.p {k} is not one of the classes")));
                        }
                        if !softmax {
                            in_range(&format!("{f}.p.{k}"), p.as_f64().unwrap_or(f64::NAN), Some((0.0, 1.0)))?;
                        }
                    }
                }
                "series" if names.is_empty() => return Err(refused("names a series output names its lines".into())),
                "action" => {
                    let intent = v.get("intent").and_then(Value::as_str).unwrap_or("");
                    if !INTENTS.contains(&intent) {
                        return Err(refused(format!("{f}.intent {intent} is not one of {}", INTENTS.join(", "))));
                    }
                }
                "text" if v.get("text").and_then(Value::as_str).map_or(0, |t| t.chars().count()) > 2000 => {
                    return Err(refused(format!("{f}.text is longer than 2000 characters")));
                }
                _ => {}
            }
        }
        if let Some(m) = &self.suggested_meaning {
            m.check(range)?;
        }
        Ok(())
    }

    /// The `output_spec` document; never an arrival.
    pub fn spec(&self) -> Value {
        let mut d = json!({"schema_version": 1, "model": self.model, "output": self.name, "symbol": self.symbol, "kind": self.kind,
            "anchor": {"time_ms": self.anchor_ms}, "values": self.values});
        for (k, v) in &self.fields {
            d[k.as_str()] = v.clone();
        }
        if self.epoch > 0 {
            d["epoch"] = json!(self.epoch);
        }
        if let Some(m) = &self.suggested_meaning {
            d["suggested_meaning"] = m.doc();
        }
        d
    }

    /// `output_put` on the workbench `bench`, after [`Output::check`].
    pub fn put(&self, c: &dyn Caller, bench: &str) -> Result<Value, Error> {
        self.check()?;
        c.call("output_put", json!({"bench": bench, "output": self.spec()}))
    }
}

/// `output_put_many`: several outputs at one instant.
pub fn put_outputs(c: &dyn Caller, bench: &str, outputs: &[Output], all_or_nothing: bool) -> Result<Value, Error> {
    for o in outputs {
        o.check()?;
    }
    c.call("output_put_many", json!({"bench": bench, "outputs": outputs.iter().map(Output::spec).collect::<Vec<_>>(), "all_or_nothing": all_or_nothing}))
}

/// **The `output_meaning` document**: what a value means to the person.
#[derive(Clone, Debug, PartialEq)]
pub struct Meaning {
    pub reads: String,
    pub high_means: Option<String>,
    pub low_means: Option<String>,
    pub act_above: Option<f64>,
    pub act_below: Option<f64>,
    pub hysteresis: f64,
    pub labels: BTreeMap<String, String>,
}

impl Default for Meaning {
    fn default() -> Meaning {
        Meaning { reads: "direction".into(), high_means: None, low_means: None, act_above: None, act_below: None, hysteresis: 0.0, labels: BTreeMap::new() }
    }
}

impl Meaning {
    /// `high` above `act_above`, `low` below `act_below`.
    pub fn of(high: &str, low: Option<&str>, act_above: Option<f64>, act_below: Option<f64>) -> Meaning {
        Meaning { high_means: Some(high.into()), low_means: low.map(String::from), act_above, act_below, ..Meaning::default() }
    }

    /// Refuses a word that is not the product's, a threshold outside `range`, a label empty or over 24.
    pub fn check(&self, range: Option<(f64, f64)>) -> Result<(), Error> {
        for w in [&self.high_means, &self.low_means].into_iter().flatten() {
            if !MEANING_WORDS.contains(&w.as_str()) {
                return Err(refused(format!("{w:?} is not one of the meaning's words")));
            }
        }
        if let Some((lo, hi)) = range {
            for t in [self.act_above, self.act_below].into_iter().flatten() {
                if t < lo || t > hi {
                    return Err(refused(format!("the output answers {lo} to {hi}; {t} is outside it")));
                }
            }
        }
        for (k, l) in &self.labels {
            if !MEANING_WORDS.contains(&k.as_str()) || l.is_empty() || l.chars().count() > 24 {
                return Err(refused(format!("the label {l:?} for {k} is not a meaning word's, or is empty or longer than 24")));
            }
        }
        Ok(())
    }

    /// The document.
    pub fn doc(&self) -> Value {
        let mut d = json!({"reads": self.reads, "hysteresis": self.hysteresis});
        if let Some(w) = &self.high_means {
            d["high_means"] = json!(w);
        }
        if let Some(w) = &self.low_means {
            d["low_means"] = json!(w);
        }
        if let Some(t) = self.act_above {
            d["act_above"] = json!(t);
        }
        if let Some(t) = self.act_below {
            d["act_below"] = json!(t);
        }
        if !self.labels.is_empty() {
            d["labels"] = json!(self.labels);
        }
        d
    }
}

/// `output_meaning_set`: the person's meaning of `model`'s `output` on the workbench `bench`.
pub fn set_meaning(c: &dyn Caller, bench: &str, model: &str, output: &str, meaning: &Meaning, range: Option<(f64, f64)>) -> Result<Value, Error> {
    meaning.check(range)?;
    c.call("output_meaning_set", json!({"bench": bench, "model": model, "output": output, "meaning": meaning.doc()}))
}

#[cfg(test)]
mod tests {
    //! Mutation (2026-11-11): `in_range` not comparing — red, a 1.3 for a 0..1 output was sent.
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Rec(RefCell<Vec<(String, Value)>>);
    impl Caller for Rec {
        fn call(&self, name: &str, args: Value) -> Result<Value, Error> {
            self.0.borrow_mut().push((name.into(), args));
            Ok(json!({"ok": true}))
        }
    }

    fn m(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    #[test]
    fn every_kind_builds_a_document_the_twelve_pinned() {
        let built = [
            Output::scalar("m1", "o1", "EURUSD", 1, Some((0.0, 1.0)), &[0.7]),
            Output::classes("m1", "o1", "EURUSD", 1, &["up", "down"], true, &[m(&[("up", 0.6), ("down", 0.4)])]),
            Output::quantiles("m1", "o1", "EURUSD", 1, &[0.1, 0.9], &[m(&[("0.1", 1.0), ("0.9", 1.2)])]),
            Output::rank("m1", "o1", "AAPL", 1, 4871, 0.62),
            Output::position("m1", "o1", "EURUSD", 1, "fraction", -0.5),
            Output::weights("m1", "o1", "EURUSD", 1, true, &m(&[("EURUSD", 0.6), ("GBPUSD", 0.4)])),
            Output::event("m1", "o1", "EURUSD", 1, 0.8, 3),
            Output::levels("m1", "o1", "EURUSD", 1, &m(&[("stop", 1.09), ("target", 1.12)])),
            Output::action("m1", "o1", "EURUSD", 1, "buy", Some(0.5)),
            Output::text("m1", "o1", "EURUSD", 1, "the trend held"),
            Output::series("m1", "o1", "EURUSD", 1, &["a"], &[m(&[("a", 1.0)])]),
        ];
        let mut kinds: std::collections::BTreeSet<&str> = ["path"].into();
        for o in &built {
            o.check().unwrap_or_else(|e| panic!("{}: {e}", o.kind));
            assert_eq!(o.spec()["kind"], o.kind);
            kinds.insert(o.kind);
        }
        assert_eq!(kinds.len(), KINDS.len());
        assert_eq!(KINDS.len(), 12);
    }

    #[test]
    fn a_value_outside_its_range_is_refused_in_words_before_anything_is_sent() {
        let r = Rec::default();
        let e = Output::scalar("m1", "o1", "EURUSD", 1, Some((0.0, 1.0)), &[1.3]).put(&r, "bench").unwrap_err();
        assert!(e.to_string().contains("0 to 1; 1.3 is outside it"), "{e}");
        assert!(r.0.borrow().is_empty(), "something was sent");
        assert!(Output::rank("m1", "o1", "AAPL", 1, 10, 1.5).check().is_err());
    }

    #[test]
    fn the_owners_three_examples_are_meanings_and_profit_is_not_a_word() {
        let r = Rec::default();
        for mm in [Meaning::of("buy", Some("sell"), Some(0.6), Some(0.4)), Meaning::of("buy", Some("nothing"), Some(0.6), None), Meaning::of("sell", Some("nothing"), Some(0.6), None)] {
            set_meaning(&r, "bench", "m1", "o1", &mm, Some((0.0, 1.0))).unwrap();
        }
        let calls = r.0.borrow();
        assert_eq!(calls[2].0, "output_meaning_set");
        assert_eq!(calls[2].1["meaning"]["high_means"], "sell");
        drop(calls);
        assert!(set_meaning(&r, "bench", "m1", "o1", &Meaning::of("profit", None, None, None), Some((0.0, 1.0))).is_err());
        assert!(set_meaning(&r, "bench", "m1", "o1", &Meaning::of("buy", None, Some(1.5), None), Some((0.0, 1.0))).is_err());
    }

    #[test]
    fn put_sends_the_document_and_never_an_arrival() {
        let r = Rec::default();
        Output::scalar("m1", "dir", "EURUSD", 1, Some((0.0, 1.0)), &[0.7]).steps(4).put(&r, "bench").unwrap();
        put_outputs(&r, "bench", &[Output::event("m1", "e", "EURUSD", 1, 0.9, 0)], true).unwrap();
        let calls = r.0.borrow();
        assert_eq!(calls[0].0, "output_put");
        assert_eq!(calls[0].1["output"]["horizon"], json!({"steps": 4}));
        assert!(calls[0].1["output"].get("put_ms").is_none(), "an arrival was sent");
        assert_eq!(calls[1].0, "output_put_many");
        assert_eq!(calls[1].1["all_or_nothing"], true);
    }
}
