//! Demands to requests: a request carries one value, as its state, and
//! every question asked of it, as written: the API's documented pattern, so
//! a question's `` `field` `` names a field of the value. The API evaluates
//! questions independently, so a value's answers don't depend on what else
//! is asked of it; values sharing one state did move each other's answers
//! (tests/live, 2026-09-29; DESIGN.md, "Why one value per request").
//!
//! A value is raw JSON, as jaq wrote it; it is grouped, hashed and sent as
//! those bytes.

use crate::transport::api::{Request, Spec};
use serde::Serialize;
use serde_json::value::{RawValue, to_raw_value};
use serde_json::{Map, Value as Json, json};
use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};

/// Bytes per token until a run has measured its own: about what log lines
/// are, and less than prose, so the first estimates err high.
pub const BYTES_PER_TOKEN: f64 = 2.0;

pub fn qname(i: usize) -> String {
    format!("q{i}")
}

/// What identifies one question about one value: a 128-bit hash of the
/// request that would ask it alone, so it doesn't depend on the other
/// questions asked of its value.
pub fn key(model: &str, state: &RawValue, spec: &Spec) -> u128 {
    let text = build(model, state, &[spec]).body();
    let h = |salt: u8| {
        let mut s = DefaultHasher::new();
        (salt, &text).hash(&mut s);
        s.finish() as u128
    };
    h(0) << 64 | h(1)
}

/// A request, and the indices of the demands its questions `q0, q1, …`
/// answer.
pub struct Built {
    pub req: Request,
    pub order: Vec<usize>,
}

/// The requests for a wave of `(value, spec)` demands: one for each
/// distinct value, in order of its first demand.
pub fn requests(model: &str, demands: &[(&RawValue, &Spec)]) -> Vec<Built> {
    let mut slots: Vec<(&RawValue, Vec<usize>)> = Vec::new();
    let mut by_state: HashMap<&str, usize> = HashMap::new();
    for (i, (state, _)) in demands.iter().enumerate() {
        let s = *by_state.entry(state.get()).or_insert_with(|| {
            slots.push((state, Vec::new()));
            slots.len() - 1
        });
        slots[s].1.push(i);
    }
    let reqs = slots.into_iter().map(|(state, order)| {
        let specs: Vec<&Spec> = order.iter().map(|&i| demands[i].1).collect();
        Built { req: build(model, state, &specs), order }
    });
    reqs.collect()
}

/// A question, its fields in sorted order.
#[derive(Serialize)]
struct Question {
    #[serde(skip_serializing_if = "Json::is_null")]
    criteria: Json,
    instructions: String,
    #[serde(rename = "type")]
    kind: &'static str,
}

/// A value as the state: as it is when the API takes it as one (a string,
/// an object or an array), any other value as its text: a state of `42`,
/// `true` or `null` is refused.
fn as_state(value: &RawValue) -> Box<RawValue> {
    match value.get().trim_start().as_bytes().first() {
        Some(b'"' | b'{' | b'[') => value.to_owned(),
        _ => to_raw_value(value.get()).expect("a string always serialises"),
    }
}

/// One request from a value and the questions asked of it.
fn build(model: &str, state: &RawValue, specs: &[&Spec]) -> Request {
    let mut questions = BTreeMap::new();
    for spec in specs {
        let qn = qname(questions.len());
        let criteria = match spec.kind() {
            "choice" => {
                // an option with no description is read by its name alone
                let about = spec.about.iter().chain(std::iter::repeat(&Json::Null));
                Json::Object(spec.among.iter().cloned().zip(about.cloned()).collect())
            }
            // a scale's levels, lowest first
            "score" => Json::Array(spec.levels.clone()),
            // what yes and what no mean, and by default their examples,
            // as the API's docs write them: `{"true": {"what": …,
            // "examples": […]}}`, or `{"true": "…"}` for a rubric alone
            // (measured: level with examples after the question, which
            // beat examples in the state)
            _ => {
                let sides = [("true", &spec.yes, &spec.like), ("false", &spec.no, &spec.unlike)];
                let side = |what: &String, ex: &Vec<String>| match (what.is_empty(), ex.is_empty()) {
                    (true, true) => None,
                    (false, true) => Some(json!(what)),
                    (true, false) => Some(json!({"examples": ex})),
                    (false, false) => Some(json!({"what": what, "examples": ex})),
                };
                let m: Map<String, Json> =
                    sides.into_iter().filter_map(|(k, what, ex)| Some((k.into(), side(what, ex)?))).collect();
                if m.is_empty() { Json::Null } else { Json::Object(m) }
            }
        };
        let q = Question { criteria, instructions: spec.q.clone(), kind: spec.kind() };
        questions.insert(qn, to_raw_value(&q).expect("a question always serialises"));
    }
    let state = as_state(state);
    Request { model: model.to_owned(), state, questions, assets: BTreeMap::new(), pictures: Vec::new() }
}

