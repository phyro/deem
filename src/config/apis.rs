//! The APIs deem can send to, named in `config.toml` in deem's config folder
//! (`$XDG_CONFIG_HOME/deem`, else `~/.config/deem`), each key in `keys/NAME`
//! in its state folder (`$XDG_STATE_HOME/deem`, else `~/.local/state/deem`),
//! kept apart so that a config folder copied to another machine carries no
//! key. `deem api` changes them; a run reads them once, as it starts.
//!
//! ```toml
//! default = "jev"
//!
//! [api.jev]
//! url = "https://api.typesafe.ai/v1"
//! model = "jev-latest"
//! pictures = true
//!
//! [api.kev]
//! url = "http://127.0.0.1:8009/v1"
//! model = "kev-latest"
//! ```
//!
//! A change is written to a new file that then replaces the old one, so a
//! run starting meanwhile reads one or the other whole; and changes wait
//! for each other on `config.lock`, so none is lost.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table, value};

pub use super::DEFAULT_MODEL;

/// One API, as the file names it.
#[derive(Clone, Debug, PartialEq)]
pub struct Api {
    pub name: String,
    /// where the Jev API lives: `{url}/systemone`, `{url}/models`
    pub url: String,
    pub model: String,
    /// whether it takes pictures: a run that shows one to an API that
    /// doesn't is refused before anything is sent
    pub pictures: bool,
    /// requests a second at most; unset, the hosted API's default
    pub rps: Option<u64>,
    /// input tokens a second at most; unset, the hosted API's default
    pub tps: Option<u64>,
    /// requests at the API at once, at most; unset, as many as the rate
    /// keeps busy
    pub parallel: Option<u64>,
}

/// A change to an API's settings: what is set is changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub url: Option<String>,
    pub model: Option<String>,
    pub pictures: Option<bool>,
    pub rps: Option<u64>,
    pub tps: Option<u64>,
    pub parallel: Option<u64>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The config file, read.
pub struct Apis {
    dirs: Dirs,
    doc: DocumentMut,
}

/// deem's folders: the config file's, and the keys'.
#[derive(Clone, Debug)]
pub struct Dirs {
    /// `config.toml`
    pub config: PathBuf,
    /// `keys/NAME`; none without a home folder or `XDG_STATE_HOME`, and
    /// then no API has a key, and none can be given one
    pub state: Option<PathBuf>,
}

/// Why a key can't be kept.
pub const NO_STATE: &str = "deem keeps keys in ~/.local/state/deem, and there is no home folder to find it in: \
                            set HOME or XDG_STATE_HOME";

const FILE: &str = "config.toml";
const LOCK: &str = "config.lock";
const KEYS: &str = "keys";
const FIELDS: [&str; 6] = ["url", "model", "pictures", "rps", "tps", "parallel"];

/// deem's folders: `$XDG_CONFIG_HOME/deem`, else `.config/deem` in the home
/// folder (`HOME`, else Windows' `USERPROFILE`), and `$XDG_STATE_HOME/deem`,
/// else `.local/state/deem` there; none without a config folder.
pub fn dirs(get: &impl Fn(&str) -> Option<String>) -> Option<Dirs> {
    let home = get("HOME").or_else(|| get("USERPROFILE")).map(PathBuf::from);
    let at = |var: &str, under: &[&str]| match get(var).map(PathBuf::from).filter(|p| p.is_absolute()) {
        Some(x) => Some(x.join("deem")),
        None => home.as_ref().map(|h| under.iter().fold(h.clone(), |p, d| p.join(d)).join("deem")),
    };
    Some(Dirs { config: at("XDG_CONFIG_HOME", &[".config"])?, state: at("XDG_STATE_HOME", &[".local", "state"]) })
}

/// An API's name is a file's name too: letters, digits, `-` and `_`.
pub fn check_name(name: &str) -> Result<(), String> {
    let ok = name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    match ok {
        true => Ok(()),
        false => Err(format!("{name:?} can't name an API: use letters, digits, - and _")),
    }
}

/// A URL deem can POST `{url}/systemone` to: `http://` only to this
/// machine, as anything else would carry the key and the records in the
/// clear; and no name or password in it, as deem shows the URL where it
/// says where a run goes. That error doesn't show it.
pub fn check_url(url: &str) -> Result<(), String> {
    let authority = authority(url);
    if authority.contains('@') {
        return Err("the URL has a name or password in it (before @), which deem would show wherever it \
                    names the URL: give the key as a key (--key, or JEV_API_KEY)"
            .into());
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("{url:?} is not an http:// or https:// URL"));
    };
    if url.trim_end_matches('/').ends_with("/systemone") {
        return Err(format!("{url:?} names the endpoint; give the base URL, without `/systemone`"));
    }
    if url.starts_with("http://") && !loopback(authority) {
        return Err(format!(
            "{url:?} would send the key and the records in the clear: use https://, \
             or http:// only to this machine (localhost, 127.0.0.1, [::1])"
        ));
    }
    Ok(())
}

