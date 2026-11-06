//! Programs by name: the built-in ones, then `NAME.deem` in the nearest
//! `.deem/` from the current directory up, then `~/.deem/NAME.deem`. A word
//! after `deem run` that ends in `.deem`, or contains `/` and names a file,
//! is a path; a known name is a program; anything else is jq text (which
//! may divide: `.a / 2`).

use crate::engine::{self, Sig};

mod forms;
pub use forms::{FILTER, LABEL, Mode, RATE, SCORE, program};
use std::path::{Path, PathBuf};

/// deem's own programs, in the order `deem programs` lists them.
pub const BUILTIN: [(&str, &str); 4] = [
    ("group", include_str!("group.deem")),
    ("histogram", include_str!("histogram.deem")),
    ("mix", include_str!("mix.deem")),
    ("disagree", include_str!("disagree.deem")),
];

/// A program's text, and what to call it in messages.
#[derive(Debug, PartialEq)]
pub struct Source {
    pub name: String,
    pub text: String,
}

impl Source {
    /// Split the words after the program into its arguments, as many as
    /// `main` takes, and the files after them.
    pub fn split(&self, mut rest: Vec<String>) -> Result<(Vec<String>, Vec<String>), String> {
        let sig = Sig::of(&self.text).map_err(|e| format!("{}: {e}", self.name))?;
        let params = sig.params.unwrap_or_default();
        if rest.len() < params.len() {
            let s = if params.len() == 1 { "" } else { "s" };
            let (n, got) = (params.len(), rest.len());
            return Err(format!("{} takes {n} argument{s} ({}), got {got}", self.name, params.join(", ")));
        }
        let files = rest.split_off(params.len());
        Ok((rest, files))
    }
}

/// Where a saved program is looked for, nearest first, as shown to the user:
/// the nearest `.deem/` from `cwd` up (`../../.deem/`), stopping at the home
/// directory, then `~/.deem/`.
fn dirs(cwd: &Path, home: Option<&Path>) -> Vec<(String, PathBuf)> {
    let mine = home.map(|h| h.join(".deem"));
    let mut dirs = Vec::new();
    let mut up = String::new();
    for dir in cwd.ancestors() {
        let deem = dir.join(".deem");
        if mine.as_ref() == Some(&deem) {
            break;
        }
        if deem.is_dir() {
            dirs.push((format!("{up}.deem/"), deem));
            break;
        }
        up.push_str("../");
    }
    dirs.extend(mine.map(|d| ("~/.deem/".to_owned(), d)));
    dirs
}

/// The error for a name that is no program.
pub fn not_found(word: &str, cwd: &Path, home: Option<&Path>) -> String {
    let dirs = dirs(cwd, home);
    let mut at = vec!["built-in"];
    at.extend(dirs.iter().map(|(shown, _)| shown.as_str()));
    let none = if dirs.iter().any(|(shown, _)| shown != "~/.deem/") { "" } else { "; no .deem/ here or above" };
    format!("no program named {word:?} (looked in: {}{none})", at.join(", "))
}

/// Whether a file stem can be a program's name, and why not.
fn check_name(name: &str) -> Result<(), String> {
    let mut cs = name.chars();
    let ok = cs.next().is_some_and(|c| c.is_ascii_lowercase())
        && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !ok {
        return Err(format!("{name:?} can't be a program's name: use lowercase letters, digits, - and _"));
    }
    if BUILTIN.iter().any(|(n, _)| *n == name) {
        return Err(format!("{name} is a built-in program and can't be replaced; rename the file"));
    }
    if engine::is_builtin(name) {
        return Err(format!("{name} is a jq builtin, so `deem run {name}` means jq's; rename the file"));
    }
    Ok(())
}

/// The program `word` names.
pub fn resolve(word: &str, cwd: &Path, home: Option<&Path>) -> Result<Source, String> {
    if word.ends_with(".deem") || (word.contains('/') && cwd.join(word).is_file()) {
        let text = std::fs::read_to_string(cwd.join(word)).map_err(|e| format!("{word}: {e}"))?;
        return Ok(Source { name: word.to_owned(), text });
    }
    if let Some((name, text)) = BUILTIN.iter().find(|(n, _)| *n == word) {
        return Ok(Source { name: name.to_string(), text: text.to_string() });
    }
    let jq = || Source { name: "the program".into(), text: word.to_owned() };
    if !word.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Ok(jq());
    }
    for (shown, dir) in dirs(cwd, home) {
        let path = dir.join(format!("{word}.deem"));
        if path.is_file() {
            let at = format!("{shown}{word}.deem");
            check_name(word).map_err(|e| format!("{at}: {e}"))?;
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{at}: {e}"))?;
            return Ok(Source { name: word.to_owned(), text });
        }
    }
    match engine::Program::compile(word, &[], &[]) {
        Ok(_) => Ok(jq()),
        Err(_) => Err(not_found(word, cwd, home)),
    }
}

