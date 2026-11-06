//! Settings: the API from the config file (`deem api`), or, without one,
//! from the environment. Only the hosted API needs a key; a server of your
//! own may not.

pub mod apis;

use crate::transport::api::Settings;
use apis::Apis;
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai/v1";
pub const DEFAULT_MODEL: &str = "jev-latest";
pub const DEFAULT_MAX_QUESTIONS: u64 = 100_000;
/// The hosted API's default limits for an organization, shared by all its
/// keys (its console, 2026-10-19): requests and input tokens a second.
pub const DEFAULT_REQUESTS_PER_SECOND: u64 = 80;
pub const DEFAULT_TOKENS_PER_SECOND: u64 = 100_000;
/// The most `JEV_REQUESTS_PER_SECOND` takes.
pub const MAX_REQUESTS_PER_SECOND: u64 = 1_000;
/// The variables that set the API without APIs in a config file; with
/// some, they are refused, so a run's settings come from one place.
pub const API_VARIABLES: [&str; 6] =
    ["JEV_API_KEY", "JEV_BASE_URL", "JEV_MODEL", "JEV_REQUESTS_PER_SECOND", "JEV_TOKENS_PER_SECOND", "JEV_PARALLEL"];

pub struct Config {
    /// the API's name in the config file; `None`: from the environment
    pub api: Option<String>,
    /// `api` was chosen (`--api`, `DEEM_API`) over the file's default
    pub not_default: bool,
    /// whether the API takes pictures (the environment's: assumed so)
    pub pictures: bool,
    /// empty: send no `authorization` header
    pub key: String,
    /// where the Jev API lives: `{base_url}/systemone`, `{base_url}/models`
    pub base_url: String,
    pub model: String,
    /// the most that is judged at once
    pub max_record_bytes: usize,
    /// questions per run; `None` is unlimited
    pub max_questions: Option<u64>,
    /// requests a second at most (`JEV_REQUESTS_PER_SECOND`)
    pub requests_per_second: u64,
    /// input tokens a second at most (`JEV_TOKENS_PER_SECOND`)
    pub tokens_per_second: u64,
    /// requests in flight: as many as keep the rate busy, at most
    /// `--parallel` (`JEV_PARALLEL`)
    pub workers: usize,
    /// records in flight: a request's worth each
    pub window: usize,
    /// the first retry's delay
    pub backoff: Duration,
    pub timeout: Duration,
    /// how long "slow down" answers (429, 529) are retried while no request
    /// succeeds (TRANSPORT.md, the API errors table)
    pub busy_for: Duration,
    /// how long retries or throttling may hold the invocation up before
    /// deem says why (TRANSPORT.md, rule 11)
    pub notice_after: Duration,
    /// memo entries past which answered ones no record holds are dropped
    /// (TRANSPORT.md, rule 3)
    pub memo: usize,
    /// `--trace` on virtual time, where only the API takes time (for tests)
    pub virtual_time: bool,
}

impl Config {
    /// The settings for a run: the API `choice` names (`--api`), else
    /// `DEEM_API`, else the config file's default; without APIs in a config
    /// file, the environment's. `get` reads an environment variable; empty counts
    /// as unset.
    pub fn load(get: impl Fn(&str) -> Option<String>, choice: Option<&str>) -> Result<Self, String> {
        let get = |k: &str| get(k).filter(|v| !v.is_empty());
        let chosen = choice.map(str::to_owned).or_else(|| get("DEEM_API"));
        let apis = match apis::dirs(&get) {
            Some(dirs) => Apis::read(&dirs)?.filter(|a| !a.is_empty()),
            None => None,
        };
        let Some(apis) = apis else {
            return match chosen {
                Some(name) => Err(format!("there are no APIs to choose {name} from: add one with `deem api add`")),
                None => Self::from_env(get),
            };
        };
        if let Some(k) = API_VARIABLES.iter().find(|k| get(k).is_some()) {
            return Err(format!(
                "{k} is set, but deem's APIs are in {}: unset it (`deem api` lists them)",
                apis.path()
            ));
        }
        let default = apis.default_name().map(str::to_owned);
        // a file with APIs has a default (`Apis::read` checks it)
        let name = chosen.or(default.clone()).ok_or_else(|| format!("{}: no default API", apis.path()))?;
        let api = apis.get(&name)?;
        let key = apis.key(&name)?;
        let rps = api.rps.unwrap_or(DEFAULT_REQUESTS_PER_SECOND);
        let tps = api.tps.unwrap_or(DEFAULT_TOKENS_PER_SECOND);
        let mut c = Self::new(key, api.url, api.model, (rps, tps, api.parallel), max_record_bytes(&get)?);
        c.not_default = default.as_deref() != Some(&name);
        c.api = Some(name);
        c.pictures = api.pictures;
        Ok(c)
    }

