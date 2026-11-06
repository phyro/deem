//! The Jev API: the request deem sends, the answers that come back, and the
//! HTTP client. Everything here is the wire format; nothing knows about jq.

use crate::transport::picture::Picture;
use serde::Serialize;
use serde_json::value::RawValue;
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// The most options the API takes for one question.
pub const MAX_OPTIONS: usize = 255;

/// The fewest and most levels the API takes for a scale (a Score question).
pub const LEVELS: (usize, usize) = (2, 10);

/// A question: what to ask, and what shape the answer takes. Neither `among`
/// nor `levels`: a calibrated float in `[0,1]`. With `among`: one of the
/// options, and `about` what each means, in the same order (empty: names
/// alone). With `levels`: a number in `[0, n−1]` on that scale, lowest
/// first. `boxed`: where in a picture, `{top, bottom, left, right}`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spec {
    pub q: String,
    pub among: Vec<String>,
    pub about: Vec<Json>,
    pub levels: Vec<Json>,
    pub boxed: bool,
    pub yes: String,
    pub no: String,
    pub like: Vec<String>,
    pub unlike: Vec<String>,
}

impl Spec {
    pub fn kind(&self) -> &'static str {
        match (self.boxed, self.among.is_empty(), self.levels.is_empty()) {
            (true, _, _) => "box",
            (_, false, _) => "choice",
            (_, true, false) => "score",
            (_, true, true) => "noul",
        }
    }
}

/// One request: a model, one state, and named questions about it. The
/// state and the questions are raw JSON, as they carry values byte for byte
/// as jaq wrote them; they never pass through a `serde_json::Value`, which
/// would round numbers and sort keys.
///
/// A request that shows pictures holds them (`pictures`) until it is sent;
/// as sent, each reference is its picture's ID, and `assets` maps each ID to
/// its file part (`picture::prepare`).
#[derive(Clone, Debug, Serialize)]
pub struct Request {
    pub model: String,
    pub state: Box<RawValue>,
    pub questions: BTreeMap<String, Box<RawValue>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, Asset>,
    #[serde(skip)]
    pub pictures: Vec<Arc<Picture>>,
}

/// A picture in a request, as the API's `assets` names it: sent as the file
/// part named by its ID.
#[derive(Clone, Debug, Serialize)]
pub struct Asset {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub part: String,
    #[serde(skip)]
    pub path: String,
    #[serde(skip)]
    pub mime: &'static str,
}

impl Request {
    /// The request body.
    pub fn body(&self) -> String {
        serde_json::to_string(self).expect("a request always serialises")
    }
}

/// An answer, in the shape `judge` hands the program.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub value: Json,
    pub confidence: Option<f64>,
    pub probabilities: Option<BTreeMap<String, f64>>,
}

impl Answer {
    /// The API names the value after the question's type (`noul`, `choice`,
    /// `score`); this is the one place that flattens it. A score's `legend`
    /// echoes the levels, which the program already has, so it is left out.
    fn from_wire(a: &Json) -> Self {
        let value = ["noul", "choice", "score", "box"].iter().find_map(|k| a.get(*k)).cloned().unwrap_or(Json::Null);
        let probabilities = a
            .get("probabilities")
            .and_then(Json::as_object)
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect());
        let confidence = a.get("confidence").and_then(Json::as_f64);
        Self { value, confidence, probabilities }
    }

    /// An answer is checked against its question: a noul is a number in
    /// `[0,1]`, a choice is one of the options, a score a number in
    /// `[0, n−1]` for `n` levels, a box four edges in `[0,1]`, `top` above
    /// `bottom` and `left` before `right`.
    pub fn check(self, spec: &Spec) -> Result<Self, String> {
        let top = spec.levels.len().saturating_sub(1).max(1);
        let edge = |k: &str| self.value.get(k).and_then(Json::as_f64).filter(|x| (0.0..=1.0).contains(x));
        let ok = match (&self.value, spec.kind()) {
            (Json::Number(n), "noul" | "score") => n.as_f64().is_some_and(|x| (0.0..=top as f64).contains(&x)),
            (Json::String(s), "choice") => spec.among.contains(s),
            (Json::Object(_), "box") => match (edge("top"), edge("bottom"), edge("left"), edge("right")) {
                (Some(t), Some(b), Some(l), Some(r)) => t < b && l < r,
                _ => false,
            },
            _ => false,
        };
        if ok {
            return Ok(self);
        }
        let want = match spec.kind() {
            "choice" => format!("one of {}", json!(spec.among)),
            "box" => "a box: top, bottom, left and right in [0,1], top < bottom, left < right".to_owned(),
            _ => format!("a number in [0,{top}]"),
        };
        Err(format!("the model answered {}, not {want}", self.value))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Response {
    /// Answers by question name, not yet checked.
    pub answers: BTreeMap<String, Answer>,
    pub input_tokens: Option<u64>,
    /// the reply as received, when it came over HTTP: `--trace` keeps it
    pub raw: Option<String>,
}

impl Response {
    pub fn decode(body: &str) -> Result<Self, ApiError> {
        let v: Json = serde_json::from_str(body).map_err(|e| ApiError::Decode(e.to_string()))?;
        let Some(answers) = v.get("answers").and_then(Json::as_object) else {
            return Err(ApiError::Decode(format!("no `answers` in {}", trim(body, 200))));
        };
        Ok(Self {
            answers: answers.iter().map(|(k, a)| (k.clone(), Answer::from_wire(a))).collect(),
            // `prompt_tokens` is what OpenAI-style servers call it
            input_tokens: ["/usage/input_tokens", "/usage/prompt_tokens"]
                .iter()
                .find_map(|p| v.pointer(p).and_then(Json::as_u64)),
            raw: Some(body.to_owned()),
        })
    }
}

/// Why a question has no answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// The request or the answer failed; the reason is for the user.
    Failed(String),
    /// The run is stopping (question limit, rejected key, closed output).
    Stopped,
}