/// A request's tokens, estimated at `bytes_per_token`.
pub fn tokens(r: &Request, bytes_per_token: f64) -> u64 {
    (r.body().len() as f64 / bytes_per_token) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(q: &str) -> Spec {
        Spec { q: q.into(), ..Spec::default() }
    }

    fn raw(j: &str) -> Box<RawValue> {
        RawValue::from_string(j.to_owned()).unwrap()
    }

    fn body(r: &Request) -> Json {
        serde_json::from_str(&r.body()).unwrap()
    }

    #[test]
    fn shape() {
        let (a, b) = (raw(r#""line a""#), raw(r#"{"id":1}"#));
        let mut ex = spec("like?");
        ex.like = vec!["l".into()];
        let (q1, q2) = (spec("q1"), spec("q2"));
        let demands = [(&*a, &q1), (&*b, &ex), (&*a, &q2)];
        let reqs = requests("jev-latest", &demands);
        let got: Vec<_> = reqs.iter().map(|p| (body(&p.req), p.order.clone())).collect();
        let want = [
            (
                json!({
                    "model": "jev-latest",
                    "state": "line a",
                    "questions": {
                        "q0": {"type": "noul", "instructions": "q1"},
                        "q1": {"type": "noul", "instructions": "q2"}
                    }
                }),
                vec![0, 2],
            ),
            (
                json!({
                    "model": "jev-latest",
                    "state": {"id": 1},
                    "questions": {
                        "q0": {"type": "noul", "instructions": "like?",
                               "criteria": {"true": {"examples": ["l"]}}}
                    }
                }),
                vec![1],
            ),
        ];
        assert_eq!(got, want);
        // a choice's options, described or not
        let mut choice = spec("which?");
        choice.among = vec!["x".into(), "y".into()];
        let req = &requests("m", &[(&*a, &choice)])[0].req;
        assert_eq!(body(req)["questions"]["q0"]["criteria"], json!({"x": null, "y": null}));
        // a rubric and examples on each side, as the API's docs write them
        let mut both = spec("both?");
        (both.yes, both.no, both.like, both.unlike) = ("y".into(), "n".into(), vec!["l".into()], vec!["u".into()]);
        let req = &requests("m", &[(&*a, &both)])[0].req;
        assert_eq!(
            body(req)["questions"]["q0"]["criteria"],
            json!({"true": {"what": "y", "examples": ["l"]}, "false": {"what": "n", "examples": ["u"]}})
        );
        // the API takes a string, an object or an array as a state: any other
        // value goes as its text
        for (value, state) in [("42", r#""42""#), ("true", r#""true""#), ("null", r#""null""#), ("[1]", "[1]")] {
            let v = raw(value);
            assert_eq!(requests("m", &[(&*v, &q1)])[0].req.state.get(), state);
        }
    }

    // A value is sent as jaq wrote it: numbers as written, keys in order.
    #[test]
    fn sends_the_value_as_written() {
        let rec = r#"{"z":1,"id":12345678901234567890,"price":1.10,"e":1e1000}"#;
        let (a, q) = (raw(rec), spec("q"));
        let reqs = requests("m", &[(&*a, &q)]);
        let want =
            format!(r#"{{"model":"m","state":{rec},"questions":{{"q0":{{"instructions":"q","type":"noul"}}}}}}"#);
        assert_eq!(reqs[0].req.body(), want);
    }

    #[test]
    fn key_ignores_the_other_questions() {
        let (a, b, q) = (raw(r#""a""#), raw(r#""b""#), spec("q"));
        assert_eq!(key("m", &a, &q), key("m", &a, &q));
        assert_ne!(key("m", &a, &q), key("m", &b, &q));
        assert_ne!(key("m", &a, &q), key("m2", &a, &q));
        assert_ne!(key("m", &a, &q), key("m", &a, &spec("q2")));
        let (ab, ba) = (raw(r#"{"a":1,"b":2}"#), raw(r#"{"b":2,"a":1}"#));
        assert_ne!(key("m", &ab, &q), key("m", &ba, &q), "the model sees the keys in order");
    }
}
