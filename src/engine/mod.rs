//! The jq engine: jaq, run again from the start until it has every answer
//! it needs. Unknown answers never leave this module: `Program::run` hands
//! out plain values only.
//!
//! A record runs with the answers it has. The run is *clean* until its first
//! question without an answer; what a clean run does (its outputs, what it
//! writes to stderr, the times it reads) is committed, and every later run
//! repeats it exactly, since it gets the same answers to the same questions.
//! After that the run only looks ahead: it queues questions and commits
//! nothing. It stops where it needs an answer it doesn't have, and once its
//! questions are answered the record runs again. A run that ends clean is
//! the record's last: plain jq on known answers.

mod natives;
mod sig;
mod val;

use crate::transport::api::{Asked, Failure, Spec};
use jaq_core::data::HasLut;
use jaq_core::load::{self, Arena, File, Loader};
use jaq_core::{Ctx, DataT, Vars};
pub use sig::{Input, Sig};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;
pub use val::J;
use val::{Interrupt, Val, interrupt};

/// `ask`, `choose`, `rate` and `box`: jq, not Rust.
pub const PRELUDE: &str = include_str!("prelude.jq");
/// jq 1.7 builtins that jaq lacks, written in jq.
pub const COMPAT: &str = include_str!("compat.jq");

/// Outputs are printed this many at a time; and a run goes at most this
/// many outputs past its first unanswered question before it stops, so a
/// record with endless output stays bounded while `.[] | ask(q)` still asks
/// its elements together.
const CHUNK: usize = 64;

/// Where `judge` gets its answers.
pub trait Ask {
    /// The answer to a question about `state`, if the record has it;
    /// otherwise the question is queued for the record's next round.
    /// `Failed` is a jq error at the `judge` call; `Stopped` ends the record.
    fn ask(&self, state: &J, spec: Spec) -> Result<Asked, Failure>;
}

/// What `emit` did with outputs it was given: each is taken, and it says
/// whether the run may go on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Emitted {
    More,
    /// No more for now: the run pauses, and the record runs again later.
    Pause,
    /// The output is gone: the record ends.
    Closed,
}

pub struct D;

impl DataT for D {
    type V<'a> = Val;
    type Data<'a> = Data<'a>;
}

#[derive(Clone)]
pub struct Data<'a> {
    lut: &'a jaq_core::Lut<D>,
    run: &'a Run<'a>,
}

impl<'a> HasLut<'a, D> for Data<'a> {
    fn lut(&self) -> &'a jaq_core::Lut<D> {
        self.lut
    }
}

/// What a record's runs have had, which each later run repeats instead of
/// doing: the answers `judge` gave, by its arguments; how many outputs it
/// printed and lines it wrote to stderr; and the times it read, in order.
#[derive(Debug, Default)]
pub struct Journal {
    answers: HashMap<(J, J), Result<J, String>>,
    outs: u64,
    said: u64,
    times: Vec<f64>,
    /// how many failed answers the last run was given
    failed: usize,
}

impl Journal {
    /// How many failed answers the record's last run was given: if it ended
    /// without failing, the program caught them.
    pub fn failed(&self) -> usize {
        self.failed
    }
}

/// One run of a record.
struct Run<'a> {
    ask: &'a dyn Ask,
    journal: RefCell<Journal>,
    /// how many outputs, stderr lines and times this run has had
    outs: Cell<u64>,
    said: Cell<u64>,
    times: Cell<usize>,
    /// no question has gone unanswered yet
    clean: Cell<bool>,
    /// the failed answers this run was given
    failed: RefCell<HashSet<(J, J)>>,
}

/// What a run does with an output, or a line for stderr.
#[derive(PartialEq)]
enum Effect {
    /// an earlier run did it
    Repeat,
    /// do it: the run is clean
    Do,
    /// leave it: the run is only looking ahead
    Skip,
}