/// What a run of a record gets when it asks a question.
#[derive(Clone, Debug, PartialEq)]
pub enum Asked {
    /// The answer, or why there is none.
    Answer(Result<Answer, String>),
    /// Not answered yet: question `.0`, queued for the record's next round.
    Later(u64),
    /// The record has as many questions unanswered as it may: the run
    /// stops, and asks this one again in its next run.
    Full,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ApiError {
    /// A reply other than 200: its status and body.
    Status(u16, String),
    /// 429 or 529: its status and body, and how long the API asked deem to
    /// wait, if it said.
    SlowDown(u16, String, Option<Duration>),
    /// A redirect: its status, and where to.
    Redirect(u16, String),
    /// A URL deem can't use, or a TLS setup that can't work.
    Setup(String),
    /// The API couldn't be reached, or didn't reply.
    Transport(String),
    /// A reply deem can't read.
    Decode(String),
    /// A picture can't be sent: why, and whether every request that shows it
    /// would fail the same way.
    Picture(String, bool),
    /// A box question the model refused (400 `Invalid request.`): models
    /// before Jev 1.14 can't box. The body.
    NoBox(String),
}

/// Where the settings a setup error names are: the environment, or an API
/// of the config file (`deem api`).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Settings {
    #[default]
    Env,
    Api(String),
}

impl Settings {
    fn url(&self) -> String {
        match self {
            Self::Env => "JEV_BASE_URL".into(),
            Self::Api(name) => format!("the URL of API {name}"),
        }
    }

    fn key(&self) -> String {
        match self {
            Self::Env => "JEV_API_KEY".into(),
            Self::Api(name) => format!("the key of API {name}"),
        }
    }

    fn model(&self) -> String {
        match self {
            Self::Env => "JEV_MODEL".into(),
            Self::Api(name) => format!("the model of API {name}"),
        }
    }

    fn use_preview(&self) -> String {
        match self {
            Self::Env => "set JEV_MODEL=jev-preview".into(),
            Self::Api(name) => format!("`deem api edit {name} --model jev-preview`"),
        }
    }
}

/// What a failure means, as TRANSPORT.md's tables sort it: whether retrying
/// could help, and whether every request would fail the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The URL is wrong: a redirect, 404, 405, a URL deem can't use.
    BadUrl,
    /// The model is wrong: 400 `Unknown model`.
    BadModel,
    /// The key is wrong: 401, 403.
    BadKey,
    /// The API asks deem to slow down: 429, 529.
    SlowDown,
    /// The API can't be reached, failed, or didn't reply: 408, 5xx.
    Unavailable,
    /// The API refused this request: any other 4xx.
    Refused,
    /// The reply is broken.
    Broken,
    /// A picture every request that shows it would fail on: an `--img` file
    /// gone or changed, or the API wanting the images header.
    BadPicture,
}

impl Kind {
    /// Every request would fail the same way, so the run aborts on the first.
    pub fn setup(self) -> bool {
        matches!(self, Self::BadUrl | Self::BadModel | Self::BadKey | Self::BadPicture)
    }

    /// The attempts a request gets while it fails this way.
    pub fn attempts(self) -> u32 {
        match self {
            Self::SlowDown => 20,
            Self::Unavailable => 6,
            _ => 1,
        }
    }
}

