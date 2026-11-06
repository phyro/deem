//! Native filters: `judge`, an empty `env`, `debug`/`stderr` that
//! write to stderr and `now` that reads the clock, each once per record
//! however many times it runs, a `fromjson` that can't loop, and a bridge
//! to jaq-json's natives.

use super::val::{Interrupt, J, Val, interrupt, liftr};
use super::{D, Run};
use crate::transport::api::{Answer, Asked, Failure, Spec};
use jaq_core::box_iter::box_once;
use jaq_core::data::JustLut;
use jaq_core::load::{Arena, File, Loader};
use jaq_core::native::{Fun, bome, v};
use jaq_core::{Ctx, Cv, Exn, Native, RunPtr, ValR, ValXs, Vars};
use serde_json::{Value as Json, json};
use std::io::Write;

type Error = jaq_core::Error<Val>;

/// `judge(spec; ctx)`: `ctx` is what the model sees, and nothing else is
/// sent; both are computed from the input, so the question can read the
/// record while `ctx` picks what goes out. Both are looked at, since the
/// question can't be asked without them. An answer the record doesn't have
/// yet is unknown; one that failed is a jq error here, as a constant that
/// couldn't be had would be.
fn judge<'a>(mut cv: Cv<'a, D>) -> ValXs<'a, Val> {
    let ctx = cv.0.pop_var();
    let spec = cv.0.pop_var();
    let run = cv.0.data().run;
    bome(judge_val(run, &spec, &ctx))
}

fn judge_val(run: &Run, spec: &Val, input: &Val) -> ValR<Val> {
    // the record's answer, if it has one: a repeat costs a lookup
    let args = (spec.force().clone(), input.force().clone());
    if let Some(a) = run.answer(&args) {
        return a.map(Val::Ready).map_err(Error::str);
    }
    let spec = parse_spec(&args.0)?;
    let q = Json::String(spec.q.clone());
    match run.ask.ask(&args.1, spec) {
        Ok(Asked::Answer(a)) => {
            let a = a.map(from_answer).map_err(|why| format!("the answer to {q} failed: {why}"));
            run.keep(args, a.clone());
            a.map(Val::Ready).map_err(Error::str)
        }
        Ok(Asked::Later(id)) => {
            run.unclean();
            Ok(Val::Unknown(id))
        }
        Ok(Asked::Full) => {
            run.unclean();
            interrupt(Interrupt::Rerun(None))
        }
        Err(Failure::Failed(e)) => Err(Error::str(e)),
        Err(Failure::Stopped) => interrupt(Interrupt::Stopped),
    }
}

/// The fields a spec may have. Any other is an error, so a typo
/// (`amoung`) can't quietly change the question.
const FIELDS: [&str; 8] = ["q", "among", "levels", "box", "yes", "no", "like", "unlike"];

fn parse_spec(j: &J) -> Result<Spec, Error> {
    let J::Obj(m) = j else {
        return Err(Error::str(format!("judge: expected an object like {{q: \"…\"}}, got {j}")));
    };
    if let Some(k) = m.keys().find(|k| !FIELDS.iter().any(|f| **k == J::from((*f).to_owned()))) {
        return Err(Error::str(format!(
            "judge: unknown field {k}; a spec has q, among, levels, box, yes, no, like, unlike"
        )));
    }
    let get = |k: &str| m.get(&J::from(k.to_owned()));
    let text = |v: &J| match v {
        J::TStr(s) => Some(String::from_utf8_lossy(s).into_owned()),
        _ => None,
    };
    let list = |k: &str| -> Result<Vec<String>, Error> {
        match get(k) {
            None | Some(J::Null) => Ok(Vec::new()),
            Some(J::Arr(xs)) => xs.iter().map(|x| text(x).ok_or(())).collect(),
            Some(_) => Err(()),
        }
        .map_err(|_| Error::str(format!("judge: `{k}` must be an array of strings")))
    };
    // absent or null is none; anything but a string is an error
    let rubric = |k: &str| -> Result<String, Error> {
        match get(k) {
            None | Some(J::Null) => Ok(String::new()),
            Some(v) => text(v).ok_or_else(|| Error::str(format!("judge: `{k}` must be a string"))),
        }
    };
    let q = get("q").and_then(text).filter(|q| !q.trim().is_empty());
    let q = q.ok_or_else(|| Error::str("judge: the spec needs a question, as {q: \"…\"}"))?;
    let (among, about) = options(get("among"))?;
    let levels = levels(get("levels"))?;
    let (yes, no, like, unlike) = (rubric("yes")?, rubric("no")?, list("like")?, list("unlike")?);
    let boxed = match get("box") {
        None | Some(J::Null | J::Bool(false)) => false,
        Some(J::Bool(true)) => true,
        Some(_) => return Err(Error::str("judge: `box` is true or false")),
    };
    let s = Spec { q, among, about, levels, boxed, yes, no, like, unlike };
    let others = [("among", !s.among.is_empty()), ("levels", !s.levels.is_empty()), ("yes", !s.yes.is_empty())];
    let others = others.into_iter().chain([("no", !s.no.is_empty()), ("like", !s.like.is_empty())]);
    let others = others.chain([("unlike", !s.unlike.is_empty())]);
    if let Some((k, _)) = others.filter(|_| s.boxed).find(|(_, set)| *set) {
        return Err(Error::str(format!("judge: a box question takes no `{k}`")));
    }
    if !s.levels.is_empty() && (get("among").is_some_and(|a| *a != J::Null) || !s.yes.is_empty() || !s.no.is_empty()) {
        return Err(Error::str("judge: `levels` is a scale; it doesn't go with `among`, `yes` or `no`"));
    }
    // the API's yes/no criteria hold these; a choice's criteria are its
    // options, and a scale's its levels, each with its own description
    let set = [("yes", !s.yes.is_empty()), ("no", !s.no.is_empty())];
    let mut set = set.into_iter().chain([("like", !s.like.is_empty()), ("unlike", !s.unlike.is_empty())]);
    if let Some((k, _)) = set.find(|(_, set)| *set).filter(|_| !s.among.is_empty() || !s.levels.is_empty()) {
        return Err(Error::str(format!(
            "judge: `{k}` is for a yes/no question; a choice or a scale describes each option or level instead"
        )));
    }
    // present but empty is a choice without options, not a score
    if s.among.is_empty() && matches!(get("among"), Some(J::Arr(_) | J::Obj(_))) {
        return Err(Error::str("judge: `among` is empty; give at least two options"));
    }
    if s.among.len() == 1 {
        return Err(Error::str("judge: a question with one option has one answer; give at least two"));
    }
    if s.among.len() > crate::transport::api::MAX_OPTIONS {
        let (n, max) = (s.among.len(), crate::transport::api::MAX_OPTIONS);
        return Err(Error::str(format!("judge: {n} options; the API takes at most {max}")));
    }
    // the options are sent as an object's keys, where a repeat would vanish
    if let Some(o) = s.among.iter().enumerate().find_map(|(i, o)| s.among[..i].contains(o).then_some(o)) {
        return Err(Error::str(format!("judge: `among` lists {} twice", json!(o))));
    }
    Ok(s)
}