impl Run<'_> {
    /// What `judge` gave for these arguments before, in this run or an
    /// earlier one.
    fn answer(&self, args: &(J, J)) -> Option<Result<J, String>> {
        let a = self.journal.borrow().answers.get(args).cloned();
        if let Some(Err(_)) = a {
            self.failed.borrow_mut().insert(args.clone());
        }
        a
    }

    /// The answer `judge` gives for these arguments, in every run from now.
    fn keep(&self, args: (J, J), a: Result<J, String>) {
        if a.is_err() {
            self.failed.borrow_mut().insert(args.clone());
        }
        self.journal.borrow_mut().answers.insert(args, a);
    }

    fn effect(&self, n: &Cell<u64>, done: impl Fn(&mut Journal) -> &mut u64) -> Effect {
        let i = n.get();
        n.set(i + 1);
        let mut j = self.journal.borrow_mut();
        let done = done(&mut j);
        match () {
            _ if i < *done => Effect::Repeat,
            _ if self.clean.get() => {
                *done += 1;
                Effect::Do
            }
            _ => Effect::Skip,
        }
    }

    fn out(&self) -> Effect {
        self.effect(&self.outs, |j| &mut j.outs)
    }

    /// Whether to write a line to stderr.
    fn say(&self) -> bool {
        self.effect(&self.said, |j| &mut j.said) == Effect::Do
    }

    /// The time, as jq's `now`: what an earlier run read here, or the clock.
    fn now(&self) -> f64 {
        let i = self.times.get();
        self.times.set(i + 1);
        let mut j = self.journal.borrow_mut();
        if let Some(t) = j.times.get(i) {
            return *t;
        }
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        if self.clean.get() {
            j.times.push(t);
        }
        t
    }

    /// A question went unanswered: from here on the run only looks ahead.
    fn unclean(&self) {
        if self.clean.get() {
            self.repeated();
            self.clean.set(false);
        }
    }

    /// Up to its first unanswered question, a run does what the run before
    /// it did: it has the same answers to the same questions.
    fn repeated(&self) {
        let j = self.journal.borrow();
        if self.outs.get() < j.outs || self.said.get() < j.said || self.times.get() < j.times.len() {
            drop(j);
            interrupt(Interrupt::Diverged("a run of the record didn't repeat what the run before it did"));
        }
    }
}

pub struct Program {
    filter: jaq_core::Filter<D>,
    /// the named values, then the arguments, as JSON: global variables
    /// after `$ENV` (`J` can't cross threads)
    globals: Vec<String>,
    input: Input,
    tool: bool,
}

/// How a record ended.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Done,
    /// A jq error: the message for the user.
    Failed(String),
    /// The run is stopping, or the output is gone.
    Stopped,
    /// A bug in deem.
    Bug(String),
}

/// A run that isn't clean has queued a question, so waiting for none would
/// run it the same way again, for ever.
pub const NOTHING: &str = "a run of the record waited for no question";

/// How one run of a record ended.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// The record has ended.
    Done(Outcome),
    /// The record needs answers it doesn't have: wait for its round and run
    /// it again. The question it stopped on, if it looked at one.
    Rerun(Option<u64>),
    /// `emit` took no more: run the record again when it can.
    Paused,
}

impl Program {
    /// Compile `src`. `args` fill `main`'s parameters; `named` override the
    /// defaults its directive declares. Both are bound as variables, never
    /// pasted into the source.
    pub fn compile(src: &str, args: &[String], named: &[(String, String)]) -> Result<Self, String> {
        Self::compile_with(src, args, named, &[])
    }