impl ApiError {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Status(s, body) => match s {
                401 | 403 => Kind::BadKey,
                300..=399 | 404 | 405 => Kind::BadUrl,
                400 if body.contains("Unknown model") => Kind::BadModel,
                400 if body.contains("Images are experimental") => Kind::BadPicture,
                429 | 529 => Kind::SlowDown,
                408 | 500..=599 => Kind::Unavailable,
                400..=499 => Kind::Refused,
                _ => Kind::Broken,
            },
            Self::SlowDown(..) => Kind::SlowDown,
            Self::Redirect(..) | Self::Setup(_) => Kind::BadUrl,
            Self::Transport(_) => Kind::Unavailable,
            Self::Decode(_) => Kind::Broken,
            Self::NoBox(_) => Kind::Refused,
            Self::Picture(_, every) => match every {
                true => Kind::BadPicture,
                false => Kind::Refused,
            },
        }
    }

    /// How long the API asked deem to wait before trying again.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::SlowDown(_, _, d) => *d,
            _ => None,
        }
    }
}

impl ApiError {
    /// What went wrong, naming the setting to check where `settings` are;
    /// what the server said is shown as text (TRANSPORT.md, rule 13).
    pub fn describe(&self, settings: &Settings) -> String {
        printable(&self.message(settings))
    }

    fn message(&self, settings: &Settings) -> String {
        match self {
            Self::Status(401 | 403, b) => {
                format!("the API refused the request; check {} ({})", settings.key(), trim(b, 300))
            }
            Self::Status(s @ (404 | 405), b) => {
                format!("api {s}: {}; check {} (it usually ends in /v1)", trim(b, 300), settings.url())
            }
            Self::Status(400, b) if b.contains("Unknown model") => {
                format!("the API doesn't know the model ({}); check {}", trim(b, 300), settings.model())
            }
            Self::Status(s, b) | Self::SlowDown(s, b, _) => format!("api {s}: {}", trim(b, 300)),
            Self::Redirect(s, to) => {
                let to = to.strip_suffix("/systemone").or(to.strip_suffix("/models")).unwrap_or(to);
                format!("{} is redirected ({s}) to {to}; set it to that URL", settings.url())
            }
            Self::Setup(e) => format!("{} can't be used: {e}", settings.url()),
            Self::Transport(e) => format!("request failed: {e}"),
            Self::Decode(e) => format!("decoding the API's response: {e}"),
            Self::Picture(e, _) => e.clone(),
            Self::NoBox(b) => {
                format!(
                    "api 400: {} (a box question needs Jev 1.14 or later: {})",
                    trim(b, 300),
                    settings.use_preview()
                )
            }
        }
    }
}

/// The error as the environment's settings name it.
impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.describe(&Settings::Env))
    }
}