/// `deem programs`: saved programs nearest first, then the built-ins. Each
/// is two lines, to stay narrow: its arguments, named values and where it
/// is; then, indented, what it does.
pub fn list(cwd: &Path, home: Option<&Path>) -> String {
    let mut rows = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    for (shown, dir) in dirs(cwd, home) {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            let (Some(stem), Some("deem")) =
                (path.file_stem().and_then(|s| s.to_str()), path.extension().and_then(|e| e.to_str()))
            else {
                continue;
            };
            let at = format!("{shown}{stem}.deem");
            let about = match (check_name(stem), seen.iter().find(|(n, _)| n == stem)) {
                (Err(e), _) => format!("({e})"),
                (Ok(()), Some((_, by))) => format!("(hidden by {by})"),
                (Ok(()), None) => {
                    seen.push((stem.to_owned(), at.clone()));
                    std::fs::read_to_string(&path).map_or_else(|e| format!("({e})"), |t| describe(stem, &t).1)
                }
            };
            let usage = std::fs::read_to_string(&path).map_or(stem.to_owned(), |t| describe(stem, &t).0);
            rows.push((usage, at, about));
        }
    }
    for (name, text) in BUILTIN {
        let (usage, about) = describe(name, text);
        rows.push((usage, "built-in".into(), about));
    }
    rows.iter().map(|(u, at, about)| format!("{u:<32} {at}\n    {about}\n")).collect()
}