    /// `compile`, and `bound`: strings bound to variables the program needn't
    /// declare (`--rawfile NAME FILE`, as in jq).
    pub fn compile_with(
        src: &str,
        args: &[String],
        named: &[(String, String)],
        bound: &[(String, String)],
    ) -> Result<Self, String> {
        loads_files(src).map_or(Ok(()), |at| Err(report(src, vec![(at, NO_FILES.into())])))?;
        let sig = Sig::of(src)?;
        let params = sig.params.as_deref().unwrap_or_default();
        if args.len() != params.len() {
            return Err(match &sig.params {
                None => format!("a program without main takes no arguments, got {}", args.len()),
                Some(ps) => {
                    let s = if ps.len() == 1 { "" } else { "s" };
                    format!("the program takes {} argument{s} ({}), got {}", ps.len(), ps.join(", "), args.len())
                }
            });
        }
        let mut values: Vec<J> = sig.defaults.iter().map(|(_, v)| v.clone()).collect();
        for (i, (name, text)) in named.iter().enumerate() {
            if named[..i].iter().any(|(n, _)| n == name) {
                return Err(format!("{name} is set twice"));
            }
            let at = sig.defaults.iter().position(|(n, _)| n == name);
            values[at.unwrap_or_default()] = sig.value(name, text)?;
        }
        for (i, (name, _)) in bound.iter().enumerate() {
            let ident = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !ident || name.starts_with("__") {
                return Err(format!("${name} isn't a name a program can use; use letters, digits and _"));
            }
            if bound[..i].iter().any(|(n, _)| n == name) || name == "ENV" {
                return Err(format!("${name} is set twice"));
            }
            if sig.defaults.iter().any(|(n, _)| n == name) || params.contains(name) {
                return Err(format!("${name} is the program's own; bind another name"));
            }
        }
        let mut names = vec!["$ENV".to_owned()];
        names.extend(sig.defaults.iter().map(|(n, _)| format!("${n}")));
        names.extend((0..args.len()).map(|i| format!("$__{i}")));
        names.extend(bound.iter().map(|(n, _)| format!("${n}")));
        values.extend(args.iter().map(|a| J::from(a.clone())));
        values.extend(bound.iter().map(|(_, v)| J::from(v.clone())));
        let values = values.iter().map(J::to_string).collect();
        let call = (0..args.len()).map(|i| format!("$__{i}")).collect::<Vec<_>>().join("; ");
        let full = match (&sig.params, args.len()) {
            (None, _) => src.to_owned(),
            (Some(_), 0) => format!("{src}\nmain"),
            (Some(_), _) => format!("{src}\nmain({call})"),
        };
        let src = full.as_str();
        let defs = jaq_core::defs().chain(jaq_std::defs()).chain(jaq_json::defs());
        let defs = defs.chain(defs_of(COMPAT)).chain(defs_of(PRELUDE));
        let arena = Arena::default();
        let modules = Loader::new(defs).load(&arena, File { code: src, path: () });
        let modules = modules.map_err(|errs| {
            let msgs = errs.into_iter().flat_map(|(_, e)| match e {
                load::Error::Io(e) => e.into_iter().map(|(p, e)| (p, format!("cannot load {p:?}: {e}"))).collect(),
                load::Error::Lex(e) => e.into_iter().map(|(x, at)| (at, format!("expected {}", x.as_str()))).collect(),
                load::Error::Parse(e) => {
                    e.into_iter().map(|(x, at)| (at, unexpected(x.as_str(), at))).collect::<Vec<_>>()
                }
            });
            report(src, msgs.collect())
        })?;
        let filter = jaq_core::Compiler::default()
            .with_funs(natives::funs())
            .with_global_vars(names.iter().map(String::as_str))
            .compile(modules);
        let filter = filter.map_err(|errs| {
            let msgs = errs.into_iter().flat_map(|(_, e)| e).map(|(name, u)| {
                let what = match u {
                    jaq_core::compile::Undefined::Filter(n) => format!("{name}/{n}"),
                    _ => name.to_owned(),
                };
                let hint = match what.as_str() {
                    "ask/1" | "choose/2" | "rate/2" | "box/1" | "judge/1" => {
                        "; say what to send as the last argument: ask(q; .msg) sends .msg, ask(q; .) the whole record"
                    }
                    _ => "",
                };
                (name, format!("{} {what} is not defined{hint}", u.as_str()))
            });
            report(src, msgs.collect())
        })?;
        Ok(Self { filter, globals: values, input: sig.input, tool: sig.params.is_some() })
    }

    /// What `main` gets: one record, or every record.
    pub fn input(&self) -> Input {
        self.input
    }

    /// Whether the program defines `main`.
    pub fn is_tool(&self) -> bool {
        self.tool
    }