/// `s` with its control characters but newline and tab written out
/// (`\u{1b}`), so a server's reply can't move the cursor, retitle the
/// terminal or reach its clipboard.
pub fn printable(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' | '\t' => out.push(c),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// A key shorter than this isn't looked for in replies: it would blank out
/// what merely contains it (TRANSPORT.md, rule 13).
const REDACT_FROM: usize = 8;

/// `s` with the key, as sent or JSON-escaped, replaced by `<key>`: a reply
/// that echoes the request's headers doesn't put the key in the terminal or
/// the trace (TRANSPORT.md, rule 13).
fn redact(s: String, key: &str) -> String {
    if key.len() < REDACT_FROM {
        return s;
    }
    let json = serde_json::to_string(key).unwrap_or_default();
    let json = json.trim_matches('"').to_owned();
    let mut s = s;
    for form in [key.to_owned(), json.clone(), json.replace('/', "\\/")] {
        if s.contains(&form) {
            s = s.replace(&form, "<key>");
        }
    }
    s
}

fn trim(s: &str, n: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

/// What deem sends requests to: the API, or a fake in tests.
pub trait Judge: Send + Sync {
    fn judge(&self, r: &Request) -> Result<Response, ApiError>;
}

/// Idle connections the client keeps, at most: at least one per sender
/// (`sched::MAX_WORKERS`, checked there), so a wave's requests reuse the last
/// wave's connections instead of each paying a new TLS handshake. (ureq
/// keeps 3 per host by default.) Only connections in use are opened.
pub const CONNECTIONS: usize = 256;

/// How long deem waits for a connection before it counts the API as
/// unreachable.
const CONNECT: Duration = Duration::from_secs(10);

/// The API over HTTPS, at a base URL like `https://api.typesafe.ai/v1`.
pub struct Http {
    agent: ureq::Agent,
    base: String,
    /// empty: no `authorization` header
    key: String,
}

impl Http {
    pub fn new(base: &str, key: &str, timeout: Duration) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .timeout_connect(Some(CONNECT))
            .http_status_as_error(false)
            // a redirected POST would arrive as a GET, without its key
            .max_redirects(0)
            .max_idle_connections(CONNECTIONS)
            .max_idle_connections_per_host(CONNECTIONS)
            .build()
            .new_agent();
        Self { agent, base: base.trim_end_matches('/').to_owned(), key: key.to_owned() }
    }

    /// `deem models`: the API's list of models, as it sends it, shown as
    /// text.
    pub fn models(&self) -> Result<String, ApiError> {
        self.read(self.auth(self.agent.get(format!("{}/models", self.base))).call()).map(|b| printable(&b))
    }

    /// The reply, the key redacted from all of it.
    fn read(&self, r: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<String, ApiError> {
        let key = |s: String| redact(s, &self.key);
        read(r).map(key).map_err(|e| match e {
            ApiError::Status(s, b) => ApiError::Status(s, key(b)),
            ApiError::SlowDown(s, b, w) => ApiError::SlowDown(s, key(b), w),
            ApiError::Redirect(s, to) => ApiError::Redirect(s, key(to)),
            ApiError::Setup(m) => ApiError::Setup(key(m)),
            ApiError::Transport(m) => ApiError::Transport(key(m)),
            e => e,
        })
    }

    fn auth<B>(&self, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        match self.key.is_empty() {
            true => req,
            false => req.header("authorization", format!("Bearer {}", self.key)),
        }
    }
}

fn read(r: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<String, ApiError> {
    let mut resp = r.map_err(|e| match e {
        ureq::Error::BadUri(_)
        | ureq::Error::Http(_)
        | ureq::Error::InvalidProxyUrl
        | ureq::Error::Tls(_)
        | ureq::Error::Rustls(_)
        | ureq::Error::Pem(_)
        | ureq::Error::RedirectFailed
        | ureq::Error::TooManyRedirects => ApiError::Setup(e.to_string()),
        e => ApiError::Transport(e.to_string()),
    })?;
    let status = resp.status().as_u16();
    let header = |k: &str| resp.headers().get(k).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let (location, wait) = (header("location"), retry_after(&header("retry-after-ms"), &header("retry-after")));
    let body = resp.body_mut().read_to_string().map_err(|e| ApiError::Transport(e.to_string()))?;
    match status {
        200 => Ok(body),
        300..=399 => Err(ApiError::Redirect(status, location.unwrap_or_default())),
        429 | 529 => Err(ApiError::SlowDown(status, body, wait)),
        _ => Err(ApiError::Status(status, body)),
    }
}

/// The wait a 429 asks for: `retry-after-ms` in milliseconds, or
/// `Retry-After` in seconds (an HTTP date is ignored).
fn retry_after(ms: &Option<String>, secs: &Option<String>) -> Option<Duration> {
    let parse = |v: &Option<String>, scale: f64| {
        let x: f64 = v.as_deref()?.trim().parse().ok()?;
        (x.is_finite() && x >= 0.0).then(|| Duration::from_secs_f64(x * scale))
    };
    parse(ms, 1e-3).or_else(|| parse(secs, 1.0))
}

/// The API refuses a box question on a model before Jev 1.14 with a bare
/// `Invalid request.`: say what to do.
fn box_hint(r: &Request, e: ApiError) -> ApiError {
    let boxed = r.questions.values().any(|q| q.get().contains(r#""type":"box""#));
    match e {
        ApiError::Status(400, b) if boxed && b.contains("Invalid request.") => ApiError::NoBox(b),
        e => e,
    }
}

/// The header every request that shows a picture (or asks for a box) needs:
/// pictures are experimental at the API.
pub const IMAGES_HEADER: &str = "TypeSafe-Danger-Zone-Images";

impl Judge for Http {
    /// A request with pictures goes as `multipart/form-data`: the request as
    /// the part `request`, then each picture's file, streamed, as the part its
    /// ID names (the API's recommended form: no base64).
    fn judge(&self, r: &Request) -> Result<Response, ApiError> {
        use ureq::unversioned::multipart::{Form, Part};
        let body = r.body();
        let url = format!("{}/systemone", self.base);
        if r.assets.is_empty() {
            let req = self.auth(self.agent.post(url).header("content-type", "application/json"));
            return Response::decode(&self.read(req.send(body.as_bytes()))?);
        }
        let bad = |e: String| ApiError::Picture(e, false);
        let json = Part::text(&body).mime_str("application/json").map_err(|e| bad(e.to_string()))?;
        let mut form = Form::new().part("request", json);
        for (id, a) in &r.assets {
            let file = Part::file(&a.path).map_err(|e| bad(format!("{}: {e}", a.path)))?;
            // the file's name is its ID: the path stays here
            let ext = if a.mime == "image/png" { "png" } else { "jpg" };
            let file = file.file_name(&format!("{id}.{ext}")).mime_str(a.mime).map_err(|e| bad(e.to_string()))?;
            form = form.part(id, file);
        }
        let req = self.auth(self.agent.post(url).header(IMAGES_HEADER, "true"));
        Response::decode(&self.read(req.send(form)).map_err(|e| box_hint(r, e))?)
    }
}
