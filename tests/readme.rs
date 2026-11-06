//! Every `deem` command in the README's main sections, and in the manual's,
//! runs against the fake model behind HTTP, from `examples/`; so does every
//! command in the deem-programs skill, on stand-in files, and its program
//! files. Examples
//! that show an error show exactly deem's error; a command whose output is
//! shown prints `deem: …` on stderr only where the example shows such lines. Every `deem run '…'`
//! program there is also in the jq differential corpus
//! (`tests/golden/cases.tsv`), so its output is checked against jq.

mod common;

use common::*;

/// (file, sections) whose commands run.
const SECTIONS: [(&str, &[&str]); 3] = [
    ("README.md", &["", "Examples", "Grouping", "Getting a question right", "Pictures", "Programs"]),
    (
        "docs/MANUAL.md",
        &[
            "Programs are jq",
            "Saving a program",
            "Programs that come with deem",
            "Gotchas",
            "Pictures",
            "Safety",
            "Calibrated answers",
            "Before a big run",
            "Differences from jq",
        ],
    ),
    ("examples/README.md", &["Programs"]),
];

/// What can't run here: `sample.log` is made by a shell command next to it
/// (`head -200 auth.log > sample.log`), and `deem tune` needs a terminal.
/// Every other file the docs name is in `examples`.
const ELSEWHERE: [&str; 2] = ["sample.log", "deem tune"];

/// The section with this heading, or with "", the text before the first.
fn section(readme: &str, name: &str) -> String {
    if name.is_empty() {
        return readme[..readme.find("\n## ").unwrap_or(readme.len())].to_owned();
    }
    let start = readme.find(&format!("\n## {name}")).unwrap_or_else(|| panic!("no section {name}")) + 1;
    let body = &readme[start + 3..];
    body[..body.find("\n## ").unwrap_or(body.len())].to_owned()
}

/// (command, expected output if the README shows one) for each `deem`
/// command in the section's `bash` and `console` blocks, block by block.
fn blocks(text: &str) -> Vec<Vec<(String, Option<String>)>> {
    let mut out = Vec::new();
    for block in text.split("```").skip(1).step_by(2) {
        let (kind, body) = block.split_once('\n').unwrap();
        let mut cmds = Vec::new();
        let mut lines = body.lines().peekable();
        while let Some(l) = lines.next() {
            let cmd = match kind {
                "console" => l.strip_prefix("$ "),
                "bash" | "sh" if l.starts_with("deem ") || l.starts_with("sh trace.sh ") => Some(l),
                _ => None,
            };
            let Some(cmd) = cmd else { continue };
            // a command continues while a quote is open, or after a `\`
            let mut cmd = cmd.to_owned();
            while cmd.matches('\'').count() % 2 == 1 || cmd.ends_with('\\') {
                cmd = format!("{cmd}\n{}", lines.next().unwrap());
            }
            // a `console` block shows output; a `bash` or `sh` block is commands
            let mut shown = String::new();
            while let Some(l) = lines.next_if(|l| kind == "console" && !l.starts_with("$ ")) {
                shown += &format!("{l}\n");
            }
            if cmd.contains("deem ") || cmd.starts_with("sh trace.sh ") {
                cmds.push((cmd, (kind == "console").then_some(shown)));
            }
        }
        out.push(cmds);
    }
    out
}

/// Every command in the section, in order.
fn commands(text: &str) -> Vec<(String, Option<String>)> {
    blocks(text).into_iter().flatten().collect()
}

/// Run a doc command against the fake API at `url`, in `examples/`, and
/// check it does what the doc shows.
fn check(cmd: &str, shown: Option<String>, url: &str, path: &str) {
    check_in("examples", cmd, shown, url, path)
}