    /// Run the program on one record, as many times as it takes. `emit`
    /// gets the outputs a chunk at a time. `wait` sends the record's round
    /// and waits until each of its questions is answered: it gets the
    /// question the run stopped on, if it looked at one, and returns how
    /// many it waited for, or `None` if the run is stopping.
    pub fn run(
        &self,
        input: J,
        ask: &dyn Ask,
        wait: &mut dyn FnMut(Option<u64>) -> Option<usize>,
        emit: &mut dyn FnMut(&[J]) -> Emitted,
    ) -> Outcome {
        let mut journal = Journal::default();
        loop {
            match self.step(&input, &mut journal, ask, emit) {
                Step::Done(outcome) => return outcome,
                Step::Paused => {}
                Step::Rerun(looked) => match wait(looked) {
                    None => return Outcome::Stopped,
                    Some(0) => return Outcome::Bug(NOTHING.into()),
                    Some(_) => {}
                },
            }
        }
    }

    /// One run of the program on a record, with what its earlier runs
    /// committed. What this run commits is emitted however it ends.
    pub fn step(&self, input: &J, journal: &mut Journal, ask: &dyn Ask, emit: &mut dyn FnMut(&[J]) -> Emitted) -> Step {
        quiet_panics();
        let run = Run {
            ask,
            journal: RefCell::new(std::mem::take(journal)),
            outs: Cell::new(0),
            said: Cell::new(0),
            times: Cell::new(0),
            clean: Cell::new(true),
            failed: RefCell::default(),
        };
        let mut chunk = Vec::new();
        let r = catch_unwind(AssertUnwindSafe(|| self.go(input.clone(), &run, &mut chunk, emit)));
        *journal = run.journal.into_inner();
        journal.failed = run.failed.into_inner().len();
        if !chunk.is_empty() && emit(&chunk) == Emitted::Closed {
            return Step::Done(Outcome::Stopped);
        }
        r.unwrap_or_else(|e| match e.downcast::<Interrupt>().map(|i| *i) {
            Ok(Interrupt::Rerun(q)) => Step::Rerun(q),
            Ok(Interrupt::Stopped) => Step::Done(Outcome::Stopped),
            Ok(Interrupt::Diverged(why)) => Step::Done(Outcome::Bug(why.into())),
            Err(e) => {
                let s = e.downcast_ref::<&str>().map(|s| s.to_string());
                Step::Done(Outcome::Bug(s.or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default()))
            }
        })
    }

    fn go(&self, input: J, run: &Run, chunk: &mut Vec<J>, emit: &mut dyn FnMut(&[J]) -> Emitted) -> Step {
        let data = Data { lut: &self.filter.lut, run };
        let env = Val::Ready(J::obj(Default::default()));
        let globals = self.globals.iter().map(|g| jaq_json::read::parse_single(g.as_bytes()).unwrap_or_default());
        let vars = Vars::new([env].into_iter().chain(globals.map(Val::Ready)));
        let mut outs = self.filter.id.run((Ctx::<D>::new(data, vars), Val::Ready(input)));
        let mut ahead = 0;
        let end = loop {
            match outs.next() {
                Some(Ok(v)) => match run.out() {
                    Effect::Repeat => {}
                    Effect::Do => {
                        let Val::Ready(j) = v else {
                            interrupt(Interrupt::Diverged("an output of a clean run waits on an answer"))
                        };
                        chunk.push(j);
                        if chunk.len() == CHUNK {
                            match emit(&std::mem::take(chunk)) {
                                Emitted::More => {}
                                Emitted::Pause => return Step::Paused,
                                Emitted::Closed => return Step::Done(Outcome::Stopped),
                            }
                        }
                    }
                    Effect::Skip => {
                        ahead += 1;
                        if ahead == CHUNK {
                            return Step::Rerun(None);
                        }
                    }
                },
                Some(Err(e)) => break Some(e),
                None => break None,
            }
        };
        if !run.clean.get() {
            return Step::Rerun(None);
        }
        run.repeated();
        Step::Done(match end.map(|e| e.get_err()) {
            None => Outcome::Done,
            Some(Ok(e)) => Outcome::Failed(message(e)),
            // `halt` ends the record without an error.
            Some(Err(_)) => Outcome::Done,
        })
    }
}