    /// `get` reads an environment variable; empty counts as unset.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let get = |k: &str| get(k).filter(|v| !v.is_empty());
        let base_url = get("JEV_BASE_URL").unwrap_or_else(|| DEFAULT_BASE_URL.into());
        apis::check_url(&base_url).map_err(|e| format!("JEV_BASE_URL: {e}"))?;
        let rps = num(&get, "JEV_REQUESTS_PER_SECOND")?.unwrap_or(DEFAULT_REQUESTS_PER_SECOND);
        apis::check_rps(rps).map_err(|e| format!("JEV_REQUESTS_PER_SECOND: {e}"))?;
        let tps = num(&get, "JEV_TOKENS_PER_SECOND")?.unwrap_or(DEFAULT_TOKENS_PER_SECOND);
        apis::check_tps(tps).map_err(|e| format!("JEV_TOKENS_PER_SECOND: {e}"))?;
        let parallel = num(&get, "JEV_PARALLEL")?;
        parallel.map(apis::check_parallel).transpose().map_err(|e| format!("JEV_PARALLEL: {e}"))?;
        let key = get("JEV_API_KEY").unwrap_or_default();
        let model = get("JEV_MODEL").unwrap_or_else(|| DEFAULT_MODEL.into());
        Ok(Self::new(key, base_url, model, (rps, tps, parallel), max_record_bytes(&get)?))
    }

    fn new(
        key: String,
        base_url: String,
        model: String,
        (rps, tps, parallel): (u64, u64, Option<u64>),
        max_record_bytes: usize,
    ) -> Self {
        // requests in flight: the rate kept up at 1.2 s a request, at most
        // `MAX_WORKERS` threads and connections, and at most what the API
        // works on at once, if it says (`--parallel`): a request beyond it
        // would wait in the server's queue, and its reply timeout with it
        let workers = (rps as usize * 6).div_ceil(5).min(crate::transport::sched::MAX_WORKERS);
        let workers = parallel.map_or(workers, |n| workers.min(n as usize));
        Self {
            api: None,
            not_default: false,
            pictures: true,
            key,
            base_url,
            model,
            max_record_bytes,
            max_questions: Some(DEFAULT_MAX_QUESTIONS),
            requests_per_second: rps,
            tokens_per_second: tps,
            workers,
            // two requests' worth of records for each sender: enough to
            // keep every sender busy while other records run between their
            // questions, and no more, as what queues behind the senders
            // only waits there
            window: 2 * workers,
            backoff: Duration::from_millis(250),
            timeout: Duration::from_secs(60),
            busy_for: Duration::from_secs(60),
            notice_after: Duration::from_secs(10),
            memo: 100_000,
            virtual_time: false,
        }
    }

    /// Where a setup error says to look: the API's settings, or the
    /// environment's.
    pub fn settings(&self) -> Settings {
        match &self.api {
            Some(name) => Settings::Api(name.clone()),
            None => Settings::Env,
        }
    }

    /// The hosted API needs a key; without one, the first wave of records
    /// would be sent before its refusal stopped the run.
    pub fn missing_key(&self) -> bool {
        self.key.is_empty() && self.base_url.trim_end_matches('/') == DEFAULT_BASE_URL
    }

    /// What to say when `missing_key`.
    pub fn no_key(&self) -> String {
        match &self.api {
            Some(name) => format!("API {name} has no key; the hosted API needs one: `deem api edit {name} --key`"),
            None => NO_KEY.into(),
        }
    }
}

pub const NO_KEY: &str = "no API is added and JEV_API_KEY is not set; to add the hosted one: deem api add jev https://api.typesafe.ai/v1 --pictures --key";

fn num(get: &impl Fn(&str) -> Option<String>, k: &str) -> Result<Option<u64>, String> {
    get(k).map(|v| v.trim().parse().map_err(|_| format!("{k}: {v:?} is not a whole number"))).transpose()
}

/// `DEEM_MAX_RECORD_BYTES`, which is the run's, whatever the API.
fn max_record_bytes(get: &impl Fn(&str) -> Option<String>) -> Result<usize, String> {
    Ok(num(get, "DEEM_MAX_RECORD_BYTES")?.filter(|&n| n > 0).unwrap_or(32 << 10) as usize)
}