fn check_in(dir: &str, cmd: &str, shown: Option<String>, url: &str, path: &str) {
    let o = std::process::Command::new("sh")
        .args(["-c", cmd])
        .current_dir(dir)
        .env_clear()
        .envs(OFFLINE)
        .env("PATH", path)
        .env("JEV_API_KEY", "sk")
        .env("JEV_BASE_URL", url)
        .output()
        .unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let code = o.status.code().unwrap();
    // a `sh` block shows no output, so it may have notes
    let notes = shown.as_ref().is_none_or(|s| s.lines().any(|l| l.starts_with("deem: ")));
    let shown = shown.unwrap_or_default();
    if notes
        && !shown.is_empty()
        && shown.lines().all(|l| l.is_empty() || l.starts_with("deem: ") || l.starts_with(' '))
    {
        // an error, shown as deem prints it
        assert_eq!(code, 2, "{cmd}");
        assert_eq!(unwrap_lines(stderr.trim()), unwrap_lines(shown.trim()), "{cmd}");
    } else {
        assert!(code == 0 || code == 1, "{cmd}: exit {code}\n{stderr}");
        if notes {
            // and `--stats` draws its histogram's rows
            let note = |l: &str| l.starts_with("deem: ") || cmd.contains("--stats") && l.starts_with("     ");
            assert!(stderr.lines().all(note), "{cmd}\n{stderr}");
        } else {
            assert_eq!(stderr, "", "{cmd}");
        }
        assert!(code == 1 || !stdout.is_empty(), "{cmd}");
    }
}

// `examples/trace.sh` runs every example program but `locate`, whose box
// isn't compared: a program added to `.deem/` and not to the script fails
// here, and so does one the script names that isn't there.
#[test]
fn trace_sh_runs_every_example() {
    let script = std::fs::read_to_string("examples/trace.sh").unwrap();
    let listed: std::collections::BTreeSet<&str> =
        script.lines().filter_map(|l| l.strip_prefix("run ")).filter_map(|l| l.split_whitespace().next()).collect();
    let programs: std::collections::BTreeSet<String> = std::fs::read_dir("examples/.deem")
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter_map(|f| f.strip_suffix(".deem").map(str::to_owned))
        .filter(|p| p != "locate")
        .collect();
    assert_eq!(listed, programs.iter().map(String::as_str).collect(), "{script}");
}

// Each command paces itself at the API's rate, so the blocks run side by
// side, 8 at a time; a block's commands run in order, in case one needs
// what another wrote.
#[test]
fn every_example_runs() {
    let s = fake_api();
    let dir = std::path::Path::new(env!("CARGO_BIN_EXE_deem")).parent().unwrap().to_owned();
    let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
    let mut all = Vec::new();
    for (file, name) in SECTIONS.iter().flat_map(|(f, names)| names.iter().map(move |n| (f, n))) {
        let readme = std::fs::read_to_string(file).unwrap();
        for block in blocks(&section(&readme, name)) {
            let block: Vec<_> =
                block.into_iter().filter(|(cmd, _)| !ELSEWHERE.iter().any(|f| cmd.contains(f))).collect();
            if !block.is_empty() {
                all.push(block);
            }
        }
    }
    let ran: usize = all.iter().map(Vec::len).sum();
    let queue = std::sync::Mutex::new(all.into_iter());
    let next = || queue.lock().unwrap().next();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                while let Some(block) = next() {
                    for (cmd, shown) in block {
                        check(&cmd, shown, &s.url, &path);
                    }
                }
            });
        }
    });
    assert!(ran >= 30, "{ran} commands");
}

