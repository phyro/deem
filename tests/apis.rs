//! `deem api`: the APIs in the config file, their keys, and how a run
//! chooses one (docs/MANUAL.md, "APIs"). Each test has a home folder of its
//! own, so deem's config folder is `HOME/.config/deem` and its state folder,
//! where the keys are, `HOME/.local/state/deem`.

mod common;

use common::*;
use serde_json::Value as Json;
use std::path::{Path, PathBuf};

fn home(name: &str) -> PathBuf {
    scratch(&format!("apis-{name}"))
}

fn deem(home: &Path, args: &[&str], stdin: &str) -> Run {
    deem_with(home, args, stdin, &[])
}

fn deem_with(home: &Path, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Run {
    let h = home.to_str().unwrap();
    let env: Vec<(&str, &str)> = [("HOME", h)].into_iter().chain(env.iter().copied()).collect();
    bin(args, stdin, &env, h)
}

fn ok(r: Run) -> Run {
    assert_eq!(r.code, 0, "{}", r.stderr);
    r
}

fn config(home: &Path) -> PathBuf {
    home.join(".config/deem")
}

fn state(home: &Path) -> PathBuf {
    home.join(".local/state/deem")
}

/// What `deem api` says when it saves a key.
fn saved(home: &Path, name: &str) -> String {
    let only = if cfg!(unix) { ", readable only by you" } else { "" };
    format!("key for {name} saved in {}{only}\n", state(home).join("keys").join(name).display())
}

/// Only its owner can change it, as deem requires of its config.
fn private(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    let _ = (path, mode);
}

fn auth(s: &Stub) -> Vec<Option<String>> {
    s.auth.lock().unwrap().clone()
}

#[test]
fn the_commands_keep_the_file() {
    let h = home("commands");
    let r = ok(deem(&h, &["api"], ""));
    assert!(r.stdout.starts_with("no APIs: deem uses JEV_BASE_URL"), "{}", r.stdout);
    let r = ok(deem(&h, &["api", "add", "jev", "https://api.typesafe.ai/v1", "--pictures", "--key"], "sk-jev\n"));
    assert_eq!(r.stdout, saved(&h, "jev"), "it says where the key is");
    let kev =
        ["api", "add", "kev", "http://127.0.0.1:8009/v1", "--model", "kev-latest", "--rps", "20", "--parallel", "4"];
    ok(deem(&h, &kev, ""));
    let list = "* jev  https://api.typesafe.ai/v1  jev-latest  pictures  key\n  \
                  kev  http://127.0.0.1:8009/v1    kev-latest  rps 20  parallel 4\n";
    assert_eq!(ok(deem(&h, &["api"], "")).stdout, list);
    // the first added is the default, and the default stays until another is chosen
    assert_eq!(ok(deem(&h, &["api", "default"], "")).stdout, "jev\n");
    let r = deem(&h, &["api", "remove", "jev"], "");
    assert_eq!(
        (r.code, r.stderr.as_str()),
        (2, "deem: jev is the default API; make another the default first: `deem api default NAME`\n")
    );
    ok(deem(&h, &["api", "default", "kev"], ""));
    assert_eq!(ok(deem(&h, &["api", "default"], "")).stdout, "kev\n");
    let r = deem(&h, &["api", "add", "kev", "https://x/v1"], "");
    assert_eq!(r.stderr, "deem: there is already an API named kev; change it with `deem api edit`\n");
    let r = deem(&h, &["api", "default", "nope"], "");
    assert_eq!(r.stderr, "deem: there is no API named nope; `deem api` lists them\n");

    // the key is in a file of its own that only its owner can read, apart
    // from the config folder
    let key = state(&h).join("keys/jev");
    assert_eq!(std::fs::read_to_string(&key).unwrap(), "sk-jev\n");
    assert!(!config(&h).join("keys").exists(), "no key in the config folder");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let paths = [&key, &state(&h).join("keys"), &state(&h), &config(&h), &config(&h).join("config.toml")];
        assert_eq!(paths.map(|p| mode(p)), [0o600, 0o700, 0o700, 0o700, 0o600]);
    }
    let file = config(&h).join("config.toml");
    assert!(!std::fs::read_to_string(&file).unwrap().contains("sk-jev"), "no secret in config.toml");

    // a change keeps what was written by hand
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("# my APIs\n{text}")).unwrap();
    ok(deem(&h, &["api", "edit", "kev", "--model", "kev-0.8b", "--pictures"], ""));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# my APIs\n"), "{text}");
    assert!(text.contains("model = \"kev-0.8b\"") && text.contains("rps = 20"), "{text}");
    assert!(text.contains("parallel = 4"), "{text}");

    // a key replaced, then removed
    ok(deem(&h, &["api", "edit", "jev", "--key"], "sk-new\n"));
    assert_eq!(std::fs::read_to_string(&key).unwrap(), "sk-new\n");
    ok(deem(&h, &["api", "edit", "jev", "--no-key"], ""));
    assert!(!key.exists());
    let r = deem(&h, &["api", "edit", "jev", "--key"], "\n");
    assert!(r.stderr.starts_with("deem: no key given"), "{}", r.stderr);
    let r = deem(&h, &["api", "edit", "jev", "--key"], "sk one\n");
    assert!(r.stderr.starts_with("deem: the key has spaces"), "{}", r.stderr);
    let r = deem(&h, &["api", "edit", "jev", "--key"], "sk-\x07one\n");
    assert!(r.stderr.starts_with("deem: the key has spaces"), "{}", r.stderr);

    // removing an API removes its key; the last one goes with the default
    ok(deem(&h, &["api", "edit", "jev", "--key"], "sk-jev\n"));
    ok(deem(&h, &["api", "remove", "jev"], ""));
    assert!(!key.exists());
    ok(deem(&h, &["api", "remove", "kev"], ""));
    // with none left, the file is as good as none: the environment's settings
    assert!(config(&h).join("config.toml").exists());
    let r = ok(deem(&h, &["api"], ""));
    assert!(r.stdout.starts_with("no APIs: deem uses JEV_BASE_URL"), "{}", r.stdout);
    let r = deem(&h, &["q"], "a\n");
    assert_eq!(
        (r.code, r.stderr.as_str()),
        (
            2,
            "deem: no API is added and JEV_API_KEY is not set; to add the hosted one: deem api add jev https://api.typesafe.ai/v1 --pictures --key\n"
        )
    );
    let s = fake_api();
    ok(deem_with(&h, &["-p", "q"], "a\n", &[("JEV_BASE_URL", &s.url)]));
    assert_eq!(s.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    let _ = std::fs::remove_dir_all(h);
}