/// `among`: an array of option names, or an object of names and what each
/// means (any JSON); absent or null is none.
fn options(among: Option<&J>) -> Result<(Vec<String>, Vec<Json>), Error> {
    let bad = || Error::str("judge: `among` must be an array of strings, or an object of descriptions");
    let name = |k: &J| match k {
        J::TStr(s) => Ok(String::from_utf8_lossy(s).into_owned()),
        _ => Err(bad()),
    };
    match among {
        None | Some(J::Null) => Ok((Vec::new(), Vec::new())),
        Some(J::Arr(xs)) => Ok((xs.iter().map(name).collect::<Result<_, _>>()?, Vec::new())),
        Some(J::Obj(m)) => {
            let about = |(k, v): (&J, &J)| {
                let v = serde_json::from_str(&v.to_string())
                    .map_err(|_| Error::str(format!("judge: the description of {k} can't be sent as JSON: {v}")))?;
                Ok((name(k)?, v))
            };
            Ok(m.iter().map(about).collect::<Result<Vec<_>, Error>>()?.into_iter().unzip())
        }
        Some(_) => Err(bad()),
    }
}

/// `levels`: 2 to 10 descriptions of a scale's levels, lowest first (any
/// JSON); absent or null is none.
fn levels(levels: Option<&J>) -> Result<Vec<Json>, Error> {
    let (min, max) = crate::transport::api::LEVELS;
    let xs = match levels {
        None | Some(J::Null) => return Ok(Vec::new()),
        Some(J::Arr(xs)) if (min..=max).contains(&xs.len()) => xs,
        Some(J::Arr(xs)) => {
            return Err(Error::str(format!("judge: {} levels; a scale has {min} to {max}", xs.len())));
        }
        Some(_) => return Err(Error::str("judge: `levels` must be an array, lowest level first")),
    };
    let level = |(i, v): (usize, &J)| {
        serde_json::from_str(&v.to_string())
            .map_err(|_| Error::str(format!("judge: level {i} can't be sent as JSON: {v}")))
    };
    xs.iter().enumerate().map(level).collect()
}

/// An answer as the object `judge` outputs.
fn from_answer(a: Answer) -> J {
    let j = |v: Json| {
        // a box's edges in the API's order, not sorted
        let edges = ["top", "bottom", "left", "right"];
        let text = match v.as_object().filter(|m| m.len() == 4 && edges.iter().all(|k| m.contains_key(*k))) {
            Some(m) => format!("{{{}}}", edges.map(|k| format!("{}:{}", json!(k), m[k])).join(",")),
            None => v.to_string(),
        };
        jaq_json::read::parse_single(text.as_bytes()).unwrap_or_default()
    };
    let kvs = [("value", a.value), ("confidence", json!(a.confidence)), ("probabilities", json!(a.probabilities))];
    J::obj(kvs.into_iter().map(|(k, v)| (J::from(k.to_owned()), j(v))).collect())
}

