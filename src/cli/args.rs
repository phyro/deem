//! The command line: subcommands, options and the usage text.

use deem::RunOptions;
use deem::apis::{self, Change};
use deem::programs::Mode;
use lexopt::prelude::*;
use std::ffi::OsString;

pub const USAGE: &str = r#"deem - grep by meaning: ask a question about every line

USAGE
  deem [OPTIONS] QUESTION [FILE...]              keep lines where it's yes
  deem [OPTIONS] QUESTION -p [FILE...]           score<TAB>record
  deem [OPTIONS] QUESTION -o A,B,C [FILE...]     option<TAB>record
  deem [OPTIONS] QUESTION -l A,B,C [FILE...]     level<TAB>record
  deem run [OPTIONS] PROGRAM [ARG...] [FILE...]  a name, a path, or jq text
  deem tune [OPTIONS] QUESTION [FILE...]         label lines, get a threshold
  deem programs [NAME]                           list programs, or print one
  deem api [add | edit | remove | default] ...   APIs to send to
  deem trace [--visual] FILE                     show a run --trace recorded
  deem trace compare --with NAME FILE...         each API's answers in the
                                                 traces against NAME's
  deem models | help | version

Options go anywhere, before run or tune too; the words that aren't options
are the question (or the program and its arguments), then files. A question
that starts with -, or is the word run or tune, goes after --.

  -p, --score        print each line's score in front of it
  -o, --options A,B  print the option each line fits best in front of it
  -l, --levels A,B   print each line's level in front of it: 0 is A, 1 B, ...
  -n N               deem tune: how many lines to label         (default 20)
  -t, --threshold N  keep lines scoring above N, in [0,1]  (default 0.5)
                     the same as --arg threshold N
      --arg NAME V   set a named value the program declares
      --rawfile NAME FILE  $NAME is the file's text, as in jq; the program
                     needn't declare it
      --img NAME FILE      $NAME is the picture (PNG or JPEG)

PROGRAMS are jq with judge(spec; ctx), and ask(q; ctx), choose(q; opts;
ctx), rate(q; levels; ctx) and box(q; ctx) built on it; ctx is all the
model sees. Built in: group, histogram, mix, disagree; `deem programs`
lists them and yours (the nearest .deem/ from here up, ~/.deem/).

  -J, --json         each JSON value is a record; without it, each line
  -I, --images       each stdin line is a picture's path (picture files
                     given as FILE need no flag)
  -s, --slurp        the whole input is one record (-J: an array of the
                     values; not with input "all")
      --split RE     record separator                       (default \n)
  -r, --raw-output   print strings unquoted
  -e, --exit-status  exit 1 if the last output was false or null

      --max-questions N  stop after N questions; the run is partial
                     (default 100000, 0: none)
      --stats        requests, tokens, tokens per 1000 records, score
                     histogram -> stderr
      --trace FILE   record the run, one event per line; see deem trace --help
      --failed FILE  the records that didn't finish, as read, to run again
      --api NAME     the API to send to                   (default: the
                     default API; DEEM_API=NAME sets it for a shell)