// An API's `parallel` reaches its runs: a server that takes 50 ms a
// request, and counts how many it holds at once, never holds more than 2.
#[test]
fn an_apis_parallel_caps_its_requests() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let h = home("parallel");
    let (held, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (hd, m) = (held.clone(), most.clone());
    let s = stub(move |_, _, body| {
        m.fetch_max(hd.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(50));
        hd.fetch_sub(1, Ordering::SeqCst);
        let v: Json = serde_json::from_str(body).unwrap();
        let answers: serde_json::Map<String, Json> = v["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| (k.clone(), serde_json::json!({"type": "noul", "noul": 0.9})))
            .collect();
        (200, serde_json::json!({"model": "kev-latest", "answers": answers}).to_string())
    });
    ok(deem(&h, &["api", "add", "kev", &s.url, "--parallel", "2"], ""));
    let input: String = (0..20).map(|i| format!("line {i}\n")).collect();
    let r = ok(deem(&h, &["-p", "q"], &input));
    assert_eq!(r.stdout.lines().count(), 20);
    assert_eq!((most.load(Ordering::SeqCst), s.hits.load(Ordering::SeqCst)), (2, 20));
    let _ = std::fs::remove_dir_all(h);
}

#[test]
fn a_key_goes_only_to_its_api() {
    let h = home("keys");
    let (jev, kev) = (fake_api(), fake_api());
    ok(deem(&h, &["api", "add", "jev", &jev.url, "--key"], "sk-jev\n"));
    ok(deem(&h, &["api", "add", "kev", &kev.url], ""));
    let r = deem(&h, &["-p", "q"], "a\nb\n");
    assert_eq!((r.code, r.stderr.as_str()), (0, ""), "the default goes unnamed");
    assert_eq!(auth(&jev), [Some("Bearer sk-jev".to_owned()), Some("Bearer sk-jev".to_owned())]);
    for chosen in [&["--api", "kev", "-p", "q"][..], &["-p", "q", "--api=kev"]] {
        let r = deem(&h, chosen, "a\n");
        assert_eq!((r.code, r.stderr), (0, format!("deem: using kev ({})\n", kev.url)));
    }
    let r = deem_with(&h, &["-p", "q"], "a\n", &[("DEEM_API", "kev")]);
    assert_eq!((r.code, r.stderr), (0, format!("deem: using kev ({})\n", kev.url)));
    assert_eq!(auth(&kev), [None, None, None], "kev has no key, and gets none");
    assert_eq!(auth(&jev).len(), 2, "nothing more went to jev");
    // `--api` beats `DEEM_API`
    let r = deem_with(&h, &["--api", "jev", "-p", "q"], "c\n", &[("DEEM_API", "kev")]);
    assert_eq!((r.code, r.stderr), (0, String::new()));
    assert_eq!((auth(&jev).len(), auth(&kev).len()), (3, 3));
    let r = deem(&h, &["--api", "nope", "q"], "a\n");
    assert_eq!(r.stderr, "deem: there is no API named nope; `deem api` lists them\n");
    let _ = std::fs::remove_dir_all(h);
}

#[test]
fn settings_come_from_one_place() {
    let h = home("one-place");
    // no config file: the environment's settings, and nothing to choose from
    let r = deem(&h, &["--api", "kev", "q"], "a\n");
    assert_eq!(r.stderr, "deem: there are no APIs to choose kev from: add one with `deem api add`\n");
    let s = fake_api();
    ok(deem(&h, &["api", "add", "kev", &s.url], ""));
    let variables = [
        "JEV_API_KEY",
        "JEV_BASE_URL",
        "JEV_MODEL",
        "JEV_REQUESTS_PER_SECOND",
        "JEV_TOKENS_PER_SECOND",
        "JEV_PARALLEL",
    ];
    for k in variables {
        let r = deem_with(&h, &["q"], "a\n", &[(k, "1")]);
        let file = config(&h).join("config.toml");
        let want =
            format!("deem: {k} is set, but deem's APIs are in {}: unset it (`deem api` lists them)\n", file.display());
        assert_eq!((r.code, r.stderr), (2, want));
    }
    // set empty is unset, as everywhere
    ok(deem_with(&h, &["-p", "q"], "a\n", &[("JEV_API_KEY", "")]));
    // another server once: a config folder that isn't there, and the variables
    let other = fake_api();
    let env = [("XDG_CONFIG_HOME", "/nonexistent"), ("JEV_BASE_URL", other.url.as_str())];
    let r = deem_with(&h, &["-p", "q"], "a\n", &env);
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    assert_eq!(
        (other.hits.load(std::sync::atomic::Ordering::SeqCst), s.hits.load(std::sync::atomic::Ordering::SeqCst)),
        (1, 1)
    );
    // the hosted API needs a key
    ok(deem(&h, &["api", "add", "jev", "https://api.typesafe.ai/v1"], ""));
    let r = deem(&h, &["--api", "jev", "q"], "a\n");
    assert_eq!(r.stderr, "deem: API jev has no key; the hosted API needs one: `deem api edit jev --key`\n");
    let _ = std::fs::remove_dir_all(h);
}

#[test]
fn an_api_without_pictures_is_shown_none() {
    let h = home("pictures");
    let s = fake_api();
    ok(deem(&h, &["api", "add", "kev", &s.url], ""));
    std::fs::write(h.join("shot.png"), png(800, 600)).unwrap();
    let refused = "deem: API kev doesn't take pictures: choose one that does (--api NAME), \
                   or `deem api edit kev --pictures`\n";
    for args in [
        &["run", r#"ask("is there an error?"; .)"#, "shot.png"][..],
        &["run", r#"ask("does it match?"; {a: ., b: $b})"#, "--img", "b", "shot.png"],
    ] {
        let r = deem(&h, args, "a\n");
        assert_eq!((r.code, r.stderr.as_str()), (2, refused));
    }
    let r = deem(&h, &["-I", "is there an error?"], "shot.png\n");
    assert_eq!((r.code, r.stderr.as_str()), (2, refused));
    assert_eq!(s.hits.load(std::sync::atomic::Ordering::SeqCst), 0, "nothing was sent");
    ok(deem(&h, &["api", "edit", "kev", "--pictures"], ""));
    let r = deem(&h, &["run", r#"ask("is there an error?"; .)"#, "shot.png"], "");
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    let _ = std::fs::remove_dir_all(h);
}

// A key typed after `--key`, which takes none, is never printed back, in
// any of the ways it can be typed, and nothing is saved: an error is often
// pasted where others read it.
#[test]
fn a_key_typed_after_key_is_not_shown() {
    let h = home("typed-key");
    let s = fake_api();
    ok(deem(&h, &["api", "add", "jev", &s.url, "--key"], "sk-jev\n"));
    let before = std::fs::read_to_string(config(&h).join("config.toml")).unwrap();
    let key = "sk-typed-0123456789";
    let eq = format!("--key={key}");
    for args in [
        &["api", "edit", "jev", &eq][..],
        &["api", "edit", "jev", "--key", key],
        &["api", "add", "kev", &s.url, &eq],
        &["api", "add", "kev", &s.url, "--key", key],
        &["api", "add", "kev", "--key", key, &s.url],
        &["api", "add", "kev", "--key", key],
        &["api", "add", "kev", key, "--key"],
    ] {
        let r = deem(&h, args, "");
        assert_eq!(r.code, 2, "{args:?}");
        assert!(
            r.stderr.contains("--key takes no value") && !r.stderr.contains("0123456789"),
            "{args:?}: {}",
            r.stderr
        );
        assert_eq!(r.stdout, "");
    }
    let r = deem(&h, &["api", "edit", "jev", &format!("--no-key={key}")], "");
    assert_eq!((r.code, r.stderr.as_str()), (2, "deem: --no-key takes no value\n"));
    assert_eq!(std::fs::read_to_string(config(&h).join("config.toml")).unwrap(), before, "nothing was saved");
    assert_eq!(std::fs::read_to_string(state(&h).join("keys/jev")).unwrap().trim(), "sk-jev");
    let _ = std::fs::remove_dir_all(h);
}

#[cfg(unix)]
#[test]
fn a_key_others_can_read_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let h = home("readable-key");
    let s = fake_api();
    ok(deem(&h, &["api", "add", "jev", &s.url, "--key"], "sk-jev\n"));
    let key = state(&h).join("keys/jev");
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let r = deem(&h, &["q"], "a\n");
    let want = format!(
        "deem: {} can be read by others, so deem won't use it: `chmod 600` it, or set the key again\n",
        key.display()
    );
    assert_eq!((r.code, r.stderr), (2, want));
    assert_eq!(s.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    let _ = std::fs::remove_dir_all(h);
}

// Who can change the config file can send the key elsewhere, and who can
// change a folder can replace what is in it: both are refused, as a key
// others can read is. `deem api` makes its folders private, made or found.
#[cfg(unix)]
#[test]
fn a_config_others_can_change_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let h = home("changeable");
    let set = |p: &Path, m: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let (dir, file, st, keys) = (config(&h), config(&h).join("config.toml"), state(&h), state(&h).join("keys"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&st).unwrap();
    set(&dir, 0o775);
    set(&st, 0o775);
    let s = fake_api();
    ok(deem(&h, &["api", "add", "jev", &s.url, "--key"], "sk-jev\n"));
    assert_eq!((mode(&dir), mode(&file), mode(&st), mode(&keys)), (0o700, 0o600, 0o700, 0o700));
    let all = [(&dir, 0o770, 0o700), (&file, 0o620, 0o600), (&st, 0o770, 0o700), (&keys, 0o702, 0o700)];
    for (p, loose, kept) in all {
        set(p, loose);
        let r = deem(&h, &["q"], "a\n");
        let want = format!("deem: {} can be changed by others, so deem won't use it: `chmod go-w` it\n", p.display());
        assert_eq!((r.code, r.stderr), (2, want));
        set(p, kept);
    }
    assert_eq!(s.hits.load(std::sync::atomic::Ordering::SeqCst), 0, "nothing was sent");
    set(&dir, 0o775);
    ok(deem(&h, &["api", "default", "jev"], ""));
    assert_eq!(mode(&dir), 0o700);
    ok(deem(&h, &["-p", "q"], "a\n"));
    let _ = std::fs::remove_dir_all(h);
}

// TRANSPORT.md, rule 13: a reply that echoes the key, and sends escape
// codes, reaches neither the terminal, the trace nor a program as it was.
#[test]
fn what_the_api_says_is_shown_as_text() {
    let h = home("shown-as-text");
    let key = "sk-jev-s3cret/key";
    let said = format!("\x1b]0;PWNED\x07\x1b[2Jbad request; you sent Authorization: Bearer {key} \u{9b}31m");
    // as sent, and as JSON that escapes `/`
    let echoed = format!("{said} {{\"authorization\": \"Bearer {}\"}}", key.replace('/', "\\/"));
    let s = stub(move |_, line, _| match line.starts_with("GET") {
        true => (200, said.clone()),
        false => (400, echoed.clone()),
    });
    ok(deem(&h, &["api", "add", "jev", &s.url, "--key"], &format!("{key}\n")));
    let clean = |out: &str| {
        assert!(!out.contains("s3cret") && !out.contains(['\x1b', '\x07', '\u{9b}']), "{out:?}");
        assert!(out.contains("<key>") && out.contains("\\u{1b}]0;PWNED\\u{7}"), "{out:?}");
    };
    let r = deem(&h, &["--trace", "t.jsonl", "-p", "q"], "a\n");
    assert_eq!(r.code, 2, "{}", r.stderr);
    clean(&r.stderr);
    assert!(!r.stderr.contains("Bearer sk"), "{}", r.stderr);
    let trace = std::fs::read_to_string(h.join("t.jsonl")).unwrap();
    let answered: Vec<Json> =
        trace.lines().map(|l| serde_json::from_str(l).unwrap()).filter(|e: &Json| e["ev"] == "answered").collect();
    clean(answered[0]["result"].as_str().unwrap());
    let r = deem(&h, &["run", r#"try ask("q"; .) catch ."#], "a\n");
    clean(&r.stdout);
    let r = ok(deem(&h, &["models"], ""));
    clean(&r.stdout);
    let _ = std::fs::remove_dir_all(h);
}

// A key goes only where it was given for: moving an API with a key to
// another server needs --key or --no-key, said in the same edit.
#[test]
fn a_new_server_needs_its_own_key() {
    let h = home("new-server");
    ok(deem(&h, &["api", "add", "jev", "https://api.typesafe.ai/v1", "--key"], "sk-jev\n"));
    let r = deem(&h, &["api", "edit", "jev", "--url", "https://elsewhere.example/v1"], "");
    let want = "deem: API jev has a key for api.typesafe.ai, and --url moves it to elsewhere.example: \
                give --key for a key there, or --no-key to send none\n";
    assert_eq!((r.code, r.stderr.as_str()), (2, want));
    assert!(std::fs::read_to_string(config(&h).join("config.toml")).unwrap().contains("api.typesafe.ai"));
    // the same server, another path: the key stays
    ok(deem(&h, &["api", "edit", "jev", "--url", "https://API.typesafe.ai/v2"], ""));
    let r = ok(deem(&h, &["api", "edit", "jev", "--url", "https://elsewhere.example/v1", "--key"], "sk-else\n"));
    assert_eq!(r.stdout, saved(&h, "jev"));
    assert_eq!(std::fs::read_to_string(state(&h).join("keys/jev")).unwrap(), "sk-else\n");
    let r = ok(deem(&h, &["api", "edit", "jev", "--url", "http://127.0.0.1:9/v1", "--no-key"], ""));
    assert_eq!(r.stdout, format!("key for jev removed from {}\n", state(&h).join("keys/jev").display()));
    assert!(!state(&h).join("keys/jev").exists());
    // no key to remove: nothing said
    assert_eq!(ok(deem(&h, &["api", "edit", "jev", "--no-key"], "")).stdout, "");
    // without a key, nothing to move
    ok(deem(&h, &["api", "edit", "jev", "--url", "https://third.example/v1"], ""));
    let _ = std::fs::remove_dir_all(h);
}

// Without a home folder, XDG_CONFIG_HOME still finds the APIs; with no
// state folder to keep keys in, none is sent, and setting one says why.
#[test]
fn apis_without_a_state_folder() {
    let h = home("no-state");
    let s = fake_api();
    ok(deem(&h, &["api", "add", "local", &s.url], ""));
    ok(deem(&h, &["api", "add", "other", &s.url, "--key"], "sk-other\n"));
    let config_home = h.join(".config");
    let only = [("XDG_CONFIG_HOME", config_home.to_str().unwrap())];
    let at = h.to_str().unwrap();
    let r = ok(bin(&["api"], "", &only, at));
    assert!(r.stdout.starts_with("* local ") && !r.stdout.contains("key"), "{}", r.stdout);
    ok(bin(&["-p", "q"], "a\n", &only, at));
    assert_eq!(auth(&s), [None]);
    let r = bin(&["api", "edit", "local", "--key"], "sk-local\n", &only, at);
    let want = "deem: deem keeps keys in ~/.local/state/deem, and there is no home folder to find it in: \
                set HOME or XDG_STATE_HOME\n";
    assert_eq!((r.code, r.stderr.as_str()), (2, want));
    assert!(!state(&h).join("keys/local").exists());
    // with one, removing an API says where its key went
    let r = ok(deem(&h, &["api", "remove", "other"], ""));
    assert_eq!(r.stdout, format!("key for other removed from {}\n", state(&h).join("keys/other").display()));
    let _ = std::fs::remove_dir_all(h);
}

// A key file written by hand is checked as a key typed in is: one that
// can't go in a header is blamed on the file, not the URL.
#[test]
fn a_key_file_that_cant_be_sent_is_named() {
    let h = home("bad-key-file");
    let s = fake_api();
    ok(deem(&h, &["api", "add", "x", &s.url, "--key"], "sk-good\n"));
    let key = state(&h).join("keys/x");
    std::fs::write(&key, "aaaaaaaa\r\nX-Injected: 1\n").unwrap();
    let r = deem(&h, &["models"], "");
    let want = format!(
        "deem: {}: the key has spaces, several lines or control characters in it: \
         set it again with `deem api edit x --key`\n",
        key.display()
    );
    assert_eq!((r.code, r.stderr), (2, want));
    assert_eq!(s.hits.load(std::sync::atomic::Ordering::SeqCst), 0, "nothing was sent");
    let _ = std::fs::remove_dir_all(h);
}

// A setup error names the API's setting, not a variable that isn't used.
#[test]
fn setup_errors_name_the_api() {
    let h = home("setup-errors");
    let s = stub(|_, _, _| (401, "invalid key".into()));
    ok(deem(&h, &["api", "add", "kev", &s.url], ""));
    let r = deem(&h, &["q"], "a\n");
    assert_eq!(
        (r.code, r.stderr.as_str()),
        (2, "deem: the API refused the request; check the key of API kev (invalid key)\n")
    );
    let s = stub(|_, _, _| (404, "not found".into()));
    ok(deem(&h, &["api", "edit", "kev", "--url", &s.url], ""));
    let r = deem(&h, &["q"], "a\n");
    assert_eq!(r.stderr, "deem: api 404: not found; check the URL of API kev (it usually ends in /v1)\n");
    let _ = std::fs::remove_dir_all(h);
}

#[test]
fn a_trace_says_where_the_run_went() {
    let h = home("trace");
    let s = fake_api();
    ok(deem(&h, &["api", "add", "kev", &s.url, "--model", "kev-latest"], ""));
    ok(deem(&h, &["--trace", "t.jsonl", "-p", "q"], "a\n"));
    let trace = std::fs::read_to_string(h.join("t.jsonl")).unwrap();
    let head: serde_json::Value = serde_json::from_str(trace.lines().next().unwrap()).unwrap();
    assert_eq!(
        head,
        serde_json::json!({"deem_trace": 1, "clock": "real", "api": "kev", "url": s.url, "model": "kev-latest"})
    );
    let _ = std::fs::remove_dir_all(h);
}

// A file written by hand is checked whole: a misspelt setting is an error,
// not a setting silently left at its default.
#[test]
fn a_file_written_by_hand_is_checked() {
    let h = home("by-hand");
    std::fs::create_dir_all(config(&h)).unwrap();
    private(&config(&h), 0o700);
    let file = config(&h).join("config.toml");
    let path = file.display().to_string();
    for (text, why) in [
        (
            "default = \"kev\"\n[api.kev]\nurl = \"https://x/v1\"\npicture = true\n",
            "api.kev.picture is not a setting deem knows (url, model, pictures, rps, tps, parallel)",
        ),
        ("default = \"jev\"\n[api.kev]\nurl = \"https://x/v1\"\n", "default is jev, but there is no API named jev"),
        ("[api.kev]\nurl = \"https://x/v1\"\n", "no default API; choose one: `deem api default NAME`"),
        ("default = \"kev\"\n[api.kev]\nmodel = \"m\"\n", "api.kev.url: missing"),
        (
            "default = \"kev\"\n[api.kev]\nurl = \"http://x/v1\"\n",
            "api.kev.url: \"http://x/v1\" would send the key and the records in the clear: use https://, \
             or http:// only to this machine (localhost, 127.0.0.1, [::1])",
        ),
        ("default = \"kev\"\n[api.kev]\nurl = \"https://x/v1\"\nrps = 0\n", "api.kev.rps: 0 is not from 1 to 1000"),
        (
            "default = \"kev\"\n[api.kev]\nurl = \"https://x/v1\"\nparallel = 0\n",
            "api.kev.parallel: 0 is not from 1 to 256",
        ),
        (
            "default = \"kev\"\nkey = \"sk\"\n[api.kev]\nurl = \"https://x/v1\"\n",
            "key is not a setting deem knows (default, api.NAME)",
        ),
    ] {
        std::fs::write(&file, text).unwrap();
        private(&file, 0o600);
        let r = deem(&h, &["q"], "a\n");
        assert_eq!((r.code, r.stderr), (2, format!("deem: {path}: {why}\n")), "{text}");
    }
    let _ = std::fs::remove_dir_all(h);
}

// The default changing while runs start: each run reads the old file or
// the new one whole, and sends to one API.
#[test]
fn a_change_never_shows_a_run_half_a_file() {
    let h = home("concurrent");
    let (a, b) = (fake_api(), fake_api());
    ok(deem(&h, &["api", "add", "a", &a.url], ""));
    ok(deem(&h, &["api", "add", "b", &b.url], ""));
    std::thread::scope(|scope| {
        let flips = scope.spawn(|| {
            for i in 0..20 {
                ok(deem(&h, &["api", "default", ["a", "b"][i % 2]], ""));
            }
        });
        for _ in 0..20 {
            let r = deem(&h, &["-p", "q"], "x\n");
            assert_eq!((r.code, r.stderr.as_str()), (0, ""));
        }
        flips.join().unwrap();
    });
    let sent = |s: &Stub| s.hits.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(sent(&a) + sent(&b), 20, "one request a run, to one API");
    let _ = std::fs::remove_dir_all(h);
}