/// A program can't read the environment.
fn env<'a>(_: Cv<'a, D>) -> ValXs<'a, Val> {
    box_once(Ok(Val::Ready(J::obj(Default::default()))))
}

/// One JSON value from a string. (jaq's reads several, and loops on some
/// invalid text.)
fn fromjson<'a>(cv: Cv<'a, D>) -> ValXs<'a, Val> {
    let parse = |j: &J| match j {
        J::TStr(s) => jaq_json::read::parse_single(s)
            .map(Val::Ready)
            .map_err(|e| Error::str(format!("{e} (while parsing '{}')", String::from_utf8_lossy(s)))),
        j => Err(Error::str(format!("{j} cannot be parsed as JSON"))),
    };
    bome(parse(cv.1.force()))
}

/// jq 1.7's `debug`: `["DEBUG:",<input>]` on a line of stderr.
fn debug<'a>(cv: Cv<'a, D>) -> ValXs<'a, Val> {
    let line = format!("[\"DEBUG:\",{}]\n", cv.1.force());
    say(cv.0.data().run, line);
    Box::new(core::iter::empty())
}

/// jq 1.7's `stderr`: the input, compact and undecorated.
fn stderr<'a>(cv: Cv<'a, D>) -> ValXs<'a, Val> {
    let text = match cv.1.force() {
        J::TStr(s) => String::from_utf8_lossy(s).into_owned(),
        j => j.to_string(),
    };
    say(cv.0.data().run, text);
    Box::new(core::iter::empty())
}

/// Write to stderr once per record, from the first run that gets here clean.
fn say(run: &Run, text: String) {
    if run.say() {
        let _ = std::io::stderr().write_all(text.as_bytes());
    }
}

/// jq's `now`: the clock, read once per record where the first run that
/// gets here clean read it.
fn now<'a>(cv: Cv<'a, D>) -> ValXs<'a, Val> {
    box_once(Ok(Val::from(cv.0.data().run.now())))
}

/// jaq-json's natives are written for `jaq_json::Val` only. Each one is
/// bridged: force the input and argument, then run the jaq-json filter.
const BRIDGED: [(&str, usize); 7] =
    [("tojson", 0), ("tobytes", 0), ("length", 0), ("contains", 1), ("has", 1), ("indices", 1), ("bsearch", 1)];

type JFilter = jaq_core::Filter<JustLut<J>>;

thread_local! {
    static JSON_FILTERS: Vec<JFilter> = BRIDGED.iter().map(|&(name, arity)| {
        let code = if arity == 0 { name.to_owned() } else { format!("{name}($x)") };
        let arena = Arena::default();
        let modules = Loader::new([]).load(&arena, File { code: code.as_str(), path: () });
        let modules = modules.unwrap_or_else(|_| panic!("bridge {name}"));
        jaq_core::Compiler::default()
            .with_funs(jaq_json::funs::<JustLut<J>>())
            .with_global_vars(["$x"])
            .compile(modules)
            .unwrap_or_else(|_| panic!("bridge {name}"))
    }).collect();
}

fn bridge<'a, const I: usize>(mut cv: Cv<'a, D>) -> ValXs<'a, Val> {
    let arg = (BRIDGED[I].1 == 1).then(|| cv.0.pop_var());
    let run = || -> Result<Vec<ValR<J>>, Error> {
        let x = cv.1.owned();
        let a = arg.map(Val::owned);
        Ok(JSON_FILTERS.with(|fs| {
            let f = &fs[I];
            let ctx = Ctx::<JustLut<J>>::new(&f.lut, Vars::new(a));
            let halt = || jaq_core::Error::str("halt");
            f.id.run((ctx, x)).map(|r| r.map_err(|e| e.get_err().unwrap_or_else(|_| halt()))).collect()
        }))
    };
    match run() {
        Ok(ys) => Box::new(ys.into_iter().map(|y| liftr(y).map_err(Exn::from))),
        Err(e) => bome(Err(e)),
    }
}

pub fn funs() -> impl Iterator<Item = Fun<D>> {
    let bridged: [RunPtr<D>; 7] =
        [bridge::<0>, bridge::<1>, bridge::<2>, bridge::<3>, bridge::<4>, bridge::<5>, bridge::<6>];
    let bridged = BRIDGED.into_iter().zip(bridged).map(|((name, n), f)| (name, v(n), Native::<D>::new(f)));
    let ours: [(&'static str, usize, RunPtr<D>); 6] = [
        ("fromjson", 0, fromjson),
        ("judge", 2, judge),
        ("env", 0, env),
        ("now", 0, now),
        ("debug_empty", 0, debug),
        ("stderr_empty", 0, stderr),
    ];
    let names = ours.map(|o| o.0);
    let std = jaq_std::funs::<D>().filter(move |f| !names.contains(&f.0));
    let ours = ours.map(|(name, n, f)| (name, v(n), Native::<D>::new(f)));
    jaq_core::funs::<D>().chain(std).chain(bridged).chain(ours)
}
