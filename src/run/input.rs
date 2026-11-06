//! Records. The input is UTF-8 text, and a byte-order mark at its start (or
//! at the start of each file) is dropped. As text, a record is a line, or a
//! chunk between `--split` matches, or with `-s` the whole input. With `-J`,
//! a record is a JSON value, however it is laid out, read as jaq reads it;
//! with `-J -s`, the input's values as one array. Nothing is guessed: the
//! command line says which.
//!
//! A picture file named on the command line (a PNG or JPEG, known by its
//! first bytes) is one record, and so is each path on stdin with `-I`; a
//! run's files are all pictures or all text. Picture bytes anywhere else in
//! the input stop it there.
//!
//! A JSON value can't cross threads, so the reader hands each record's
//! thread its value as compact JSON, parsed again there.

use crate::engine::J;
use crate::transport::picture::{self, Picture, Pictures};
use regex_lite::Regex;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// The longest record deem reads.
const MAX_RECORD: usize = 64 << 20;

const BOM: &[u8] = b"\xef\xbb\xbf";

/// One record, as read.
pub enum Raw {
    Text(String),
    /// compact JSON
    Json(String),
    /// a picture file, by its path
    Picture(String),
    /// `input: "all"`: every record, and the separator `--failed` writes
    /// between them
    All(Vec<Raw>, &'static str),
}

/// Why a record has no value.
pub enum Unvalued {
    /// its picture can't be used; the record fails
    Picture(String),
    /// deem's own JSON didn't parse again
    Bug(String),
}

impl Raw {
    /// The record as `--failed` writes it: text as read, JSON compact, a
    /// picture as its path.
    pub fn text(&self) -> String {
        match self {
            Raw::Text(s) | Raw::Json(s) | Raw::Picture(s) => s.clone(),
            Raw::All(rs, sep) => rs.iter().map(Raw::text).collect::<Vec<_>>().join(sep),
        }
    }

