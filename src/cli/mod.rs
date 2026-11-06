//! The command line: parse the arguments, then call the library.

mod apis;
mod args;
mod tune;

pub use args::{Cmd, TRACE_USAGE, USAGE, View, parse};
use deem::programs::{self, program};
use deem::{Config, RunOptions, api, trace};
use std::path::PathBuf;
use std::sync::Arc;

/// Returns the exit code.
pub fn main() -> i32 {
    let fail = |e: String| {
        eprintln!("deem: {e}");
        2
    };
    // on Windows, where the shell leaves `*.png` as it is, deem expands it
    // (outside double quotes); elsewhere these are the shell's arguments
    let (cmd, chosen) = match parse(wild::args_os().skip(1).collect()) {
        Ok(parsed) => parsed,
        Err(e) => return fail(e),
    };
    let cfg = || Config::load(|k| std::env::var(k).ok(), chosen.as_deref());
    match cmd {
        Cmd::Help => print!("{USAGE}"),
        Cmd::TraceHelp => print!("{TRACE_USAGE}"),
        Cmd::Version => println!("deem {}", env!("CARGO_PKG_VERSION")),
        Cmd::Trace(file, view) => {
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{file}: {e}"));
            let draw = match view {
                View::Text => trace::report,
                View::Visual => trace::visual,
            };
            match text.and_then(|t| draw(&t)) {
                Ok(r) => print!("{r}"),
                Err(e) => return fail(e),
            }
        }
        Cmd::Models => {
            let cfg = match cfg().and_then(checked) {
                Ok(c) => c,
                Err(e) => return fail(e),
            };
            match api::Http::new(&cfg.base_url, &cfg.key, cfg.timeout).models() {
                Ok(body) => println!("{}", body.trim_end()),
                Err(e) => return fail(e.describe(&cfg.settings())),
            }
        }
        Cmd::TraceCompare { with, files } => {
            let read: Result<Vec<(String, String)>, String> = (files.into_iter())
                .map(|f| std::fs::read_to_string(&f).map(|t| (f.clone(), t)).map_err(|e| format!("{f}: {e}")))
                .collect();
            match read.and_then(|r| trace::compare(&r, &with)) {
                Ok(r) => print!("{r}"),
                Err(e) => return fail(e),
            }
        }
        Cmd::Api(cmd) => match apis::api(cmd, apis::dirs()) {
            Ok(out) => print!("{out}"),
            Err(e) => return fail(e),
        },
        Cmd::Programs(name) => {
            let (cwd, home) = places();
            match name {
                None => print!("{}", programs::list(&cwd, home.as_deref())),
                Some(n) => match programs::resolve(&n, &cwd, home.as_deref()) {
                    Ok(p) if p.name != "the program" => print!("{}", p.text),
                    Ok(_) => return fail(programs::not_found(&n, &cwd, home.as_deref())),
                    Err(e) => return fail(e),
                },
            }
        }
        Cmd::Run { word, rest, opts } => {
            let (cwd, home) = places();
            let found = programs::resolve(&word, &cwd, home.as_deref());
            let (p, args, files) = match found.and_then(|p| Ok((p.split(rest)?, p))) {
                Ok(((args, files), p)) => (p, args, files),
                Err(e) => return fail(e),
            };
            return go((&p.name, &p.text, &args), &files, &opts, cfg);
        }
        Cmd::Tune { question, count, files, opts } => {
            let (tty_in, tty_out) = match open_tty() {
                Ok((i, o)) => (std::io::BufReader::new(i), o),
                Err(_) => return fail(tune::NO_TERMINAL.into()),
            };
            let (input, cfg) = match setup(&files, &opts, cfg) {
                Ok((Input::Text(input), cfg)) => (input, cfg),
                Ok(_) => return fail("deem tune labels text in the terminal; it can't show pictures".into()),
                Err(e) => return fail(e),
            };
            let judge = Arc::new(api::Http::new(&cfg.base_url, &cfg.key, cfg.timeout));
            let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
            return tune::tune(
                (&question, count),
                &files,
                &opts,
                &cfg,
                judge,
                input,
                (tty_in, tty_out),
                &mut out,
                &mut err,
            );
        }
        Cmd::Ask { question, mode, files, opts } => {
            let (program, args) = program(&question, &mode);
            return go(("the program", program, &args), &files, &opts, cfg);
        }
    }
    0
}