APIS are in ~/.config/deem/config.toml, their keys in ~/.local/state/deem/keys.
  deem api                       list them; * marks the default
  deem api add NAME URL [SET]    add one; the first added is the default
  deem api edit NAME SET         change one
  deem api remove NAME           remove one, and its key
  deem api default [NAME]        print, or set, the default
  SET is any of --url U, --model M, --pictures or --no-pictures, --key or
  --no-key (--key reads it from the terminal, or stdin), --rps N, --tps N,
  --parallel N (the most requests at the API at once: as many as your
  server runs at once, e.g. llama-server's --parallel)

ENVIRONMENT
  DEEM_API DEEM_MAX_RECORD_BYTES
  without APIs (deem api), the API is JEV_BASE_URL, JEV_MODEL, JEV_API_KEY,
  JEV_REQUESTS_PER_SECOND, JEV_TOKENS_PER_SECOND and JEV_PARALLEL

EXIT  0 output produced - 1 nothing selected, or -e and last output false/null
      2 usage error, program error, failed record, or API error
      141 the output was closed

More: README.md and docs/MANUAL.md
"#;

/// `deem trace --help`: the trace commands, with examples.
pub const TRACE_USAGE: &str = r#"deem trace - show a run --trace recorded, or compare runs against APIs

USAGE
  deem trace [--visual] FILE                     a run's waves and requests,
                                                 or (--visual) a timeline
  deem trace compare --with NAME FILE...         each API's answers in the
                                                 traces against NAME's

A RUN
  deem run triage app.log --trace run.jsonl      record it
  deem trace run.jsonl                           each wave, its requests, and
                                                 what each question waited for
  deem trace --visual run.jsonl                  the same run as a timeline

COMPARING APIS (in examples/: a trace of each example program)
  sh trace.sh jev                                the hosted API
  sh trace.sh kev                                a server of your own
  deem trace compare --with jev traces/*.jsonl   kev's answers against jev's

Traces of any runs compare: answers are matched by question and value.
More: docs/MANUAL.md, "Seeing a run" and "Comparing APIs"
"#;

/// How `deem trace` shows a trace.
#[derive(Debug, PartialEq)]
pub enum View {
    Text,
    Visual,
}

#[derive(Debug, PartialEq)]
pub enum Cmd {
    /// `deem QUESTION [-p | -o A,B] FILE…`
    Ask {
        question: String,
        mode: Mode,
        files: Vec<String>,
        opts: RunOptions,
    },
    /// `deem run WORD ARG… FILE…`: the arguments, then the files, in `rest`;
    /// how many are arguments depends on the program
    Run {
        word: String,
        rest: Vec<String>,
        opts: RunOptions,
    },
    /// `deem tune QUESTION FILE…`, and how many lines to label (`-n`)
    Tune {
        question: String,
        count: usize,
        files: Vec<String>,
        opts: RunOptions,
    },
    /// `deem programs [NAME]`
    Programs(Option<String>),
    /// `deem api …` that reads or changes the config file
    Api(ApiCmd),
    Help,
    /// `deem trace --help`
    TraceHelp,
    Version,
    Models,
    /// A `--trace` file, and how to show it.
    Trace(String, View),
    /// `deem trace compare --with NAME FILE…`
    TraceCompare {
        with: String,
        files: Vec<String>,
    },
}

/// `deem api …`
#[derive(Debug, PartialEq)]
pub enum ApiCmd {
    List,
    /// `key`: read one and keep it
    Add {
        api: apis::Api,
        key: bool,
    },
    /// `key`: read one (`Some(true)`), or remove it (`Some(false)`)
    Edit {
        name: String,
        change: Change,
        key: Option<bool>,
    },
    Remove(String),
    Default(Option<String>),
}

/// The command, and the API `--api` chooses for it.
pub fn parse(mut args: Vec<OsString>) -> Result<(Cmd, Option<String>), String> {
    // `--api NAME` before a subcommand
    let first = args.first().and_then(|a| a.to_str()).map(str::to_owned);
    if let Some(f) = first.filter(|f| *f == "--api" || f.starts_with("--api=")) {
        let name = match f.strip_prefix("--api=") {
            Some(n) => n.to_owned(),
            None if args.len() > 1 => text(args.remove(1))?,
            None => return Err("--api: give the name of an API (`deem api` lists them)".into()),
        };
        args.remove(0);
        return match parse(args)? {
            (Cmd::Api(_), _) => Err("--api chooses a run's API; `deem api` lists and changes them".into()),
            (_, Some(_)) => Err("--api is given twice".into()),
            (cmd, None) => Ok((cmd, Some(name))),
        };
    }
    let sub = args.first().and_then(|a| a.to_str());
    // `--help` after any subcommand is the help, as after `run` and `tune`
    let help = args[1.min(args.len())..].iter().any(|a| a == "--help" || a == "-h");
    match sub {
        Some("trace") if help => return Ok((Cmd::TraceHelp, None)),
        Some("api" | "models" | "programs") if help => return Ok((Cmd::Help, None)),
        _ => {}
    }
    match sub {
        Some("help") => return Ok((Cmd::Help, None)),
        Some("version") => return Ok((Cmd::Version, None)),
        Some("api") => return Ok((api(args[1..].to_vec())?, None)),
        Some("models") => {
            return match &args[1..] {
                [] => Ok((Cmd::Models, None)),
                [flag, name] if flag == "--api" => Ok((Cmd::Models, Some(text(name.clone())?))),
                [flag] if flag.to_str().is_some_and(|f| f.starts_with("--api=")) => {
                    Ok((Cmd::Models, Some(text(flag.clone())?["--api=".len()..].to_owned())))
                }
                _ => Err("usage: deem models [--api NAME]".into()),
            };
        }
        Some("programs") => {
            return match &args[1..] {
                [] => Ok((Cmd::Programs(None), None)),
                [name] => Ok((Cmd::Programs(Some(text(name.clone())?)), None)),
                _ => Err("usage: deem programs [NAME]".into()),
            };
        }
        Some("trace") => {
            let rest: Vec<&str> = args[1..].iter().filter_map(|a| a.to_str()).collect();
            let usage = || Err("usage: deem trace [--visual] FILE, or deem trace compare --with NAME FILE...".into());
            if rest.len() != args.len() - 1 {
                return usage();
            }
            if rest.first() == Some(&"compare") {
                return trace_compare(&rest[1..]).map(|c| (c, None));
            }
            return match rest[..] {
                [file] if !file.starts_with('-') => Ok((Cmd::Trace(file.into(), View::Text), None)),
                ["--visual", file] | [file, "--visual"] => Ok((Cmd::Trace(file.into(), View::Visual), None)),
                _ => usage(),
            };
        }
        _ => {}
    }
    let (opts, mut words, mode, count, literal, chosen) = match options(args)? {
        Parsed::Opts(o, words, (mode, count, chosen), literal) => (o, words, mode, count, literal, chosen),
        Parsed::Cmd(c) => return Ok((c, None)),
    };
    let cmd = command(opts, &mut words, (mode, count), literal)?;
    Ok((cmd, chosen))
}

/// A question, `run` or `tune`, from the options and the other words.
fn command(
    opts: RunOptions,
    words: &mut Vec<String>,
    (mode, count): (Option<Mode>, Option<usize>),
    literal: Option<usize>,
) -> Result<Cmd, String> {
    // `run` or `tune` is the first word, wherever the options are, unless it
    // is after `--`: then it is a question
    let sub = words.first().filter(|w| ["run", "tune"].contains(&w.as_str()) && literal != Some(0)).cloned();
    if sub.is_some() {
        words.remove(0);
    }
    if count.is_some() && sub.as_deref() != Some("tune") {
        return Err("-n is for deem tune: how many lines to label".into());
    }
    if sub.is_some() && mode.is_some() {
        return Err("-p, -o and -l are for a question: deem \"QUESTION\" -p FILE".into());
    }
    let first = (!words.is_empty()).then(|| words.remove(0));
    let files = std::mem::take(words);
    match (sub.as_deref(), first) {
        (Some("run"), Some(word)) => Ok(Cmd::Run { word, rest: files, opts }),
        (Some("run"), None) => Err("usage: deem run PROGRAM [ARG...] [FILE...]. Try `deem programs`".into()),
        (Some(_), _) if !opts.named.is_empty() => {
            Err("deem tune takes no -t or --arg: it finds the threshold itself".into())
        }

        (Some(_), Some(question)) => {
            Ok(Cmd::Tune { question, count: count.unwrap_or(super::tune::SHOWN), files, opts })
        }
        (Some(_), None) => Err("usage: deem tune QUESTION [FILE...]".into()),
        (None, None) => Err("no question. Try `deem \"is this an error?\" app.log` or `deem --help`".into()),
        (None, Some(question)) => {
            let mode = mode.unwrap_or(Mode::Filter);
            if mode != Mode::Filter && opts.named.iter().any(|(n, _)| n == "threshold") {
                return Err("-t doesn't go with -p, -o or -l: they print every line. \
                     To keep some, filter the scores: deem \"…\" -p FILE | awk '$1 > 0.8'"
                    .into());
            }
            Ok(Cmd::Ask { question, mode, files, opts })
        }
    }
}

/// `deem trace compare`'s arguments: `--with NAME`, anywhere, and files.
fn trace_compare(rest: &[&str]) -> Result<Cmd, String> {
    const USAGE: &str =
        "usage: deem trace compare --with NAME FILE...: NAME is the API whose answers are taken as right";
    let (mut with, mut files) = (None, Vec::new());
    let mut i = 0;
    while i < rest.len() {
        let w = match rest[i] {
            "--with" => {
                i += 1;
                Some(rest.get(i).copied().ok_or(USAGE)?)
            }
            a => a.strip_prefix("--with="),
        };
        match w {
            Some(_) if with.is_some() => return Err("--with is given twice".into()),
            Some(n) => with = Some(n.to_owned()),
            None if rest[i].starts_with('-') => return Err(format!("{}: not an option here. {USAGE}", rest[i])),
            None => files.push(rest[i].to_owned()),
        }
        i += 1;
    }
    match (with, files.is_empty()) {
        (Some(with), false) => Ok(Cmd::TraceCompare { with, files }),
        _ => Err(USAGE.into()),
    }
}

/// After `--key`, what deem says instead of what was typed: it may be the key.
const KEY_TAKES_NO_VALUE: &str = "--key takes no value: deem asks for the key, or reads it from stdin. \
     What you typed after it wasn't saved, but it is in your shell's history: if it was your key, replace it";

/// An option that takes no value given one (`--key=…`), named without the
/// value: after `--key` it may be the key, and an error is often pasted
/// where others read it.
fn unexpected(e: lexopt::Error) -> String {
    match e {
        lexopt::Error::UnexpectedValue { option, .. } if option == "--key" => KEY_TAKES_NO_VALUE.into(),
        lexopt::Error::UnexpectedValue { option, .. } => format!("{option} takes no value"),
        e => e.to_string(),
    }
}

fn text(v: OsString) -> Result<String, String> {
    v.into_string().map_err(|v| format!("{v:?} is not UTF-8"))
}

enum Parsed {
    /// the run's options, the other words, `-p`/`-o`, `-n` and `--api`, and
    /// how many of the words came before `--`, if it was there
    Opts(RunOptions, Vec<String>, (Option<Mode>, Option<usize>, Option<String>), Option<usize>),
    Cmd(Cmd),
}

/// The options, anywhere, and the other words in order.
fn options(args: Vec<OsString>) -> Result<Parsed, String> {
    let mut p = lexopt::Parser::from_args(args);
    let mut o = RunOptions::default();
    let mut words = Vec::new();
    let mut mode = None;
    let mut count = None;
    let mut chosen = None;
    let mut literal = None;
    let value = |p: &mut lexopt::Parser| p.value().map_err(|e| e.to_string()).and_then(text);
    let mut set = |m: Mode| match mode.replace(m) {
        Some(_) => Err("-p, -o and -l don't go together: -p scores, -o picks an option, -l a level".to_owned()),
        None => Ok(()),
    };
    loop {
        if literal.is_none() && p.try_raw_args().is_some_and(|raw| raw.peek() == Some(std::ffi::OsStr::new("--"))) {
            literal = Some(words.len());
        }
        let Some(arg) = p.next().map_err(unexpected)? else { break };
        match arg {
            Short('p') | Long("score") => set(Mode::Score)?,
            Short('n') => {
                let v = value(&mut p)?;
                count = Some(v.parse().ok().filter(|&n: &usize| n > 0).ok_or(format!("-n: {v:?} is not a count"))?);
            }
            Short('o') | Long("options") => {
                let v = value(&mut p)?;
                if v.split(',').any(|o| o.trim().is_empty()) || !v.contains(',') {
                    return Err(format!("-o: {v:?} is not a list of options, like -o bug,billing,other"));
                }
                set(Mode::Label(v))?
            }
            Short('l') | Long("levels") => {
                let v = value(&mut p)?;
                let (min, max) = deem::api::LEVELS;
                let n = v.split(',').count();
                if v.split(',').any(|l| l.trim().is_empty()) || !(min..=max).contains(&n) {
                    return Err(format!(
                        "-l: {v:?} is not a scale of {min} to {max} levels, lowest first, like -l low,medium,high"
                    ));
                }
                set(Mode::Rate(v))?
            }
            Short('t') | Long("threshold") => {
                let v = value(&mut p)?;
                match v.parse::<f64>() {
                    Ok(t) if (0.0..=1.0).contains(&t) => o.named.push(("threshold".into(), v)),
                    Ok(_) => return Err(format!("-t: {v} is not between 0 and 1")),
                    Err(_) => return Err(format!("-t: {v:?} is not a number")),
                }
            }
            Long("arg") => {
                let name = value(&mut p)?;
                let v = value(&mut p)?;
                o.named.push((name, v));
            }
            Long("rawfile") => {
                let name = value(&mut p)?;
                let file = value(&mut p)?;
                o.rawfile.push((name, file));
            }
            Long("img") => {
                let name = value(&mut p)?;
                let file = value(&mut p)?;
                o.img.push((name, file));
            }
            Long("api") => {
                if chosen.replace(value(&mut p)?).is_some() {
                    return Err("--api is given twice".into());
                }
            }
            Long("split") => o.split = Some(value(&mut p)?),
            Long("trace") => o.trace = Some(value(&mut p)?),
            Long("failed") => o.failed = Some(value(&mut p)?),
            Long("max-questions") => {
                let v = value(&mut p)?;
                o.max_questions = Some(v.parse().map_err(|_| format!("--max-questions: {v:?} is not a whole number"))?);
            }
            Short('J') | Long("json") => o.json_in = true,
            Short('I') | Long("images") => o.paths_in = true,
            Short('s') | Long("slurp") => o.slurp = true,
            Short('r') | Long("raw-output") => o.raw_out = true,
            Short('e') | Long("exit-status") => o.exit_status = true,
            Long("stats") => o.stats = true,
            Short('h') | Long("help") => return Ok(Parsed::Cmd(Cmd::Help)),
            Long("version") => return Ok(Parsed::Cmd(Cmd::Version)),
            Value(v) => words.push(text(v)?),
            arg => return Err(format!("{}. Try `deem --help`", arg.unexpected())),
        }
    }
    for (i, (name, _)) in o.named.iter().enumerate() {
        if o.named[..i].iter().any(|(n, _)| n == name) {
            let how = if name == "threshold" { " (-t is --arg threshold)" } else { "" };
            return Err(format!("{name} is set twice{how}"));
        }
    }
    Ok(Parsed::Opts(o, words, (mode, count, chosen), literal))
}

/// `deem api [add | edit | remove | default] …`
fn api(args: Vec<OsString>) -> Result<Cmd, String> {
    const USAGE: &str = "usage: deem api [add NAME URL | edit NAME | remove NAME | default [NAME]]; see `deem --help`";
    let mut p = lexopt::Parser::from_args(args);
    let value = |p: &mut lexopt::Parser| p.value().map_err(|e| e.to_string()).and_then(text);
    let number = |p: &mut lexopt::Parser, flag: &str| -> Result<u64, String> {
        let v = value(p)?;
        v.parse().map_err(|_| format!("{flag}: {v:?} is not a whole number"))
    };
    let (mut words, mut change, mut key) = (Vec::new(), Change::default(), None);
    let set = |slot: &mut Option<bool>, v: bool, what: &str| match slot.replace(v) {
        Some(_) => Err(format!("give --{what} or --no-{what} once")),
        None => Ok(()),
    };
    while let Some(arg) = p.next().map_err(unexpected)? {
        match arg {
            Long("url") => {
                let u = value(&mut p)?;
                apis::check_url(&u).map_err(|e| format!("--url: {e}"))?;
                change.url = Some(u);
            }
            Long("model") => change.model = Some(value(&mut p)?),
            Long("pictures") => set(&mut change.pictures, true, "pictures")?,
            Long("no-pictures") => set(&mut change.pictures, false, "pictures")?,
            Long("key") => set(&mut key, true, "key")?,
            Long("no-key") => set(&mut key, false, "key")?,
            Long("rps") => {
                let n = number(&mut p, "--rps")?;
                apis::check_rps(n).map_err(|e| format!("--rps: {e}"))?;
                change.rps = Some(n);
            }
            Long("tps") => {
                let n = number(&mut p, "--tps")?;
                apis::check_tps(n).map_err(|e| format!("--tps: {e}"))?;
                change.tps = Some(n);
            }
            Long("parallel") => {
                let n = number(&mut p, "--parallel")?;
                apis::check_parallel(n).map_err(|e| format!("--parallel: {e}"))?;
                change.parallel = Some(n);
            }
            Value(v) => words.push(text(v)?),
            arg => return Err(format!("{}. {USAGE}", arg.unexpected())),
        }
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let none = change.is_empty() && key.is_none();
    let name = |n: &str| apis::check_name(n).map(|_| n.to_owned());
    // with --key, a word too many may be the key, and so may a URL that
    // isn't one: neither is shown
    if key == Some(true) {
        match words[..] {
            ["add", _, u] if !(u.starts_with("https://") || u.starts_with("http://")) => {
                return Err(format!(
                    "the URL isn't an http:// or https:// URL (not shown: after --key, it may be the key). \
                     {KEY_TAKES_NO_VALUE}"
                ));
            }
            ["add", ..] if words.len() > 3 => return Err(KEY_TAKES_NO_VALUE.into()),
            ["edit", ..] if words.len() > 2 => return Err(KEY_TAKES_NO_VALUE.into()),
            _ => {}
        }
    }
    match words[..] {
        [] if none => Ok(Cmd::Api(ApiCmd::List)),
        ["add", n, url] => {
            if change.url.is_some() {
                return Err("deem api add takes the URL after the name, not as --url".into());
            }
            if key == Some(false) {
                return Err("--no-key is for deem api edit: a new API has no key unless --key".into());
            }
            apis::check_url(url)?;
            let model = change.model.unwrap_or_else(|| apis::DEFAULT_MODEL.into());
            let pictures = change.pictures.unwrap_or(false);
            let (rps, tps, parallel) = (change.rps, change.tps, change.parallel);
            let api = apis::Api { name: name(n)?, url: url.into(), model, pictures, rps, tps, parallel };
            Ok(Cmd::Api(ApiCmd::Add { api, key: key == Some(true) }))
        }
        ["edit", n] if none => Err(format!(
            "nothing to change: give --url, --model, --pictures or --no-pictures, --key or --no-key, --rps, --tps or --parallel (deem api edit {n} --model M)"
        )),
        ["edit", n] => Ok(Cmd::Api(ApiCmd::Edit { name: name(n)?, change, key })),
        ["remove", n] if none => Ok(Cmd::Api(ApiCmd::Remove(name(n)?))),
        ["default"] if none => Ok(Cmd::Api(ApiCmd::Default(None))),
        ["default", n] if none => Ok(Cmd::Api(ApiCmd::Default(Some(name(n)?)))),
        ["add" | "edit" | "remove" | "default", ..] if !none => {
            Err(format!("deem api {} takes no settings. {USAGE}", words[0]))
        }
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cmd, String> {
        with_api(args).map(|(c, _)| c)
    }

    fn with_api(args: &[&str]) -> Result<(Cmd, Option<String>), String> {
        super::parse(args.iter().map(OsString::from).collect())
    }

    fn ask(args: &[&str]) -> (String, Mode, Vec<String>, RunOptions) {
        match parse(args) {
            Ok(Cmd::Ask { question, mode, files, opts }) => (question, mode, files, opts),
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn question_forms() {
        let (q, mode, files, o) = ask(&["-Jse", "-t.8", "--split", r"\0", "is it?", "a.log", "b.log"]);
        assert!(o.json_in && o.slurp && o.exit_status);
        assert_eq!((&o.named[..], o.split.as_deref()), (&[("threshold".into(), ".8".into())][..], Some(r"\0")));
        assert_eq!((q.as_str(), mode, files.len()), ("is it?", Mode::Filter, 2));
        let (q, mode, files, o) = ask(&["q", "f", "-p", "--max-questions=7"]);
        assert_eq!((q.as_str(), mode, o.max_questions, &files[..]), ("q", Mode::Score, Some(7), &["f".to_owned()][..]));
        assert_eq!(ask(&["-o", "a, b", "q"]).1, Mode::Label("a, b".into()));
        assert_eq!(ask(&["--levels", "can wait, now", "q"]).1, Mode::Rate("can wait, now".into()));
        assert_eq!(ask(&["--", "-q"]).0, "-q");
    }

    #[test]
    fn subcommands() {
        let Ok(Cmd::Run { word, rest, opts }) = parse(&["run", "mix", "-t", "0.7", "q", "f"]) else { panic!() };
        assert_eq!((word.as_str(), &rest[..], opts.named.len()), ("mix", &["q".to_owned(), "f".to_owned()][..], 1));
        let Ok(Cmd::Tune { question, count, files, .. }) = parse(&["tune", "q", "f"]) else { panic!() };
        assert_eq!((question.as_str(), count, &files[..]), ("q", 20, &["f".to_owned()][..]));
        let Ok(Cmd::Tune { count, .. }) = parse(&["tune", "-n", "50", "q"]) else { panic!() };
        assert_eq!(count, 50);
        assert_eq!(parse(&["programs", "mix"]), Ok(Cmd::Programs(Some("mix".into()))));
        assert_eq!(parse(&["run", "--", "-x"]).map(|c| matches!(c, Cmd::Run { .. })), Ok(true));
        // options before the subcommand; after `--`, `run` is a question
        let Ok(Cmd::Run { word, rest, opts }) = parse(&["-J", "-t", "0.7", "run", "mix", "q"]) else { panic!() };
        assert_eq!((word.as_str(), &rest[..], opts.json_in, opts.named.len()), ("mix", &["q".to_owned()][..], true, 1));
        assert!(matches!(parse(&["-J", "tune", "q"]), Ok(Cmd::Tune { .. })));
        assert_eq!(ask(&["-J", "--", "run", "f"]).0, "run");
        assert_eq!(ask(&["--", "tune"]).0, "tune");
    }

    #[test]
    fn errors() {
        let err = |args: &[&str]| parse(args).unwrap_err();
        assert!(err(&["-x"]).contains("-x"));
        assert!(err(&["--xyz"]).contains("--xyz"));
        assert!(err(&["-t", "abc", "q"]).contains("not a number"));
        assert_eq!(err(&["-t", "1.5", "q"]), "-t: 1.5 is not between 0 and 1");
        assert!(err(&["q", "-p", "-t", "0.5"]).contains("awk '$1 > 0.8'"));
        assert!(err(&["q", "-o", "a,b", "-p"]).starts_with("-p, -o and -l don't go together"));
        assert!(err(&["q", "-o", "a,b", "-l", "a,b"]).starts_with("-p, -o and -l don't go together"));
        assert!(err(&["q", "-l", "a,b", "-t", "0.5"]).starts_with("-t doesn't go with -p, -o or -l"));
        for bad in ["a", "a,", "a,,b", ","] {
            assert!(err(&["q", "-o", bad]).starts_with("-o: "), "{bad}");
            assert!(err(&["q", "-l", bad]).starts_with("-l: "), "{bad}");
        }
        assert!(err(&["q", "-l", "0,1,2,3,4,5,6,7,8,9,10"]).contains("2 to 10 levels"));
        assert_eq!(ask(&["q", "-l", "0,1,2,3,4,5,6,7,8,9"]).1, Mode::Rate("0,1,2,3,4,5,6,7,8,9".into()));
        assert!(err(&["run", "mix", "-p"]).starts_with("-p, -o and -l are for a question"));
        assert!(err(&["run", "mix", "-l", "a,b"]).starts_with("-p, -o and -l are for a question"));
        // a switch given a value is named, the value not shown
        assert_eq!(err(&["--stats=sk-secret", "q"]), "--stats takes no value");
        assert!(err(&["tune", "q", "-t", "0.5"]).starts_with("deem tune takes no -t"));
        assert!(err(&[]).starts_with("no question"));
        assert!(err(&["run"]).starts_with("usage: deem run"));
        assert_eq!(err(&["-n", "5", "q"]), "-n is for deem tune: how many lines to label");
        assert_eq!(err(&["tune", "-n", "0", "q"]), "-n: \"0\" is not a count");
        assert_eq!(
            err(&["-t", "0.5", "--arg", "threshold", "0.6", "q"]),
            "threshold is set twice (-t is --arg threshold)"
        );
        assert!(err(&["--arg", "x"]).contains("--arg"));
    }

    // `--api NAME` goes anywhere a run's options go, and before a subcommand
    #[test]
    fn choosing_an_api() {
        let api = |args: &[&str]| with_api(args).map(|(_, a)| a);
        assert_eq!(api(&["--api", "kev", "q", "f"]), Ok(Some("kev".into())));
        assert_eq!(api(&["q", "--api=kev", "f"]), Ok(Some("kev".into())));
        assert_eq!(api(&["--api", "kev", "run", "mix", "q"]), Ok(Some("kev".into())));
        assert_eq!(api(&["-J", "tune", "--api", "kev", "q"]), Ok(Some("kev".into())));
        assert_eq!(with_api(&["--api", "kev", "models"]), Ok((Cmd::Models, Some("kev".into()))));
        assert_eq!(with_api(&["models", "--api", "kev"]), Ok((Cmd::Models, Some("kev".into()))));
        assert_eq!(api(&["q", "f"]), Ok(None));
        // after `--`, a question
        assert_eq!(ask(&["--", "--api"]).0, "--api");
        let err = |args: &[&str]| with_api(args).unwrap_err();
        assert_eq!(err(&["--api", "a", "--api", "b", "q"]), "--api is given twice");
        assert_eq!(err(&["q", "--api", "a", "--api", "b"]), "--api is given twice");
        assert!(err(&["--api", "kev", "api"]).starts_with("--api chooses a run's API"));
        assert!(err(&["--api"]).starts_with("--api: give the name"));
    }

    #[test]
    fn api_commands() {
        use deem::apis::{Api, Change};
        let cmd = |args: &[&str]| match parse(args) {
            Ok(Cmd::Api(c)) => c,
            c => panic!("{c:?}"),
        };
        assert_eq!(cmd(&["api"]), ApiCmd::List);
        let kev = Api {
            name: "kev".into(),
            url: "http://127.0.0.1:8009/v1".into(),
            model: "jev-latest".into(),
            pictures: false,
            rps: None,
            tps: None,
            parallel: None,
        };
        assert_eq!(
            cmd(&["api", "add", "kev", "http://127.0.0.1:8009/v1"]),
            ApiCmd::Add { api: kev.clone(), key: false }
        );
        let jev = cmd(&["api", "add", "jev", "https://api.typesafe.ai/v1", "--pictures", "--key", "--rps", "20"]);
        let want = Api {
            name: "jev".into(),
            url: "https://api.typesafe.ai/v1".into(),
            pictures: true,
            rps: Some(20),
            ..kev.clone()
        };
        assert_eq!(jev, ApiCmd::Add { api: want, key: true });
        let change = Change { model: Some("kev-latest".into()), pictures: Some(false), ..Change::default() };
        assert_eq!(
            cmd(&["api", "edit", "kev", "--model", "kev-latest", "--no-pictures", "--no-key"]),
            ApiCmd::Edit { name: "kev".into(), change, key: Some(false) }
        );
        assert_eq!(cmd(&["api", "remove", "kev"]), ApiCmd::Remove("kev".into()));
        assert!(parse(&["--api", "kev", "api", "remove", "a"]).unwrap_err().starts_with("--api chooses a run's API"));
        assert_eq!(cmd(&["api", "default"]), ApiCmd::Default(None));
        assert_eq!(cmd(&["api", "default", "kev"]), ApiCmd::Default(Some("kev".into())));
        let err = |args: &[&str]| parse(args).unwrap_err();
        assert!(err(&["api", "edit", "kev"]).starts_with("nothing to change"));
        assert_eq!(err(&["api", "edit", "kev", "--key", "--no-key"]), "give --key or --no-key once");
        assert!(err(&["api", "add", "kev", "ftp://x"]).contains("is not an http:// or https:// URL"));
        assert!(err(&["api", "add", "kev", "http://x/v1/systemone"]).contains("names the endpoint"));
        assert!(err(&["api", "add", "k v", "https://x/v1"]).contains("can't name an API"));
        // http:// only to this machine; anywhere else, the key would go in the clear
        for url in ["http://x/v1", "http://localhost.x/v1", "http://[::2]/v1"] {
            assert!(err(&["api", "add", "kev", url]).contains("in the clear: use https://"), "{url}");
        }
        for url in ["http://localhost:8009/v1", "http://127.1.2.3/v1", "http://[::1]:80/v1", "http://LOCALHOST"] {
            assert_eq!(
                cmd(&["api", "add", "kev", url]),
                ApiCmd::Add { api: Api { url: url.into(), ..kev.clone() }, key: false }
            );
        }
        assert_eq!(err(&["api", "edit", "kev", "--rps", "0"]), "--rps: 0 is not from 1 to 1000");
        let change = Change { parallel: Some(4), ..Change::default() };
        assert_eq!(
            cmd(&["api", "edit", "kev", "--parallel", "4"]),
            ApiCmd::Edit { name: "kev".into(), change, key: None }
        );
        assert_eq!(err(&["api", "edit", "kev", "--parallel", "0"]), "--parallel: 0 is not from 1 to 256");
        assert_eq!(err(&["api", "edit", "kev", "--parallel", "257"]), "--parallel: 257 is not from 1 to 256");
        assert!(err(&["api", "remove", "kev", "--pictures"]).starts_with("deem api remove takes no settings"));
        assert!(err(&["api", "compare", "kev"]).starts_with("usage: deem api"));
        assert!(err(&["api", "add", "kev"]).starts_with("usage: deem api"));
        // a name or password in the URL is refused, and not shown
        for url in ["https://me:sk-secret@x/v1", "http://127.0.0.1@x/v1", "https://sk-secret@x"] {
            let e = err(&["api", "add", "kev", url]);
            assert!(e.contains("has a name or password in it") && !e.contains("secret") && !e.contains("@x"), "{e}");
        }
        let url = "https://x/v1/a@b";
        let want = ApiCmd::Add { api: Api { url: url.into(), ..kev.clone() }, key: false };
        assert_eq!(cmd(&["api", "add", "kev", url]), want, "an @ in the path is no password");
        assert!(err(&["api", "add", "kev", "https://x/v1", "--no-key"]).starts_with("--no-key is for deem api edit"));
    }

    #[test]
    fn help_after_any_subcommand() {
        for args in [&["trace", "--help"][..], &["trace", "compare", "--help"], &["trace", "-h", "run.jsonl"]] {
            assert_eq!(parse(args), Ok(Cmd::TraceHelp), "{args:?}");
        }
        for args in [
            &["api", "--help"][..],
            &["api", "add", "-h"],
            &["models", "--help"],
            &["programs", "--help"],
            &["run", "--help"],
            &["tune", "-h"],
        ] {
            assert_eq!(parse(args), Ok(Cmd::Help), "{args:?}");
        }
    }

    // the examples in `deem trace --help` are commands deem takes
    #[test]
    fn the_trace_help_examples_parse() {
        let examples: Vec<&str> = (TRACE_USAGE.lines())
            .filter_map(|l| l.strip_prefix("  deem "))
            .map(|l| l.split("   ").next().unwrap())
            .filter(|l| !l.contains('[') && !l.contains("NAME"))
            .collect();
        assert_eq!(examples.len(), 4, "{examples:?}");
        for e in examples {
            let args: Vec<&str> = e.split_whitespace().collect();
            assert!(matches!(parse(&args), Ok(Cmd::Run { .. } | Cmd::Trace(..) | Cmd::TraceCompare { .. })), "{e}");
        }
        assert!(TRACE_USAGE.lines().all(|l| l.chars().count() <= 80));
    }

    #[test]
    fn trace_compare() {
        let want = |with: &str, files: &[&str]| {
            Ok(Cmd::TraceCompare { with: with.into(), files: files.iter().map(|f| f.to_string()).collect() })
        };
        assert_eq!(
            parse(&["trace", "compare", "--with", "jev", "a.jsonl", "b.jsonl"]),
            want("jev", &["a.jsonl", "b.jsonl"])
        );
        assert_eq!(parse(&["trace", "compare", "a.jsonl", "--with=jev"]), want("jev", &["a.jsonl"]));
        let err = |args: &[&str]| parse(args).unwrap_err();
        assert!(err(&["trace", "compare", "a.jsonl"]).starts_with("usage: deem trace compare"));
        assert!(err(&["trace", "compare", "--with", "jev"]).starts_with("usage: deem trace compare"));
        assert!(err(&["trace", "compare", "a", "--with"]).starts_with("usage: deem trace compare"));
        assert_eq!(err(&["trace", "compare", "--with", "a", "--with", "b", "f"]), "--with is given twice");
        assert!(err(&["trace", "compare", "--with", "a", "--visual", "f"]).starts_with("--visual: not an option"));
    }

    #[test]
    fn named_values() {
        let (_, _, _, o) = ask(&["--arg", "urgent", "0.9", "q", "--arg=billing", "x y", "f"]);
        assert_eq!(o.named, [("urgent".into(), "0.9".into()), ("billing".into(), "x y".into())]);
        let (_, _, files, o) = ask(&["--rawfile", "rubric", "r.txt", "q", "f"]);
        assert_eq!((o.rawfile, files), (vec![("rubric".into(), "r.txt".into())], vec!["f".to_owned()]));
    }
}