/// `NAME ARGS [defaults]`, and the first comment line less `# NAME:`.
fn describe(name: &str, text: &str) -> (String, String) {
    let about = text.lines().map(str::trim).find_map(|l| l.strip_prefix('#')).unwrap_or("").trim();
    let about = about.strip_prefix(&format!("{name}:")).unwrap_or(about).trim().to_owned();
    match Sig::of(text) {
        Err(e) => (name.to_owned(), format!("(error: {})", e.lines().next().unwrap_or(""))),
        Ok(sig) => {
            let mut usage = vec![name.to_owned()];
            usage.extend(sig.params.iter().flatten().map(|p| p.to_uppercase()));
            usage.extend(sig.defaults.iter().map(|(n, v)| format!("[{n}={v}]")));
            (usage.join(" "), about)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_programs() {
        for (name, text) in BUILTIN {
            let sig = Sig::of(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(sig.input, engine::Input::All, "{name}");
            let args = vec!["x".to_owned(); sig.params.as_ref().unwrap().len()];
            engine::Program::compile(text, &args, &[]).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(text.lines().nth(1).unwrap().starts_with(&format!("# {name}: ")), "{name}");
        }
    }

    /// A scratch directory with a project and a home.
    fn places(files: &[(&str, &str)]) -> (PathBuf, PathBuf) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("deem-programs-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let (cwd, home) = (root.join("proj"), root.join("home"));
        std::fs::create_dir_all(&cwd).unwrap();
        (cwd, home)
    }

    #[test]
    fn lookup() {
        let (cwd, home) = places(&[
            ("proj/.deem/triage.deem", "# triage: near\n."),
            ("home/.deem/triage.deem", "# triage: far\n."),
            ("home/.deem/mine.deem", "module {defaults: {threshold: 0.7}};\n# mine: mine\n."),
            ("proj/.deem/mix.deem", "."),
            ("proj/.deem/length.deem", "."),
            ("proj/x/prog.deem", "1"),
        ]);
        let find = |w: &str| resolve(w, &cwd, Some(&home));
        assert_eq!(find("triage").unwrap().text, "# triage: near\n.", ".deem/ first");
        assert_eq!(find("mine").unwrap().name, "mine");
        assert_eq!(find("mix").unwrap().text, BUILTIN[2].1, "built-ins can't be replaced");
        assert!(find("length").unwrap_err().starts_with(".deem/length.deem: length is a jq builtin"));
        assert_eq!(find("x/prog.deem").unwrap(), Source { name: "x/prog.deem".into(), text: "1".into() });
        assert_eq!(find("./x/prog.deem").unwrap().text, "1");
        assert!(find("gone.deem").unwrap_err().starts_with("gone.deem: "));
        assert_eq!(find("x/prog").unwrap().name, "the program", "no such file: jq text");
        assert_eq!(find(".a / 2").unwrap().text, ".a / 2");
        assert_eq!(find("select(. > 1)").unwrap().name, "the program");
        assert_eq!(find("empty").unwrap().text, "empty", "a jq builtin is jq");
        assert_eq!(find("triag").unwrap_err(), r#"no program named "triag" (looked in: built-in, .deem/, ~/.deem/)"#);
        assert!(resolve("mine", &cwd, None).is_err(), "no HOME, no ~/.deem/");

        let list = list(&cwd, Some(&home));
        let lines: Vec<&str> = list.lines().collect();
        let rows: Vec<Vec<&str>> = lines
            .chunks(2)
            .map(|l| {
                assert!(l[1].starts_with("    ") && l[0].len() <= 60, "{l:?}");
                l[0].split("  ").map(str::trim).filter(|s| !s.is_empty()).chain([l[1].trim()]).collect()
            })
            .collect();
        assert_eq!(
            rows[0],
            [
                "length",
                ".deem/length.deem",
                "(length is a jq builtin, so `deem run length` means jq's; rename the file)"
            ]
        );
        assert_eq!(rows[1][2], "(mix is a built-in program and can't be replaced; rename the file)");
        assert_eq!(rows[2], ["triage", ".deem/triage.deem", "near"]);
        assert_eq!(rows[3], ["mine [threshold=0.7]", "~/.deem/mine.deem", "mine"]);
        assert_eq!(rows[4], ["triage", "~/.deem/triage.deem", "(hidden by .deem/triage.deem)"]);
        // a usage too long for its column is set off by one space
        assert_eq!(rows[5][0], "group Q [threshold=0.5] [groups=20] built-in");
        assert_eq!(rows.len(), 9);
        let _ = std::fs::remove_dir_all(cwd.parent().unwrap());
    }

    #[test]
    fn nearest() {
        let (cwd, home) = places(&[
            ("proj/.deem/triage.deem", "# triage: near\n."),
            ("proj/api/handlers/x", ""),
            ("home/.deem/triage.deem", "# triage: far\n."),
            ("home/work/x", ""),
        ]);
        let deep = cwd.join("api/handlers");
        assert_eq!(
            resolve("triage", &deep, Some(&home)).unwrap().text,
            "# triage: near\n.",
            "the project's, from below"
        );
        let shown = list(&deep, Some(&home));
        assert!(shown.starts_with("triage                           ../../.deem/triage.deem\n    near\n"), "{shown}");
        assert!(shown.contains("~/.deem/triage.deem\n    (hidden by ../../.deem/triage.deem)\n"), "{shown}");
        assert_eq!(
            resolve("triag", &deep, Some(&home)).unwrap_err(),
            r#"no program named "triag" (looked in: built-in, ../../.deem/, ~/.deem/)"#
        );

        let work = home.join("work");
        assert_eq!(
            resolve("triage", &work, Some(&home)).unwrap().text,
            "# triage: far\n.",
            "~/.deem/ is not a project's"
        );
        assert_eq!(list(&work, Some(&home)).lines().filter(|l| l.starts_with("triage")).count(), 1, "listed once");
        assert_eq!(
            list(&home, Some(&home)).lines().filter(|l| l.starts_with("triage")).count(),
            1,
            "listed once, from home"
        );
        assert_eq!(
            resolve("triag", &work, Some(&home)).unwrap_err(),
            r#"no program named "triag" (looked in: built-in, ~/.deem/; no .deem/ here or above)"#
        );
        let _ = std::fs::remove_dir_all(cwd.parent().unwrap());
    }

    #[test]
    fn names() {
        assert!(check_name("triage").is_ok() && check_name("a-b_2").is_ok());
        for bad in ["Triage", "2x", "a.b", "", "length", "ask", "if", "group", "tojson"] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
    }
}