/// Whether `name` is a jq builtin (of any arity, deem's included) or a
/// keyword: a word that already means something in a program.
pub fn is_builtin(name: &str) -> bool {
    const KEYWORDS: [&str; 18] = [
        "def", "if", "then", "elif", "else", "end", "as", "reduce", "foreach", "try", "catch", "label", "import",
        "include", "and", "or", "module", "__loc__",
    ];
    let defs = jaq_core::defs().chain(jaq_std::defs()).chain(jaq_json::defs());
    let mut defs = defs.chain(defs_of(COMPAT)).chain(defs_of(PRELUDE));
    KEYWORDS.contains(&name) || natives::funs().any(|f| f.0 == name) || defs.any(|d| d.name == name)
}

const NO_FILES: &str = "import and include are not supported: a deem program can't read files";

/// Where the program imports or includes a module or data file, if it
/// does. jaq leaves reading them to its caller; deem refuses.
fn loads_files(src: &str) -> Option<&str> {
    use load::lex::{Lexer, Tok, Token};
    let tokens = Lexer::new(src).lex().ok()?;
    // `module {…};` may come first
    let first = match tokens.first()? {
        Token("module", _) => tokens.iter().skip_while(|t| t.0 != ";").nth(1)?,
        t => t,
    };
    (matches!(first.1, Tok::Word) && ["import", "include"].contains(&first.0)).then_some(first.0)
}