// The manual's APIs section changes a config file, so its commands run in
// order, in a home folder of their own, each given a key on stdin for the
// ones that read one; before each run among them, every API added since
// the last is pointed at the fake.
#[test]
fn the_api_examples_run() {
    let s = fake_api();
    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_deem")).parent().unwrap().to_owned();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let home = scratch("readme-apis");
    // a copy of examples/: `sh trace.sh` writes traces/ there, and a user's
    // own traces in examples/traces are left alone
    let examples = home.join("examples");
    copy("examples".as_ref(), &examples);
    let run = |cmd: &str| {
        let o = std::process::Command::new("sh")
            .args(["-c", &format!("printf 'sk-doc\\n' | {cmd}")])
            .current_dir(&examples)
            .env_clear()
            .envs(OFFLINE)
            .env("PATH", &path)
            .env("HOME", &home)
            .output()
            .unwrap();
        (
            o.status.code().unwrap(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    let manual = std::fs::read_to_string("docs/MANUAL.md").unwrap();
    let cmds = commands(&section(&manual, "APIs"));
    // the README's Install adds the same APIs, so its commands run here too
    let readme = std::fs::read_to_string("README.md").unwrap();
    let install = commands(&section(&readme, "Install"));
    assert!(!install.is_empty());
    for (cmd, _) in &install {
        assert!(cmds.iter().any(|(c, _)| c == cmd), "the README's {cmd:?} isn't in the manual's APIs");
    }
    let (mut added, mut pointed) = (Vec::new(), 0);
    for (cmd, shown) in &cmds {
        if let Some(rest) = cmd.strip_prefix("deem api add ") {
            // one with a key, moved to another server, is given one there
            let name = rest.split_whitespace().next().unwrap();
            added.push(if cmd.contains("--key") { format!("{name} --key") } else { name.to_owned() });
        }
        if !cmd.starts_with("deem api") {
            for edit in added.drain(..) {
                assert_eq!(run(&format!("deem api edit {edit} --url {}", s.url)).0, 0, "{edit}");
                pointed += 1;
            }
        }
        let (code, stdout, stderr) = run(cmd);
        assert!(code == 0 || code == 1, "{cmd}: exit {code}\n{stderr}");
        match shown {
            Some(shown) => assert_eq!((stdout.as_str(), stderr.as_str()), (shown.as_str(), ""), "{cmd}"),
            None => assert!(stderr.lines().all(|l| l.starts_with("deem: ")), "{cmd}\n{stderr}"),
        }
    }
    assert!(pointed >= 3 && cmds.len() >= 5, "{cmds:#?}");
    // what `sh trace.sh` wrote: a trace of each example, for each API
    let traces = std::fs::read_dir(examples.join("traces")).expect("the manual's trace.sh writes traces").count();
    assert!(traces >= 20, "{traces} traces");
    let _ = std::fs::remove_dir_all(home);
}

// The manual's "How it works" records a run with `--trace` and reads it
// back, so its commands run in order, in a copy of examples/; each `deem
// trace` shows the counts the manual does (records, questions, requests,
// waves), the time aside, which is the live run's.
#[test]
fn the_trace_examples_run() {
    let s = fake_api();
    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_deem")).parent().unwrap().to_owned();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let examples = scratch("readme-trace");
    copy("examples".as_ref(), &examples);
    let manual = std::fs::read_to_string("docs/MANUAL.md").unwrap();
    let cmds = commands(&section(&manual, "How it works"));
    let counts = |s: &str| s.lines().next().and_then(|l| l.rsplit_once(" · ")).map(|(c, _)| c.to_owned());
    let mut compared = 0;
    for (cmd, shown) in &cmds {
        let o = std::process::Command::new("sh")
            .args(["-c", cmd])
            .current_dir(&examples)
            .env_clear()
            .envs(OFFLINE)
            .env("PATH", &path)
            .env("JEV_API_KEY", "sk")
            .env("JEV_BASE_URL", &s.url)
            .output()
            .unwrap();
        let (stdout, stderr) = (String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        assert!(o.status.code() == Some(0), "{cmd}\n{stderr}");
        assert_eq!(stderr, "", "{cmd}");
        if cmd.starts_with("deem trace") {
            let shown = shown.as_deref().unwrap_or_default();
            assert!(counts(shown).is_some(), "{cmd}: the manual shows no counts");
            assert_eq!(counts(&stdout), counts(shown), "{cmd}");
            compared += 1;
        }
    }
    assert!(compared >= 2 && cmds.len() >= 5, "{cmds:#?}");
    let _ = std::fs::remove_dir_all(examples);
}

/// `from`, copied to `to`, without the traces a user may have there.
fn copy(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().map(Result::unwrap) {
        let (path, name) = (e.path(), e.file_name());
        match e.file_type().unwrap().is_dir() {
            true if name == "traces" => {}
            true => copy(&path, &to.join(name)),
            false => drop(std::fs::copy(&path, to.join(name)).unwrap()),
        }
    }
}

const SKILL: &str = ".claude/skills/deem-programs/SKILL.md";

/// The `deem` commands in the deem-programs skill's text and tables: an
/// agent copies them into what it hands a user. A span with a placeholder
/// is a template, not a command.
fn skill_commands(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    // the text outside fenced blocks
    let text: String = text.split("```").step_by(2).collect();
    // ``…`` spans first, as they may hold single backticks
    let (doubles, rest): (Vec<&str>, String) = {
        let mut doubles = Vec::new();
        let mut rest = String::new();
        let mut parts = text.split("``");
        rest += parts.next().unwrap_or("");
        while let (Some(code), Some(after)) = (parts.next(), parts.next()) {
            doubles.push(code.trim());
            rest += after;
        }
        (doubles, rest)
    };
    let singles = rest.split('`').skip(1).step_by(2);
    for span in doubles.into_iter().chain(singles) {
        let template = ["…", "...", "NAME", "PROGRAM", "deem tune"].iter().any(|p| span.contains(p));
        if (span.starts_with("deem ") || span.contains("| deem ")) && !template {
            out.push(span.replace("\\|", "|"));
        }
    }
    out
}

// The skill's commands run against the fake, on stand-in files named as
// they are, and its two program files compile and run: an agent writing
// programs for users learns from them.
#[test]
fn the_skill_examples_run() {
    let s = fake_api();
    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_deem")).parent().unwrap().to_owned();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let dir = scratch("skill");
    let lines = "the checkout page keeps spinning\nlove the new dark mode\ncharged twice for one order\n";
    for (name, text) in [("file", lines), ("sample", lines), ("big.log", lines)] {
        std::fs::write(dir.join(name), text).unwrap();
    }
    std::fs::write(dir.join("tickets.jsonl"), "{\"body\": \"I was charged twice\"}\n").unwrap();
    for d in ["shots", "cam"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    for (name, bytes) in [("shots/a.png", png(800, 600)), ("ok.png", png(800, 600)), ("cam/x.jpg", jpeg(800, 600))] {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    let skill = std::fs::read_to_string(SKILL).unwrap();
    let cmds = skill_commands(&skill);
    // its jq blocks: a program file with `main($q)`, and a decision tree
    let jq: Vec<&str> = skill.split("```jq\n").skip(1).map(|b| b.split("```").next().unwrap()).collect();
    assert_eq!(jq.len(), 2, "the skill's program files");
    std::fs::write(dir.join("one.deem"), jq[0]).unwrap();
    std::fs::write(dir.join("tree.deem"), jq[1]).unwrap();
    let files = ["deem run ./one.deem \"is this a problem?\" file", "deem run ./tree.deem file"];
    let d = dir.to_str().unwrap();
    // `deem trace run.jsonl` reads the trace of a run made with `--trace`
    check_in(d, "deem --trace run.jsonl \"is this a problem?\" file", None, &s.url, &path);
    for cmd in cmds.iter().map(String::as_str).chain(files) {
        check_in(d, cmd, None, &s.url, &path);
    }
    assert!(cmds.len() >= 20, "{} commands: {cmds:#?}", cmds.len());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn programs_are_checked_against_jq() {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let cases = norm(&std::fs::read_to_string("tests/golden/cases.tsv").unwrap());
    for (file, name) in [("README.md", "Programs"), ("docs/MANUAL.md", "Programs are jq")] {
        let readme = std::fs::read_to_string(file).unwrap();
        for (cmd, _) in commands(&section(&readme, name)) {
            let program = match cmd.split_once("deem run '") {
                Some((_, rest)) => rest.split('\'').next().unwrap().to_owned(),
                None => continue, // a question form, or a program by name or path
            };
            assert!(cases.contains(&norm(&program)), "add to tests/golden/cases.tsv: {program}");
        }
    }
}

/// Code blocks in the user's docs fit 80 columns, so none scrolls sideways.
#[test]
fn code_blocks_fit_80_columns() {
    let mut long = Vec::new();
    for file in ["README.md", "docs/MANUAL.md", "examples/README.md"] {
        let text = std::fs::read_to_string(file).unwrap();
        let mut code = false;
        for (i, l) in text.lines().enumerate() {
            if l.starts_with("```") {
                code = !code;
            } else if code && l.chars().count() > 80 {
                long.push(format!("{file}:{}: {l}", i + 1));
            }
        }
    }
    assert!(long.is_empty(), "lines over 80 columns:\n{}", long.join("\n"));
}

/// The manual's reference shows `deem --help` exactly, so the two can't
/// drift apart.
#[test]
fn the_manual_shows_the_help() {
    let manual = std::fs::read_to_string("docs/MANUAL.md").unwrap();
    let open = "## Reference\n\n```\n";
    let start = manual.find(open).expect("a reference with a code block") + open.len();
    let block = &manual[start..start + manual[start..].find("```\n").unwrap()];
    assert_eq!(block, bin(&["--help"], "", &[], ".").stdout, "docs/MANUAL.md, Reference: paste `deem --help`");
    let open = "`deem trace --help` prints the trace commands, with examples:\n\n```\n";
    let start = manual.find(open).expect("the trace help in the reference") + open.len();
    let block = &manual[start..start + manual[start..].find("```\n").unwrap()];
    let help = bin(&["trace", "--help"], "", &[], ".").stdout;
    assert_eq!(block, help, "docs/MANUAL.md, Reference: paste `deem trace --help`");
}