/// Where a URL goes: its `host:port`, as written.
pub fn authority(url: &str) -> &str {
    url.split_once("://").map_or("", |(_, r)| r.split(['/', '?', '#']).next().unwrap_or(""))
}

/// Whether the host of a URL's `authority` (`host:port`) is this machine.
fn loopback(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// A key goes in a header: one word, nothing a header can't carry.
pub fn check_key(key: &str) -> Result<(), String> {
    match key.contains(|c: char| c.is_whitespace() || c.is_control()) {
        true => Err("the key has spaces, several lines or control characters in it".into()),
        false => Ok(()),
    }
}

pub fn check_rps(rps: u64) -> Result<(), String> {
    match (1..=super::MAX_REQUESTS_PER_SECOND).contains(&rps) {
        true => Ok(()),
        false => Err(format!("{rps} is not from 1 to {}", super::MAX_REQUESTS_PER_SECOND)),
    }
}

pub fn check_tps(tps: u64) -> Result<(), String> {
    match tps {
        0 => Err("0 would send nothing; give at least 1".into()),
        _ => Ok(()),
    }
}

pub fn check_parallel(n: u64) -> Result<(), String> {
    let max = crate::transport::sched::MAX_WORKERS as u64;
    match (1..=max).contains(&n) {
        true => Ok(()),
        false => Err(format!("{n} is not from 1 to {max}")),
    }
}

impl Apis {
    /// The file in `dirs.config`; `None` if there is none.
    pub fn read(dirs: &Dirs) -> Result<Option<Self>, String> {
        let dir = &dirs.config;
        let path = dir.join(FILE);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        guarded(dir)?;
        guarded(&path)?;
        let doc = text.parse::<DocumentMut>().map_err(|e| format!("{}: {e}", path.display()))?;
        let apis = Self { dirs: dirs.clone(), doc };
        apis.check()?;
        Ok(Some(apis))
    }

    /// Change the file in `dirs.config`, made if there is none, with `f`; no
    /// other change happens meanwhile.
    pub fn change<T>(dirs: &Dirs, f: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        let dir = &dirs.config;
        let at = |e: io::Error| format!("{}: {e}", dir.display());
        private_dir(dir).map_err(at)?;
        let lock = File::options().create(true).truncate(false).write(true).open(dir.join(LOCK)).map_err(at)?;
        lock.lock().map_err(at)?;
        let mut apis = Self::read(dirs)?.unwrap_or_else(|| Self { dirs: dirs.clone(), doc: DocumentMut::new() });
        let out = f(&mut apis)?;
        apis.check()?;
        replace(&apis.file(), apis.doc.to_string().as_bytes()).map_err(|e| format!("{}: {e}", apis.path()))?;
        Ok(out)
    }

    fn file(&self) -> PathBuf {
        self.dirs.config.join(FILE)
    }

    /// The file's path, for messages.
    pub fn path(&self) -> String {
        self.file().display().to_string()
    }

    /// The APIs, in the file's order.
    pub fn list(&self) -> Result<Vec<Api>, String> {
        self.tables().map(|(name, t)| self.api(name, t)).collect()
    }

    /// No APIs: a run uses the environment's settings.
    pub fn is_empty(&self) -> bool {
        self.tables().next().is_none()
    }

    pub fn default_name(&self) -> Option<&str> {
        self.doc.get("default").and_then(Item::as_str)
    }

    pub fn get(&self, name: &str) -> Result<Api, String> {
        match self.tables().find(|(n, _)| *n == name) {
            Some((n, t)) => self.api(n, t),
            None => Err(format!("there is no API named {name}; `deem api` lists them")),
        }
    }

    pub fn has_key(&self, name: &str) -> bool {
        self.key_file(name).is_some_and(|p| p.exists())
    }

    /// The API's key; empty if it has none. A key file others can read is
    /// refused, as ssh refuses such a private key.
    pub fn key(&self, name: &str) -> Result<String, String> {
        let (Some(state), Some(path)) = (&self.dirs.state, self.key_file(name)) else { return Ok(String::new()) };
        let at = |e: io::Error| format!("{}: {e}", path.display());
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
            Err(e) => return Err(at(e)),
        };
        guarded(state)?;
        guarded(&state.join(KEYS))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(format!(
                    "{} can be read by others, so deem won't use it: `chmod 600` it, or set the key again",
                    path.display()
                ));
            }
        }
        let _ = meta;
        let key = fs::read_to_string(&path).map_err(at)?.trim().to_owned();
        check_key(&key)
            .map_err(|e| format!("{}: {e}: set it again with `deem api edit {name} --key`", path.display()))?;
        Ok(key)
    }

    /// Add an API; the first one added is the default.
    pub fn add(&mut self, api: &Api) -> Result<(), String> {
        check_name(&api.name)?;
        if self.tables().any(|(n, _)| n == api.name) {
            return Err(format!("there is already an API named {}; change it with `deem api edit`", api.name));
        }
        let mut t = Table::new();
        t["url"] = value(&api.url);
        t["model"] = value(&api.model);
        t["pictures"] = value(api.pictures);
        api.rps.into_iter().for_each(|n| t["rps"] = value(n as i64));
        api.tps.into_iter().for_each(|n| t["tps"] = value(n as i64));
        api.parallel.into_iter().for_each(|n| t["parallel"] = value(n as i64));
        if self.default_name().is_none() {
            self.doc["default"] = value(&api.name);
        }
        let all = self.doc.entry("api").or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        });
        let all = all.as_table_mut().ok_or("`api` in the config file is not a table")?;
        all.insert(&api.name, Item::Table(t));
        Ok(())
    }

    pub fn edit(&mut self, name: &str, c: &Change) -> Result<(), String> {
        self.get(name)?;
        let t = self.doc["api"][name].as_table_mut().expect("checked by get");
        c.url.iter().for_each(|u| t["url"] = value(u));
        c.model.iter().for_each(|m| t["model"] = value(m));
        c.pictures.into_iter().for_each(|p| t["pictures"] = value(p));
        c.rps.into_iter().for_each(|n| t["rps"] = value(n as i64));
        c.tps.into_iter().for_each(|n| t["tps"] = value(n as i64));
        c.parallel.into_iter().for_each(|n| t["parallel"] = value(n as i64));
        Ok(())
    }

    /// Remove an API and its key; the key file removed, if there was one.
    /// The default goes only if it is the last: deem doesn't choose another
    /// for you.
    pub fn remove(&mut self, name: &str) -> Result<Option<PathBuf>, String> {
        self.get(name)?;
        let others = self.tables().filter(|(n, _)| *n != name).count();
        if self.default_name() == Some(name) {
            if others > 0 {
                return Err(format!(
                    "{name} is the default API; make another the default first: `deem api default NAME`"
                ));
            }
            self.doc.remove("default");
        }
        self.doc["api"].as_table_mut().expect("checked by get").remove(name);
        self.set_key(name, None)
    }

    pub fn set_default(&mut self, name: &str) -> Result<(), String> {
        self.get(name)?;
        self.doc["default"] = value(name);
        Ok(())
    }

    /// Write the API's key, or remove it (`None`), in a file only its owner
    /// can read; the file written or removed, if any.
    pub fn set_key(&self, name: &str, key: Option<&str>) -> Result<Option<PathBuf>, String> {
        let (Some(state), Some(path)) = (&self.dirs.state, self.key_file(name)) else {
            return key.map_or(Ok(None), |_| Err(NO_STATE.into()));
        };
        let at = |e: io::Error| format!("{}: {e}", path.display());
        match key {
            Some(k) => {
                private_dir(state).map_err(at)?;
                private_dir(&state.join(KEYS)).map_err(at)?;
                replace(&path, format!("{k}\n").as_bytes()).map_err(at)?;
                Ok(Some(path))
            }
            None => match fs::remove_file(&path) {
                Ok(()) => Ok(Some(path)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(at(e)),
            },
        }
    }

    fn key_file(&self, name: &str) -> Option<PathBuf> {
        self.dirs.state.as_ref().map(|s| s.join(KEYS).join(name))
    }

    fn tables(&self) -> impl Iterator<Item = (&str, &Table)> {
        let all = self.doc.get("api").and_then(Item::as_table);
        all.into_iter().flat_map(|t| t.iter()).filter_map(|(n, i)| Some((n, i.as_table()?)))
    }

    fn api(&self, name: &str, t: &Table) -> Result<Api, String> {
        let bad = |what: &str, why: String| format!("{}: api.{name}.{what}: {why}", self.path());
        let text = |k: &str| -> Result<Option<String>, String> {
            t.get(k).map(|i| i.as_str().map(str::to_owned).ok_or_else(|| bad(k, "not a string".into()))).transpose()
        };
        let num = |k: &str| -> Result<Option<u64>, String> {
            let n = t.get(k).map(|i| i.as_integer().ok_or_else(|| bad(k, "not a whole number".into()))).transpose()?;
            n.map(|n| u64::try_from(n).map_err(|_| bad(k, format!("{n} is negative")))).transpose()
        };
        let url = text("url")?.ok_or_else(|| bad("url", "missing".into()))?;
        check_url(&url).map_err(|e| bad("url", e))?;
        let pictures = match t.get("pictures") {
            None => false,
            Some(i) => i.as_bool().ok_or_else(|| bad("pictures", "not true or false".into()))?,
        };
        let (rps, tps) = (num("rps")?, num("tps")?);
        rps.map(check_rps).transpose().map_err(|e| bad("rps", e))?;
        tps.map(check_tps).transpose().map_err(|e| bad("tps", e))?;
        let parallel = num("parallel")?;
        parallel.map(check_parallel).transpose().map_err(|e| bad("parallel", e))?;
        let model = text("model")?.unwrap_or_else(|| DEFAULT_MODEL.into());
        Ok(Api { name: name.into(), url, model, pictures, rps, tps, parallel })
    }

    /// Every setting is one deem knows, and has a value it can use.
    fn check(&self) -> Result<(), String> {
        let path = self.path();
        if let Some((k, _)) = self.doc.iter().find(|(k, _)| !["default", "api"].contains(k)) {
            return Err(format!("{path}: {k} is not a setting deem knows (default, api.NAME)"));
        }
        let all = match self.doc.get("api") {
            None => {
                return self
                    .default_name()
                    .map_or(Ok(()), |d| Err(format!("{path}: default is {d}, but there are no APIs")));
            }
            Some(i) => i.as_table().ok_or(format!("{path}: api is not a table"))?,
        };
        for (name, item) in all.iter() {
            let t = item.as_table().ok_or(format!("{path}: api.{name} is not a table"))?;
            check_name(name).map_err(|e| format!("{path}: {e}"))?;
            if let Some((k, _)) = t.iter().find(|(k, _)| !FIELDS.contains(k)) {
                return Err(format!("{path}: api.{name}.{k} is not a setting deem knows ({})", FIELDS.join(", ")));
            }
            self.api(name, t)?;
        }
        match (self.doc.get("default"), all.is_empty()) {
            (None, false) => Err(format!("{path}: no default API; choose one: `deem api default NAME`")),
            (Some(i), _) => match i.as_str() {
                Some(d) if all.contains_key(d) => Ok(()),
                Some(d) => Err(format!("{path}: default is {d}, but there is no API named {d}")),
                None => Err(format!("{path}: default is not a string")),
            },
            (None, true) => Ok(()),
        }
    }
}