fn defs_of(code: &'static str) -> Vec<load::parse::Def<&'static str>> {
    load::parse(code, |p| p.defs()).expect("the built-in definitions parse")
}

/// An uncaught error's message.
fn message(e: jaq_core::Error<Val>) -> String {
    match e.into_val().owned() {
        J::TStr(s) => String::from_utf8_lossy(&s).into_owned(),
        j => format!("{j} (not a string)"),
    }
}

fn unexpected(expected: &str, found: &str) -> String {
    match found {
        "" => format!("expected {expected}, but the program ended"),
        found => format!("expected {expected}, found {found:?}"),
    }
}

/// Compile errors, each with its line:column, the line, and a caret.
fn report(src: &str, errs: Vec<(&str, String)>) -> String {
    let errs = errs.into_iter().map(|(at, msg)| {
        let off = at.as_ptr() as usize;
        let base = src.as_ptr() as usize;
        let off = if (base..=base + src.len()).contains(&off) { off - base } else { 0 };
        let line_start = src[..off].rfind('\n').map_or(0, |i| i + 1);
        let line = src[line_start..].lines().next().unwrap_or("");
        let (row, col) = (src[..off].matches('\n').count() + 1, src[line_start..off].chars().count() + 1);
        let caret = format!("{}^", " ".repeat(col - 1));
        // jq accepts `.5`; jaq wants a leading digit.
        let near = &src.as_bytes()[off.saturating_sub(1)..(off + 2).min(src.len())];
        let bare = near.windows(2).any(|w| w[0] == b'.' && w[1].is_ascii_digit());
        let hint = if bare { "\n  numbers need a leading digit: write 0.5, not .5" } else { "" };
        format!("{row}:{col}: {msg}\n    {line}\n    {caret}{hint}")
    });
    errs.collect::<Vec<_>>().join("\n")
}

/// A run that stops unwinds to `Program::step` on purpose; that is not a
/// bug, so the panic hook stays quiet about it.
fn quiet_panics() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let p = info.payload();
            if !p.is::<Interrupt>() {
                default(info)
            }
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::api::Answer;

    /// Answers by run: `answers(question, run)`.
    struct Script {
        runs: Cell<usize>,
        answers: fn(&str, usize) -> Asked,
    }

    impl Ask for Script {
        fn ask(&self, _: &J, spec: Spec) -> Result<Asked, Failure> {
            Ok((self.answers)(&spec.q, self.runs.get()))
        }
    }

    fn answer(x: f64) -> Asked {
        Asked::Answer(Ok(Answer { value: serde_json::json!(x), confidence: None, probabilities: None }))
    }

    /// Each wait waits for `waited` questions.
    fn run(program: &str, answers: fn(&str, usize) -> Asked, waited: usize) -> (Outcome, Vec<String>) {
        let p = Program::compile(program, &[], &[]).unwrap();
        let ask = Script { runs: Cell::new(0), answers };
        let mut out = Vec::new();
        let mut wait = |_| {
            ask.runs.set(ask.runs.get() + 1);
            Some(waited)
        };
        let outcome = p.run(J::Null, &ask, &mut wait, &mut |js| {
            out.extend(js.iter().map(J::to_string));
            Emitted::More
        });
        (outcome, out)
    }

    // A record gets one answer to a question, however many times it runs:
    // the journal keeps the first, so a source that would answer
    // differently later can't change what the record printed.
    #[test]
    fn a_record_keeps_its_first_answer() {
        let program = r#"(if ask("q1"; .) > 0.5 then 1 else empty end), (ask("q2"; .) > 0)"#;
        let answers = |q: &str, run: usize| match (q, run) {
            ("q1", 0) | ("q2", 0 | 1) => Asked::Later(run as u64 + 1),
            ("q1", 1) => answer(0.9),
            _ => answer(0.1),
        };
        assert_eq!(run(program, answers, 1), (Outcome::Done, vec!["1".to_owned(), "true".to_owned()]));
    }

    // A run that isn't clean has queued a question, so waiting for none
    // would rerun the same run forever: a bug.
    #[test]
    fn waiting_for_nothing_is_a_bug() {
        let (outcome, _) = run(r#"ask("q"; .) > 0"#, |_, _| Asked::Later(1), 0);
        assert_eq!(outcome, Outcome::Bug(NOTHING.into()));
    }

    // `Full` stops the run; the next one asks the question again.
    #[test]
    fn a_full_round_stops_the_run() {
        let answers = |q: &str, run: usize| match (q, run) {
            ("q2", 0) => Asked::Full,
            ("q1", 0) | ("q2", 1) => Asked::Later(1),
            _ => answer(0.5),
        };
        let (outcome, out) = run(r#"[ask("q1"; .), ask("q2"; .)] | add"#, answers, 1);
        assert_eq!((outcome, out), (Outcome::Done, vec!["1.0".to_owned()]));
    }

    // `builtins` lists deem's own: `judge` and each function of the prelude.
    #[test]
    fn builtins_lists_deems_own() {
        let (outcome, out) = run("builtins", |_, _| Asked::Later(1), 1);
        assert_eq!(outcome, Outcome::Done);
        let listed: Vec<String> = serde_json::from_str(&out[0]).unwrap();
        let ours = defs_of(PRELUDE).into_iter().map(|d| format!("{}/{}", d.name, d.args.len()));
        for name in ours.chain(["judge/2".to_owned()]) {
            assert!(listed.contains(&name), "builtins lacks {name}");
        }
    }

    #[test]
    fn builtins_compile() {
        if let Err(e) = Program::compile(".", &[], &[]) {
            panic!("{e}");
        }
    }

    #[test]
    fn compile_errors_point_at_the_program() {
        let e = Program::compile("choose(\"which?\", [\"a\",\"b\"])", &[], &[]).err().unwrap();
        assert!(e.starts_with("1:1: filter choose/1 is not defined"), "{e}");
        let e = Program::compile("ask(\"q\")", &[], &[]).err().unwrap();
        assert!(e.contains("ask/1 is not defined; say what to send as the last argument"), "{e}");
        for (program, missing) in [("rate(\"q\"; [\"a\",\"b\"])", "rate/2"), ("box(\"q\")", "box/1")] {
            let e = Program::compile(program, &[], &[]).err().unwrap();
            assert!(e.contains(&format!("{missing} is not defined; say what to send")), "{e}");
        }
        let e = Program::compile("select(. > .5)", &[], &[]).err().unwrap();
        assert!(e.contains("write 0.5, not .5"), "{e}");
        let e = Program::compile("1 +\n  (2", &[], &[]).err().unwrap();
        assert!(e.starts_with("2:5: expected closing parenthesis"), "{e}");
        assert!(Program::compile("import \"a\" as a; .", &[], &[]).is_err());
        assert!(Program::compile("input", &[], &[]).is_err());
    }
}
