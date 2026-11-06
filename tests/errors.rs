//! "When an answer fails": every example in that section of docs/MANUAL.md, run as
//! written, and every row of its table, against the fake.

mod common;

use common::*;

/// The `console` blocks of a README section, as (commands, expected output).
fn console_blocks(section: &str) -> Vec<(Vec<String>, String)> {
    let readme = std::fs::read_to_string("docs/MANUAL.md").unwrap();
    let start = readme.find(&format!("## {section}\n")).unwrap();
    let body = &readme[start + 3..];
    let body = &body[..body.find("\n## ").unwrap_or(body.len())];
    body.split("```console\n")
        .skip(1)
        .map(|b| {
            let b = &b[..b.find("```").unwrap()];
            let mut cmds: Vec<String> = Vec::new();
            let mut out = String::new();
            let mut lines = b.lines();
            while let Some(l) = lines.next() {
                match l.strip_prefix("$ ") {
                    Some(c) => {
                        // a command continues while a quote is open, or after a `\`
                        let mut c = c.to_owned();
                        while c.matches('\'').count() % 2 == 1 || c.ends_with('\\') {
                            c = format!("{c}\n{}", lines.next().unwrap());
                        }
                        cmds.push(c)
                    }
                    None => out += &format!("{l}\n"),
                }
            }
            (cmds, out)
        })
        .collect()
}

#[test]
fn readme_examples_run_as_written() {
    let dir = std::path::Path::new(env!("CARGO_BIN_EXE_deem")).parent().unwrap().to_owned();
    let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
    let blocks = console_blocks("When an answer fails");
    assert!(blocks.len() >= 5);
    // one shell session for the section, as a reader would type it
    let script: String = blocks.iter().flat_map(|(c, _)| c).map(|c| format!("{c} 2>&1\n")).collect();
    let want: String = blocks.iter().map(|(_, o)| o.as_str()).collect();
    let o = std::process::Command::new("sh")
        .args(["-c", &script])
        .current_dir("examples")
        .env_clear()
        .envs(OFFLINE)
        .env("PATH", path)
        .env("JEV_API_KEY", "sk")
        .output()
        .unwrap();
    let _ = std::fs::remove_file("examples/retry.log");
    assert_eq!(unwrap_lines(&as_on_linux(&String::from_utf8_lossy(&o.stdout))), unwrap_lines(&want));
}

/// The table, row by row: the fake fails "FAIL" questions about "boom".
#[test]
fn the_table() {
    const WHY: &str = "the answer to \"FAIL\" failed: the model answered null, not a number in [0,1]";
    let caught = "deem: 1 failed answer was caught by a try or ? in the program\n";
    let failed = |r: &Run| {
        assert_eq!((r.code, r.stdout.as_str()), (2, ""), "{}", r.stderr);
        assert_eq!(r.stderr, format!("deem: record 1: {WHY}\n"));
    };
    // a failed answer is a jq error where it was asked
    for (program, code, stdout) in [
        (r#"try ask("FAIL"; .) catch 0"#, 0, "0\n".to_owned()),
        (r#"try ask("FAIL"; .) catch ."#, 0, format!("{WHY}\n")),
        (r#"ask("FAIL"; .)?"#, 1, String::new()),
        (r#"try (ask("FAIL"; .) | length) catch 0"#, 0, "0\n".into()),
        (r#"[.xs[] | try ask("FAIL"; .) catch 0]"#, 0, "[0]\n".into()),
    ] {
        let r = match program.contains(".xs") {
            true => deem_json(program, "{\"xs\": [\"boom\"]}\n"),
            false => deem(program, "boom\n"),
        };
        assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (code, stdout.as_str(), caught), "{program}");
    }
    // uncaught, it fails the record; jaq's `//` passes errors on
    for program in [
        r#"ask("FAIL"; .) // 0"#,
        r#"select(ask("FAIL"; .) > 0.5)"#,
        r#"[ask("FAIL"; .), .] | @tsv"#,
        r#"ask("FAIL"; .) as $x | "unused""#,
    ] {
        failed(&deem(program, "boom\n"));
    }
    let r = deem(r#"ask("FAIL"; .) as $x | "unused""#, "boom\n");
    assert_eq!(r.log.len(), 1, "asked, so sent");

    // arithmetic on an answer fails where it is written, inside the `try`
    let r = deem(r#"try (ask("q"; .) + "s") catch "bad""#, "fine\n");
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, "bad\n", ""));

    // a question the API can't take is the program's mistake: a jq error at
    // the call, caught by `try`, and nothing is sent
    let r = deem(r#"try choose("q"; [range(300) | tostring]; .) catch ."#, "x\n");
    assert_eq!((r.code, r.stdout.as_str(), r.log.len()), (0, "judge: 300 options; the API takes at most 255\n", 0));
}

/// Only the record that looks at a failed answer fails; the others print.
#[test]
fn a_failure_is_one_record() {
    let mut recs = records48();
    recs[3] = "record 3: boom".into();
    recs[30] = "record 30: boom".into();
    let r = deem(r#"select(ask("FAIL?"; .) > 0.5) | "kept""#, &lines(&recs));
    let failed: Vec<&str> =
        r.stderr.lines().filter(|l| l.starts_with("deem: record")).map(|l| l.split(':').nth(1).unwrap()).collect();
    assert_eq!(failed, [" record 4", " record 31"]);
    assert!(
        r.stderr.ends_with("\ndeem: 2 of 48 records failed; --failed FILE keeps them to run again\n"),
        "{}",
        r.stderr
    );
    assert!(!r.stdout.is_empty());
}