    /// The record's value, on its own thread. A picture's value is its
    /// reference, and `held` keeps the picture while the record runs.
    pub fn value(self, pictures: &Pictures, held: &mut Vec<Arc<Picture>>) -> Result<J, Unvalued> {
        match self {
            Raw::Text(s) => Ok(J::from(s)),
            Raw::Json(s) => {
                jaq_json::read::parse_single(s.as_bytes()).map_err(|e| Unvalued::Bug(format!("reading {s}: {e}")))
            }
            Raw::Picture(path) => {
                let p = pictures.add(&path).map_err(Unvalued::Picture)?;
                let reference = J::from(p.reference.clone());
                held.push(p);
                Ok(reference)
            }
            Raw::All(rs, _) => rs.into_iter().map(|r| r.value(pictures, held)).collect(),
        }
    }
}

/// What a run reads: text (lines or JSON), picture files, or with `-I` the
/// paths of pictures, one a line.
pub enum Input {
    Text(Box<dyn BufRead + Send>),
    Pictures(Vec<String>),
    Paths(Box<dyn BufRead + Send>),
}

impl std::convert::From<Box<dyn BufRead + Send>> for Input {
    fn from(src: Box<dyn BufRead + Send>) -> Self {
        Input::Text(src)
    }
}

/// The input: the files, or stdin; with `paths` (`-I`), stdin's lines are
/// pictures' paths. A file is a picture if it starts like a PNG or JPEG,
/// else text; a run's files are all one or the other.
pub fn open_input(files: &[String], paths: bool) -> Result<Input, String> {
    if paths {
        return match files {
            [] => Ok(Input::Paths(Box::new(BufReader::new(io::stdin())))),
            _ => Err("-I reads pictures' paths from stdin; give the pictures as files without it".into()),
        };
    }
    // each file is opened once, and a text file's first bytes, which say
    // what it is, are kept for reading it: a pipe (`<(…)`) can't be read
    // twice. A picture is closed here, and read when it is sent.
    let (mut kinds, mut opened) = (Vec::new(), Vec::new());
    for path in files {
        let at = |e: io::Error| format!("{path}: {e}");
        let mut f = File::open(path).map_err(at)?;
        let mut first = Vec::with_capacity(64);
        (&mut f).take(64).read_to_end(&mut first).map_err(at)?;
        let text = match picture::kind(&first) {
            picture::Kind::Other(name) => return Err(format!("{path} is {name}; deem sends PNG or JPEG: convert it")),
            k => k == picture::Kind::Text,
        };
        kinds.push(text);
        if text {
            opened.push((path, io::Cursor::new(first).chain(f)));
        }
    }
    match (kinds.iter().all(|&t| t), kinds.iter().any(|&t| t)) {
        (true, _) => Ok(Input::Text(open(opened)?)),
        (false, false) => Ok(Input::Pictures(files.to_vec())),
        (false, true) => {
            let first = |text: bool| files[kinds.iter().position(|&t| t == text).unwrap_or(0)].clone();
            Err(format!(
                "{} is a picture and {} is text: a run reads pictures or text; run them apart",
                first(false),
                first(true)
            ))
        }
    }
}

/// stdin, one file, or several read one after another.
fn open(files: Vec<(&String, impl Read + Send + 'static)>) -> Result<Box<dyn BufRead + Send>, String> {
    if files.is_empty() {
        return Ok(Box::new(BufReader::new(io::stdin())));
    }
    let mut all: Box<dyn Read + Send> = Box::new(io::empty());
    for (path, f) in files {
        let mut f = BufReader::new(f);
        skip_bom(&mut f).map_err(|e| format!("{path}: {e}"))?;
        all = Box::new(all.chain(f).chain(&b"\n"[..]));
    }
    Ok(Box::new(BufReader::new(all)))
}

/// A whole file as text, as input is read: UTF-8, a byte-order mark
/// dropped, at most 64 MiB; not a picture.
pub fn text_file(path: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    let f = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    f.take(MAX_RECORD as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("{path}: {e}"))?;
    if bytes.len() > MAX_RECORD {
        return Err(format!("{path} is longer than 64 MiB"));
    }
    if let picture::Kind::Picture(_) = picture::kind(&bytes[..bytes.len().min(64)]) {
        return Err(format!("{path} is a picture; bind it with --img"));
    }
    let bytes = bytes.strip_prefix(BOM).unwrap_or(&bytes);
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn skip_bom(r: &mut dyn BufRead) -> io::Result<()> {
    if r.fill_buf()?.starts_with(BOM) {
        r.consume(BOM.len());
    }
    Ok(())
}

pub struct Reader {
    from: From,
    slurp: bool,
    /// every record goes into one (`-s`, `input: "all"`), so the 64 MiB
    /// limit is on the whole input
    whole: bool,
    done: bool,
    /// records read, for messages
    n: usize,
}

enum From {
    Text(Lines),
    Json {
        values: Box<dyn Iterator<Item = io::Result<J>> + Send>,
        count: Arc<Count>,
    },
    /// pictures' paths
    Pictures(Box<dyn Iterator<Item = Result<String, String>> + Send>),
}

/// What a text record that holds a picture is told.
const IS_PICTURE: &str = "is a picture: pass picture files as arguments, or their paths with -I; the input stops here";

impl Reader {
    pub fn new(src: Box<dyn BufRead + Send>, split: Option<&str>, slurp: bool, json: bool) -> Result<Self, String> {
        Self::with_max(src, split, slurp, json, MAX_RECORD)
    }

    /// With the JSON reader's limit at `max` bytes; tests make it small.
    fn with_max(
        mut src: Box<dyn BufRead + Send>,
        split: Option<&str>,
        slurp: bool,
        json: bool,
        max: usize,
    ) -> Result<Self, String> {
        skip_bom(&mut src).map_err(|e| e.to_string())?;
        let from = if json {
            if split.is_some() {
                return Err("--split cuts text into records; with -J each JSON value is a record".into());
            }
            let count = Arc::new(Count { bytes: AtomicUsize::new(0), lines: AtomicUsize::new(0), max });
            let src = Counted { src, count: count.clone() };
            From::Json { values: Box::new(jaq_json::read::read_many(src)), count }
        } else {
            let split = split.map(|re| Regex::new(&unescape(re)).map_err(|e| format!("--split: {e}"))).transpose()?;
            From::Text(Lines { src, split, buf: Vec::new(), eof: false })
        };
        Ok(Self { from, slurp, whole: slurp, done: false, n: 0 })
    }

    /// The pictures in `input`, one record each: the files, or with `-I`
    /// stdin's lines. Errors if the options are for text.
    pub fn pictures(input: Input, split: Option<&str>, slurp: bool, json: bool) -> Result<Self, String> {
        let flag = [(json, "-J"), (slurp, "-s"), (split.is_some(), "--split")].into_iter().find(|f| f.0);
        let paths: Box<dyn Iterator<Item = Result<String, String>> + Send> = match input {
            Input::Text(_) => unreachable!("text has its own reader"),
            Input::Pictures(files) => {
                if let Some((_, f)) = flag {
                    return Err(format!("{f} reads text, and these files are pictures"));
                }
                Box::new(files.into_iter().map(Ok))
            }
            Input::Paths(src) => {
                if let Some((_, f)) = flag {
                    return Err(format!("{f} reads text, and -I reads pictures' paths"));
                }
                let lines = src.lines().map(|l| l.map_err(|e| e.to_string()));
                Box::new(
                    lines
                        .filter(|l| l.as_ref().map_or(true, |l| !l.trim_end_matches('\r').is_empty()))
                        .map(|l| l.map(|l| l.trim_end_matches('\r').to_owned())),
                )
            }
        };
        Ok(Self { from: From::Pictures(paths), slurp: false, whole: false, done: false, n: 0 })
    }

    pub fn json(&self) -> bool {
        matches!(self.from, From::Json { .. })
    }

    /// Every record as one, for `input: "all"`: `.` is their array. With no
    /// records, it is empty.
    pub fn all(mut self) -> impl Iterator<Item = Result<Raw, String>> + Send {
        self.whole = true;
        let mut me = Some(self);
        std::iter::from_fn(move || me.take().map(Self::gather))
    }

    fn gather(self) -> Result<Raw, String> {
        // the separator `--failed` writes between records
        let sep = match &self.from {
            From::Text(l) if l.split.is_some() => "\0",
            _ => "\n",
        };
        let mut size = 0;
        let mut all = Vec::new();
        for raw in self {
            let raw = raw?;
            if let Raw::Text(s) = &raw {
                size += s.len() + 1;
                if size > MAX_RECORD {
                    return Err(too_long("input \"all\""));
                }
            }
            all.push(raw);
        }
        Ok(Raw::All(all, sep))
    }

    /// The next JSON value, or `None` at the end of the input.
    fn value(&mut self) -> Result<Option<J>, String> {
        let From::Json { values, count } = &mut self.from else { unreachable!() };
        if !self.whole {
            count.bytes.store(0, Relaxed);
        }
        match values.next() {
            None => Ok(None),
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(e)) if e.kind() == io::ErrorKind::InvalidData => {
                let line = count.lines.load(Relaxed) + 1;
                Err(format!("record {} (line {line}) is not JSON: {e}; the input stops here", self.n + 1))
            }
            Some(Err(e)) if e.kind() == io::ErrorKind::FileTooLarge => Err(match self.whole {
                true => too_long(if self.slurp { "-s" } else { "input \"all\"" }),
                false => format!("record {} is a JSON value longer than 64 MiB", self.n + 1),
            }),
            Some(Err(e)) => Err(e.to_string()),
        }
    }
}

fn too_long(what: &str) -> String {
    format!("the input is longer than 64 MiB, the most {what} reads")
}

impl Iterator for Reader {
    type Item = Result<Raw, String>;

    /// The next record, or why the input can't be read.
    fn next(&mut self) -> Option<Result<Raw, String>> {
        if self.done {
            return None;
        }
        self.done |= self.slurp;
        let raw = match (&mut self.from, self.slurp) {
            (From::Text(l), true) => l.all().map(|s| (!s.is_empty()).then_some(Raw::Text(s))),
            (From::Text(l), false) => loop {
                match l.next() {
                    Ok(Some(s)) if s.is_empty() => continue,
                    r => break r.map(|s| s.map(Raw::Text)),
                }
            },
            (From::Json { .. }, true) => {
                let mut vals = Vec::new();
                loop {
                    match self.value() {
                        Ok(Some(v)) => vals.push(v),
                        Ok(None) => break Ok(Some(Raw::Json(vals.into_iter().collect::<J>().to_string()))),
                        Err(e) => break Err(e),
                    }
                }
            }
            (From::Json { .. }, false) => self.value().map(|v| v.map(|v| Raw::Json(v.to_string()))),
            (From::Pictures(paths), _) => paths.next().transpose().map(|p| p.map(Raw::Picture)),
        };
        let raw = raw.map_err(|e| if e == IS_PICTURE { format!("record {} {e}", self.n + 1) } else { e });
        match raw {
            Ok(Some(raw)) => {
                self.n += 1;
                Some(Ok(raw))
            }
            Ok(None) => None,
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

/// Text records: lines, or chunks between `--split` matches.
struct Lines {
    src: Box<dyn BufRead + Send>,
    split: Option<Regex>,
    buf: Vec<u8>,
    /// the source is exhausted (`--split` may still hold records in `buf`)
    eof: bool,
}

impl Lines {
    /// The whole input, for `-s`.
    fn all(&mut self) -> Result<String, String> {
        let mut all = Vec::new();
        (&mut self.src).take(MAX_RECORD as u64 + 1).read_to_end(&mut all).map_err(|e| e.to_string())?;
        if all.len() > MAX_RECORD {
            return Err(too_long("-s"));
        }
        if picture::starts_in(&all) {
            return Err(IS_PICTURE.into());
        }
        let all = String::from_utf8_lossy(&all).into_owned();
        Ok(all.strip_suffix('\n').map(str::to_owned).unwrap_or(all))
    }

    /// The next line, or chunk between separators.
    fn next(&mut self) -> Result<Option<String>, String> {
        let long = || "a record is longer than 64 MiB; split the input with --split, or cut its lines".to_owned();
        let Some(re) = &self.split else {
            let mut line = Vec::new();
            (&mut self.src).take(MAX_RECORD as u64 + 1).read_until(b'\n', &mut line).map_err(|e| e.to_string())?;
            if line.len() > MAX_RECORD {
                return Err(long());
            }
            if line.is_empty() {
                return Ok(None);
            }
            if picture::starts_in(&line) {
                return Err(IS_PICTURE.into());
            }
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            return Ok(Some(String::from_utf8_lossy(line).into_owned()));
        };
        loop {
            let valid = match std::str::from_utf8(&self.buf) {
                Ok(s) => s,
                Err(e) => std::str::from_utf8(&self.buf[..e.valid_up_to()]).unwrap_or_default(),
            };
            let eof = self.eof;
            let m = re.find_iter(valid).find(|m| !m.is_empty() && (m.end() < valid.len() || eof));
            if picture::starts_in(&self.buf) {
                return Err(IS_PICTURE.into());
            }
            if let Some(m) = m {
                let rec = String::from_utf8_lossy(&self.buf[..m.start()]).trim_end_matches('\n').to_owned();
                self.buf.drain(..m.end());
                return Ok(Some(rec));
            }
            if eof {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                let rec = String::from_utf8_lossy(&self.buf).trim_end_matches('\n').to_owned();
                self.buf.clear();
                return Ok(Some(rec));
            }
            if self.buf.len() > MAX_RECORD {
                return Err(long());
            }
            let chunk = self.src.fill_buf().map_err(|e| e.to_string())?;
            let n = chunk.len();
            self.buf.extend_from_slice(chunk);
            self.src.consume(n);
            self.eof = n == 0;
        }
    }
}

/// What the JSON reader has taken: bytes since the last record (or in all,
/// when every record goes into one), and lines, for messages.
struct Count {
    bytes: AtomicUsize,
    lines: AtomicUsize,
    max: usize,
}

/// The input as the JSON reader takes it, counted, and cut off past
/// `Count::max` bytes.
struct Counted {
    src: Box<dyn BufRead + Send>,
    count: Arc<Count>,
}

impl Count {
    fn tally(&self, bytes: &[u8]) -> io::Result<()> {
        self.lines.fetch_add(bytes.iter().filter(|&&b| b == b'\n').count(), Relaxed);
        match self.bytes.fetch_add(bytes.len(), Relaxed) + bytes.len() > self.max {
            true => Err(io::ErrorKind::FileTooLarge.into()),
            false => Ok(()),
        }
    }
}

impl Read for Counted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.src.read(buf)?;
        self.count.tally(&buf[..n])?;
        Ok(n)
    }
}

impl BufRead for Counted {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.src.fill_buf()
    }

    fn consume(&mut self, n: usize) {
        if let Ok(b) = self.src.fill_buf() {
            let _ = self.count.tally(&b[..n.min(b.len())]);
        }
        self.src.consume(n);
    }
}

/// So `--split '\0'` and `--split '\n\n+'` can be typed without shell games.
fn unescape(s: &str) -> String {
    s.replace(r"\0", "\0").replace(r"\t", "\t").replace(r"\r", "\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(input: &[u8], split: Option<&str>, slurp: bool, json: bool) -> Result<Reader, String> {
        // 64 MiB, read a byte at a time in a debug build, is slow
        Reader::with_max(Box::new(io::Cursor::new(input.to_vec())), split, slurp, json, 1000)
    }

    /// Each record's value as JSON, or the error that ended the input.
    fn records(input: &[u8], split: Option<&str>, slurp: bool, json: bool) -> Vec<String> {
        let r = reader(input, split, slurp, json).unwrap();
        r.map(|raw| raw.map_or_else(|e| format!("error: {e}"), |raw| value(raw).to_string())).collect()
    }

    fn value(raw: Raw) -> J {
        let Ok(v) = raw.value(&Pictures::new(), &mut Vec::new()) else { panic!("no value") };
        v
    }

    fn text(input: &[u8]) -> Vec<String> {
        records(input, None, false, false)
    }

    fn json(input: &[u8]) -> Vec<String> {
        records(input, None, false, true)
    }

    #[test]
    fn text_is_lines() {
        assert_eq!(text(b"\n  \na\r\n\nb"), [r#""  ""#, r#""a""#, r#""b""#], "empty lines are skipped, CR dropped");
        assert_eq!(text(b"{\"a\":1}\n[1]\n"), [r#""{\"a\":1}""#, r#""[1]""#], "JSON is text without -J");
        assert_eq!(text(b"\xef\xbb\xbfa\n"), [r#""a""#], "a byte-order mark is dropped");
        assert_eq!(text(b"caf\xe9\n"), ["\"caf\u{fffd}\""], "invalid UTF-8 is replaced");
        assert_eq!(text(b"a\0b\tc\n"), [r#""a\u0000b\tc""#]);
        assert_eq!(text(b"no newline"), [r#""no newline""#]);
    }

    #[test]
    fn json_is_values() {
        assert_eq!(json(b"{\"a\":1}\n{\"a\":2}\n"), [r#"{"a":1}"#, r#"{"a":2}"#], "JSON lines");
        assert_eq!(json(b"{\n  \"a\": 1\n}\n{\n  \"b\": 2\n}\n"), [r#"{"a":1}"#, r#"{"b":2}"#], "pretty-printed");
        assert_eq!(json(b"{\"a\":1} {\"b\":2}"), [r#"{"a":1}"#, r#"{"b":2}"#], "on one line");
        assert_eq!(json(b"[{\"a\":1},\n {\"b\":2}]\n"), [r#"[{"a":1},{"b":2}]"#], "an array is one value");
        assert_eq!(json(b"1 \"s\" null\n"), ["1", r#""s""#, "null"], "any value");
        assert_eq!(json(b"\xef\xbb\xbf{\"a\":1}\n"), [r#"{"a":1}"#], "a byte-order mark is dropped");
        assert_eq!(json(b"\r\n{\"a\":1}\r\n"), [r#"{"a":1}"#]);
        assert_eq!(json(b"\n  \n"), Vec::<String>::new());
    }

    #[test]
    fn broken_json_stops_the_input() {
        let r = json(b"{\"a\":1}\n{\n  \"a\": ]\n}\n{\"b\":2}\n");
        assert_eq!(r[0], r#"{"a":1}"#);
        assert!(r[1].starts_with("error: record 2 (line 3) is not JSON: "), "{}", r[1]);
        assert!(r[1].ends_with("; the input stops here"), "{}", r[1]);
        assert_eq!(r.len(), 2, "nothing after it is read");
        assert!(json(b"hello\n")[0].starts_with("error: record 1 (line 1) is not JSON"));
    }

    #[test]
    fn split_and_slurp() {
        assert_eq!(records(b"one\ntwo\n\nthree\n", Some(r"\n\n+"), false, false), [r#""one\ntwo""#, r#""three""#]);
        assert_eq!(records(b"a\0b\0", Some(r"\0"), false, false), [r#""a""#, r#""b""#]);
        assert_eq!(records(b"a\nb\nc\n", None, true, false), [r#""a\nb\nc""#]);
        assert_eq!(records(b"{\"a\":1}\n{\n\"a\":2}\n", None, true, true), [r#"[{"a":1},{"a":2}]"#]);
        assert_eq!(records(b"", None, true, true), ["[]"]);
        assert!(reader(b"", Some(r"\0"), false, true).is_err_and(|e| e.contains("-J")));
    }

    fn all(input: &[u8], split: Option<&str>, json: bool) -> (String, String) {
        let mut all = reader(input, split, false, json).unwrap().all();
        let raw = all.next().unwrap();
        assert!(all.next().is_none());
        raw.map_or_else(|e| (String::new(), e), |raw| (raw.text(), value(raw).to_string()))
    }

    #[test]
    fn every_record_as_one() {
        assert_eq!(all(b"a\n\nb\n", None, false), ("a\nb".into(), r#"["a","b"]"#.into()));
        assert_eq!(all(b"", None, false), ("".into(), "[]".into()));
        assert_eq!(all(b"x\ny\0z\0", Some(r"\0"), false), ("x\ny\0z".into(), r#"["x\ny","z"]"#.into()));
        assert_eq!(all(b"{\"a\":1}\n{\"a\":\n[2]}\n", None, true).1, r#"[{"a":1},{"a":[2]}]"#);
        assert!(all(b"{\"a\":1}\n{\n", None, true).1.starts_with("record 2 (line 3) is not JSON"));
    }

    // A picture in text input stops it where it starts, as broken JSON does.
    #[test]
    fn a_picture_in_text_stops_the_input() {
        let stop = "error: record 2 is a picture: pass picture files as arguments, or their paths with -I; \
                    the input stops here";
        assert_eq!(text(b"notes\n\xff\xd8\xff\xe0\x00\x10JFIF\nmore\n"), [r#""notes""#, stop]);
        assert_eq!(text(b"notes\n\x89PNG\r\n\x1a\n\x00\x00"), [r#""notes""#, stop]);
        assert_eq!(text(b"\x89PNG\r\n\x1a\n")[0], stop.replace("record 2", "record 1"));
        assert_eq!(
            records(b"a\0\xff\xd8\xff\xe0\0", Some(r"\0"), false, false)[0],
            stop.replace("record 2", "record 1")
        );
        assert!(records(b"a\n\xff\xd8\xff", None, true, false)[0].contains("is a picture"), "-s");
        assert_eq!(text(b"caf\xe9\n"), ["\"caf\u{fffd}\""], "Latin-1 is read as today");
    }

    #[test]
    fn json_is_bounded() {
        let big = format!("\"{}\"\n", "x".repeat(1000));
        let r = json(format!("1\n{big}").as_bytes());
        assert_eq!(r, ["1".to_owned(), "error: record 2 is a JSON value longer than 64 MiB".into()]);
        assert_eq!(json(b"1\n".repeat(1000).as_slice()).len(), 1000, "the limit is per value");
        let many = "1\n".repeat(501);
        let r = records(many.as_bytes(), None, true, true);
        assert_eq!(r, ["error: the input is longer than 64 MiB, the most -s reads"]);
    }
}