use deem::Input;

/// The configuration, with a key if the API needs one, and the input; a
/// run that shows pictures only to an API that takes them.
fn setup(
    files: &[String],
    opts: &RunOptions,
    cfg: impl FnOnce() -> Result<Config, String>,
) -> Result<(Input, Config), String> {
    let cfg = checked(cfg()?)?;
    written_is_read(files, opts)?;
    let input = deem::open_input(files, opts.paths_in)?;
    let shows = !opts.img.is_empty() || matches!(input, Input::Pictures(_) | Input::Paths(_));
    if let (true, false, Some(name)) = (shows, cfg.pictures, &cfg.api) {
        return Err(format!(
            "API {name} doesn't take pictures: choose one that does (--api NAME), or `deem api edit {name} --pictures`"
        ));
    }
    Ok((input, cfg))
}

/// The configuration, if it has a key where the API needs one; an API
/// other than the default is named, so a run never goes elsewhere unseen.
fn checked(cfg: Config) -> Result<Config, String> {
    if cfg.missing_key() {
        return Err(cfg.no_key());
    }
    if let (true, Some(name)) = (cfg.not_default, &cfg.api) {
        eprintln!("deem: using {name} ({})", cfg.base_url);
    }
    Ok(cfg)
}

/// A file deem writes (`--failed`, `--trace`) is emptied when the run
/// starts, so it can't also be an input: rerunning `--failed retry.log` on
/// `retry.log` would lose the records it was meant to run again.
fn written_is_read(files: &[String], opts: &RunOptions) -> Result<(), String> {
    let same =
        |a: &str, b: &str| matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b);
    for (flag, path) in [("--failed", &opts.failed), ("--trace", &opts.trace)] {
        if let Some(path) = path
            && files.iter().any(|f| same(f, path))
        {
            return Err(format!(
                "{flag} {path} is also an input, and deem would empty it before reading it; write to another file"
            ));
        }
    }
    Ok(())
}

/// Run a program on the files, or stdin.
fn go(
    program: (&str, &str, &[String]),
    files: &[String],
    opts: &RunOptions,
    cfg: impl FnOnce() -> Result<Config, String>,
) -> i32 {
    let (input, cfg) = match setup(files, opts, cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("deem: {e}");
            return 2;
        }
    };
    let judge = Arc::new(api::Http::new(&cfg.base_url, &cfg.key, cfg.timeout));
    let out = std::io::stdout().lock();
    deem::run(program, opts, &cfg, judge, input, out, &mut std::io::stderr()).code
}

/// The current directory, and the home directory if one is set (`HOME`, else
/// Windows' `USERPROFILE`): where saved programs are looked for from.
fn places() -> (PathBuf, Option<PathBuf>) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let var = |v| std::env::var_os(v).filter(|h| !h.is_empty());
    (cwd, var("HOME").or_else(|| var("USERPROFILE")).map(Into::into))
}

/// The terminal, to read from and write to, whatever stdin and stdout are.
#[cfg(unix)]
fn open_tty() -> std::io::Result<(std::fs::File, std::fs::File)> {
    let t = std::fs::File::options().read(true).write(true).open("/dev/tty")?;
    Ok((t.try_clone()?, t))
}

/// The console, to read from and write to, whatever stdin and stdout are.
#[cfg(windows)]
fn open_tty() -> std::io::Result<(std::fs::File, std::fs::File)> {
    let i = std::fs::File::open("CONIN$")?;
    let o = std::fs::File::options().write(true).open("CONOUT$")?;
    Ok((i, o))
}
