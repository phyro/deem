//! `deem api`: list and change the APIs in the config file.

use super::args::ApiCmd;
use deem::apis::{self, Apis};
use std::io::{IsTerminal, Read};
use std::path::PathBuf;

/// Run the command; what it prints. Without a config folder (no home
/// folder) there are no APIs to list, and none can be changed.
pub fn api(cmd: ApiCmd, dirs: Option<apis::Dirs>) -> Result<String, String> {
    let read = || dirs.as_ref().map(Apis::read).transpose().map(Option::flatten);
    let change = |f: &mut dyn FnMut(&mut Apis) -> Result<String, String>| {
        let dirs = dirs.as_ref().ok_or(NO_HOME)?;
        Apis::change(dirs, f)
    };
    match cmd {
        ApiCmd::List => list(read()?),
        ApiCmd::Default(None) => match read()?.as_ref().and_then(Apis::default_name) {
            Some(name) => Ok(format!("{name}\n")),
            None => Err(NONE.into()),
        },
        ApiCmd::Default(Some(name)) => change(&mut |a| a.set_default(&name).map(|_| String::new())),
        ApiCmd::Remove(name) => change(&mut |a| Ok(told(&name, false, a.remove(&name)?))),
        ApiCmd::Add { api, key } => {
            let key = key.then(|| read_key(&api.name)).transpose()?;
            change(&mut |a| {
                a.add(&api)?;
                match key.as_deref() {
                    Some(k) => Ok(told(&api.name, true, a.set_key(&api.name, Some(k))?)),
                    None => Ok(String::new()),
                }
            })
        }
        ApiCmd::Edit { name, change: c, key } => {
            let key = match key {
                Some(true) => Some(Some(read_key(&name)?)),
                Some(false) => Some(None),
                None => None,
            };
            change(&mut |a| {
                moves_key(a, &name, &c, key.is_some())?;
                a.edit(&name, &c)?;
                match &key {
                    Some(k) => Ok(told(&name, k.is_some(), a.set_key(&name, k.as_deref())?)),
                    None => Ok(String::new()),
                }
            })
        }
    }
}

/// Where a key went: the file it was saved in, or removed from; nothing if
/// there was none to remove.
fn told(name: &str, saved: bool, file: Option<PathBuf>) -> String {
    let Some(file) = file else { return String::new() };
    match saved {
        true => format!(
            "key for {name} saved in {}{}\n",
            file.display(),
            if cfg!(unix) { ", readable only by you" } else { "" }
        ),
        false => format!("key for {name} removed from {}\n", file.display()),
    }
}

const NO_HOME: &str =
    "deem keeps its APIs in ~/.config/deem, and there is no home folder to find it in: set HOME or XDG_CONFIG_HOME";

const NONE: &str = "there are no APIs: add one with `deem api add NAME URL`";

/// A row for each API, the default marked `*`; a key is named, never shown.
fn list(apis: Option<Apis>) -> Result<String, String> {
    let Some(apis) = apis.filter(|a| !a.is_empty()) else {
        return Ok("no APIs: deem uses JEV_BASE_URL, JEV_MODEL and JEV_API_KEY. \
             Add one with `deem api add NAME URL`\n"
            .into());
    };
    let all = apis.list()?;
    let rows: Vec<[String; 5]> = all
        .iter()
        .map(|a| {
            let mark = if apis.default_name() == Some(a.name.as_str()) { "*" } else { " " };
            let mut also = Vec::new();
            a.pictures.then(|| also.push("pictures".to_owned()));
            apis.has_key(&a.name).then(|| also.push("key".to_owned()));
            a.rps.iter().for_each(|n| also.push(format!("rps {n}")));
            a.tps.iter().for_each(|n| also.push(format!("tps {n}")));
            a.parallel.iter().for_each(|n| also.push(format!("parallel {n}")));
            [mark.into(), a.name.clone(), a.url.clone(), a.model.clone(), also.join("  ")]
        })
        .collect();
    let width = |i: usize| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0);
    let (name, url, model) = (width(1), width(2), width(3));
    let line = |r: &[String; 5]| format!("{} {:name$}  {:url$}  {:model$}  {}", r[0], r[1], r[2], r[3], r[4]);
    Ok(rows.iter().map(|r| line(r).trim_end().to_owned() + "\n").collect())
}

/// A key goes only where it was given for: a new URL on another server
/// takes a new key (`--key`) or none (`--no-key`), said in the same edit.
fn moves_key(a: &Apis, name: &str, c: &apis::Change, said: bool) -> Result<(), String> {
    let Some(url) = &c.url else { return Ok(()) };
    let from = a.get(name)?.url;
    let (old, new) = (apis::authority(&from), apis::authority(url));
    if said || !a.has_key(name) || old.eq_ignore_ascii_case(new) {
        return Ok(());
    }
    Err(format!(
        "API {name} has a key for {old}, and --url moves it to {new}: \
         give --key for a key there, or --no-key to send none"
    ))
}

/// The key, from the terminal without echoing it, or from stdin: never an
/// argument, which `ps` and the shell's history would show.
fn read_key(name: &str) -> Result<String, String> {
    let key = if std::io::stdin().is_terminal() {
        rpassword::prompt_password(format!("key for {name}: ")).map_err(|e| format!("reading the key: {e}"))?
    } else {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(|e| format!("reading the key from stdin: {e}"))?;
        s
    };
    let key = key.trim();
    if key.is_empty() {
        return Err("no key given: type it when asked, or pipe it in (pass show jev | deem api edit jev --key)".into());
    }
    apis::check_key(key).map_err(|e| format!("{e}; give the key alone"))?;
    Ok(key.to_owned())
}

/// deem's folders, from the environment.
pub fn dirs() -> Option<apis::Dirs> {
    apis::dirs(&|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
}