/// Make `dir` if it isn't there; only its owner may open it, made or found.
fn private_dir(dir: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// A folder or file others can change is refused: who can change the
/// config file can send the key elsewhere, and who can change a folder can
/// replace what is in it.
fn guarded(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.permissions().mode();
        if mode & 0o022 != 0 {
            return Err(format!("{} can be changed by others, so deem won't use it: `chmod go-w` it", path.display()));
        }
    }
    let _ = path;
    Ok(())
}

/// Write `path` whole, that only its owner can read: a new file beside it,
/// then renamed over it. The new file is made afresh, so one left by a run
/// that died, whatever its mode, or a link, never gets the bytes.
fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{name}.{}", std::process::id()));
    match fs::remove_file(&tmp) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut o = File::options();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let wrote = o.open(&tmp).and_then(|mut f| f.write_all(bytes).and_then(|_| f.sync_all()));
    match wrote.and_then(|_| fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    // What a run that died left where the new file goes: a file others can
    // read, or a link to elsewhere. Neither gets the bytes.
    #[test]
    fn a_left_file_never_gets_the_bytes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("deem-replace-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (path, tmp, elsewhere) =
            (dir.join("key"), dir.join(format!(".key.{}", std::process::id())), dir.join("elsewhere"));
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        fs::write(&tmp, "old").unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).unwrap();
        replace(&path, b"sk-1\n").unwrap();
        assert_eq!((fs::read_to_string(&path).unwrap(), mode(&path)), ("sk-1\n".into(), 0o600));
        fs::write(&elsewhere, "mine").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &tmp).unwrap();
        replace(&path, b"sk-2\n").unwrap();
        assert_eq!((fs::read_to_string(&path).unwrap(), mode(&path)), ("sk-2\n".into(), 0o600));
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "mine");
        assert!(!tmp.exists());
        let _ = fs::remove_dir_all(dir);
    }
}
