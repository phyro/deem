//! Pictures: PNG and JPEG files named on the command line, known by their
//! first bytes, checked against the API's limits from their headers, and
//! read only when a request that shows one is sent.
//!
//! A program holds a picture as its reference, a string
//! `<image:TOKEN:PATH>`, TOKEN random for each run so that data can't write
//! one. A reference is only ever a whole JSON string: the scanner here finds
//! each in a request's JSON text, and the sender replaces it with the
//! picture's ID, a hash of its bytes.

use crate::transport::api::{ApiError, Asset, Request};
use serde_json::value::RawValue;
use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::hash::{BuildHasher, DefaultHasher, Hash, Hasher};
use std::io::{self, BufReader, Read};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// The API's limits on a picture: each side in pixels, and the file's size.
pub const SIDE: (u32, u32) = (280, 1568);
pub const MAX_BYTES: u64 = 16 << 20;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
const JPEG: &[u8] = b"\xff\xd8\xff";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
}

impl Format {
    /// As the API's `format` and a file part's type name it.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }
}

/// What a file's first bytes say it is.
#[derive(Debug, PartialEq)]
pub enum Kind {
    Picture(Format),
    /// a picture the API doesn't take, by name
    Other(&'static str),
    Text,
}

/// What `first`, a file's first bytes (64 are enough), says it is. A PNG
/// or JPEG signature can't start UTF-8 text, so this is never a guess. Other
/// formats are named only when their first bytes aren't UTF-8 either; else
/// the file is text.
pub fn kind(first: &[u8]) -> Kind {
    if first.starts_with(PNG) {
        return Kind::Picture(Format::Png);
    }
    if first.starts_with(JPEG) {
        return Kind::Picture(Format::Jpeg);
    }
    let text = match std::str::from_utf8(first) {
        Ok(_) => true,
        // a character cut off at the end of `first` is still text
        Err(e) => e.error_len().is_none(),
    };
    let at = |i: usize, s: &[u8]| first.get(i..i + s.len()) == Some(s);
    let other = if at(0, b"GIF8") {
        Some("a GIF")
    } else if at(0, b"RIFF") && at(8, b"WEBP") {
        Some("a WebP")
    } else if [&b"ftypheic"[..], b"ftypheix", b"ftypmif1", b"ftyphevc", b"ftypavif"].iter().any(|s| at(4, s)) {
        Some("a HEIC or AVIF")
    } else {
        None
    };
    match other {
        Some(name) if !text => Kind::Other(name),
        _ => Kind::Text,
    }
}

/// Where in `line`, one line of text input, a picture starts, if one does:
/// a JPEG's signature anywhere (`FF` is never UTF-8), or a PNG's where the
/// line stops being UTF-8 (its signature spans the line's end, `\r\n`).
/// Neither is a guess: no UTF-8 text has either there.
pub fn starts_in(line: &[u8]) -> bool {
    if line.windows(JPEG.len()).any(|w| w == JPEG) {
        return true;
    }
    // each place the line stops being UTF-8
    let mut at = 0;
    while let Err(e) = std::str::from_utf8(&line[at..]) {
        at += e.valid_up_to();
        if line[at..].starts_with(b"\x89PNG") {
            return true;
        }
        at += e.error_len().unwrap_or(line.len() - at);
    }
    false
}

/// A picture: its file, format and size, and once it has been sent, its ID.
#[derive(Clone, Debug)]
pub struct Picture {
    pub path: String,
    pub reference: String,
    /// `--img NAME`, for a picture the run holds throughout
    pub img: Option<String>,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    id: OnceLock<String>,
}

impl Picture {
    /// The tokens it costs each time a request shows it (measured on
    /// `jev-latest`, 2026-10-16: about width × height / 390).
    pub fn tokens(&self) -> u64 {
        (u64::from(self.width) * u64::from(self.height)).div_ceil(390)
    }

    /// Its ID in requests: 128 bits of a hash of the file's bytes, as a
    /// UUID. Read once, the first time it is needed.
    pub fn id(&self) -> Result<String, String> {
        if let Some(id) = self.id.get() {
            return Ok(id.clone());
        }
        let file = File::open(&self.path).map_err(|e| format!("{}: {e}", self.path))?;
        let mut r = BufReader::new(file);
        let (mut a, mut b) = (DefaultHasher::new(), DefaultHasher::new());
        (0u8).hash(&mut a);
        (1u8).hash(&mut b);
        let mut buf = vec![0; 1 << 16];
        loop {
            let n = r.read(&mut buf).map_err(|e| format!("{}: {e}", self.path))?;
            if n == 0 {
                break;
            }
            a.write(&buf[..n]);
            b.write(&buf[..n]);
        }
        Ok(self.id.get_or_init(|| uuid(a.finish(), b.finish())).clone())
    }

    /// Check the file again before it is sent: it may have changed since its
    /// header was read.
    pub fn check(&self) -> Result<(), String> {
        let again = header(&self.path)?;
        match (again.0, again.1, again.2) == (self.format, self.width, self.height) {
            true => Ok(()),
            false => Err(format!("{} changed after deem read it", self.path)),
        }
    }
}

fn uuid(a: u64, b: u64) -> String {
    let x = (u128::from(a) << 64) | u128::from(b);
    let h = format!("{x:032x}");
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// A picture file's format and size in pixels, checked against the API's
/// limits.
pub fn header(path: &str) -> Result<(Format, u32, u32), String> {
    let at = |e: io::Error| format!("{path}: {e}");
    let file = File::open(path).map_err(at)?;
    let len = file.metadata().map_err(at)?.len();
    let mut r = BufReader::new(file);
    let mut first = [0; 8];
    r.read_exact(&mut first[..3]).map_err(|_| format!("{path} is not a PNG or JPEG"))?;
    let (format, (w, h)) = if first[..3] == JPEG[..] {
        (Format::Jpeg, jpeg_size(&mut r).ok_or_else(|| format!("{path}: no size in the JPEG"))?)
    } else {
        r.read_exact(&mut first[3..]).map_err(|_| format!("{path} is not a PNG or JPEG"))?;
        if first != PNG {
            return Err(format!("{path} is not a PNG or JPEG"));
        }
        (Format::Png, png_size(&mut r).ok_or_else(|| format!("{path}: no size in the PNG"))?)
    };
    let (min, max) = SIDE;
    if !(min..=max).contains(&w) || !(min..=max).contains(&h) {
        // `>` only shrinks, and `^<` only enlarges, until the short side fits
        let (short, long) = (u64::from(w.min(h)), u64::from(w.max(h)));
        let fix = if long * u64::from(min) > u64::from(max) * short {
            "crop it: it is too narrow for any size to fit".to_owned()
        } else if short < u64::from(min) {
            format!("enlarge it, e.g. magick {path} -resize '{min}x{min}^<' …")
        } else {
            format!("shrink it, e.g. magick {path} -resize '{max}x{max}>' …")
        };
        return Err(format!("{path} is {w}×{h}; the API takes {min} to {max} pixels a side ({fix})"));
    }
    if len > MAX_BYTES {
        return Err(format!("{path} is {len} bytes; the API takes at most 16 MiB"));
    }
    Ok((format, w, h))
}

/// After the signature: the `IHDR` chunk's width and height.
fn png_size(r: &mut impl Read) -> Option<(u32, u32)> {
    let mut b = [0; 16];
    r.read_exact(&mut b).ok()?;
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    (&b[4..8] == b"IHDR").then(|| (be(8), be(12)))
}

/// After `FF D8 FF`: the first frame header's (`SOF0`–`SOF15`, but not
/// `C4`, `C8`, `CC`) width and height, skipping the segments before it.
fn jpeg_size(r: &mut impl Read) -> Option<(u32, u32)> {
    let mut byte = || {
        let mut b = [0];
        r.read_exact(&mut b).ok().map(|_| b[0])
    };
    // the third byte of the signature was the first marker's FF
    let mut marker = byte()?;
    loop {
        while marker == 0xff {
            marker = byte()?;
        }
        match marker {
            0x01 | 0xd0..=0xd7 => {}
            0xd9 | 0xda => return None,
            _ => {
                let len = (u16::from(byte()?) << 8 | u16::from(byte()?)).checked_sub(2)?;
                if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                    let _precision = byte()?;
                    let h = u16::from(byte()?) << 8 | u16::from(byte()?);
                    let w = u16::from(byte()?) << 8 | u16::from(byte()?);
                    return Some((u32::from(w), u32::from(h)));
                }
                for _ in 0..len {
                    byte()?;
                }
            }
        }
        if byte()? != 0xff {
            return None;
        }
        marker = byte()?;
    }
}

/// The run's pictures: each by its reference. An input picture's entry
/// lives while its record (or a request) holds it; `--img` pictures, for
/// the run.
pub struct Pictures {
    token: String,
    held: Mutex<HashMap<String, Weak<Picture>>>,
}

impl Default for Pictures {
    fn default() -> Self {
        Self::new()
    }
}

impl Pictures {
    /// A table with a fresh token: 128 bits from the OS's randomness, through
    /// the standard library's randomly keyed hasher.
    pub fn new() -> Self {
        let random = || RandomState::new().hash_one(std::process::id());
        let token = format!("{:016x}{:016x}", random(), random());
        Self { token, held: Mutex::default() }
    }

    /// What starts every reference this run makes.
    pub fn prefix(&self) -> String {
        format!("<image:{}:", self.token)
    }

    /// Read `path`'s header and give it a reference. The caller holds the
    /// picture; the table forgets it once nothing does. A path named twice
    /// while held is one picture, as its reference is one: an input picture
    /// that is also an `--img` one is the `--img` one.
    pub fn add(&self, path: &str) -> Result<Arc<Picture>, String> {
        self.add_as(path, None)
    }

    /// `add`, for `--img NAME`.
    pub fn add_as(&self, path: &str, img: Option<&str>) -> Result<Arc<Picture>, String> {
        let reference = format!("{}{path}>", self.prefix());
        if let Some(p) = self.get(&reference) {
            return Ok(p);
        }
        let (format, width, height) = header(path)?;
        let img = img.map(str::to_owned);
        let p = Arc::new(Picture { path: path.to_owned(), reference, img, format, width, height, id: OnceLock::new() });
        let mut held = crate::lock(&self.held);
        // forget the pictures nothing holds, now and then
        if held.len() >= 64 && held.len().is_power_of_two() {
            held.retain(|_, w| w.strong_count() > 0);
        }
        held.insert(p.reference.clone(), Arc::downgrade(&p));
        Ok(p)
    }

    /// The picture a reference names, if something still holds it.
    pub fn get(&self, reference: &str) -> Option<Arc<Picture>> {
        crate::lock(&self.held).get(reference).and_then(Weak::upgrade)
    }

    /// Print each reference in `out`, printed text or JSON, as its
    /// picture's path: the token is deem's, and never shown.
    pub fn show(&self, out: &mut Vec<u8>) {
        let prefix = self.prefix();
        let find = |b: &[u8], from: usize| b[from..].windows(prefix.len()).position(|w| w == prefix.as_bytes());
        if find(out, 0).is_none() {
            return;
        }
        let (old, mut at) = (std::mem::take(out), 0);
        while let Some(i) = find(&old, at).map(|i| i + at) {
            out.extend_from_slice(&old[at..i]);
            // the reference ends at a `>`: the first that makes one deem knows
            let ends = (i..old.len().min(i + prefix.len() + 4096)).filter(|&j| old[j] == b'>');
            let shown = ends.into_iter().find_map(|j| {
                let text = std::str::from_utf8(&old[i..=j]).ok()?;
                if let Some(p) = self.get(text) {
                    return Some((j, p.path.clone()));
                }
                // inside JSON: escaped as JSON
                let decoded: String = serde_json::from_str(&format!("\"{text}\"")).ok()?;
                let p = self.get(&decoded)?;
                let path = serde_json::to_string(&p.path).ok()?;
                Some((j, path[1..path.len() - 1].to_owned()))
            });
            match shown {
                Some((j, path)) => {
                    out.extend_from_slice(path.as_bytes());
                    at = j + 1;
                }
                None => {
                    out.extend_from_slice(prefix.as_bytes());
                    at = i + prefix.len();
                }
            }
        }
        out.extend_from_slice(&old[at..]);
    }

    /// `text` with its references shown as paths.
    pub fn shown(&self, text: &str) -> String {
        let mut b = text.as_bytes().to_vec();
        self.show(&mut b);
        String::from_utf8(b).unwrap_or_else(|_| text.to_owned())
    }

    /// How many pictures something still holds.
    #[cfg(test)]
    fn len(&self) -> usize {
        crate::lock(&self.held).values().filter(|w| w.strong_count() > 0).count()
    }
}

/// The most tokens the API reads of the state and any one question, and of a
/// whole request.
pub const MAX_TOKENS: (u64, u64) = (32_000, 64_000);

/// The pictures `req` shows, each once, from the table; and the request's
/// tokens, its text at `bytes_per_token` and each picture once for every
/// place it is shown. Errors if a part is over the API's limit, before
/// anything is sent.
pub fn resolve(req: &Request, table: &Pictures, bytes_per_token: f64) -> Result<(Vec<Arc<Picture>>, u64), ApiError> {
    let prefix = table.prefix();
    let mut held: Vec<Arc<Picture>> = Vec::new();
    // each part's tokens: the state, then each question
    let mut part = |json: &str| -> Result<(u64, Vec<String>), ApiError> {
        let mut tokens = (json.len() as f64 / bytes_per_token) as u64;
        let mut shown = Vec::new();
        for f in scan(json, &prefix).into_iter().flatten() {
            let Some(p) = table.get(&f.reference) else {
                return Err(ApiError::Picture(format!("deem no longer holds the picture {}", f.reference), false));
            };
            tokens += p.tokens();
            shown.push(p.path.clone());
            if !held.iter().any(|h| Arc::ptr_eq(h, &p)) {
                held.push(p);
            }
        }
        Ok((tokens, shown))
    };
    let (state, mut shown) = part(req.state.get())?;
    let mut questions = Vec::new();
    for q in req.questions.values() {
        let (t, s) = part(q.get())?;
        questions.push(t);
        shown.extend(s);
    }
    let longest = questions.iter().copied().max().unwrap_or(0);
    let all = state + questions.iter().sum::<u64>();
    let (one, max) = MAX_TOKENS;
    if !held.is_empty() && (state + longest > one || all > max) {
        shown.sort();
        shown.dedup();
        return Err(ApiError::Picture(
            format!(
                "the request is about {} tokens with its pictures ({}), over the API's {one} for the value and a \
                 question, or {max} in all: use smaller pictures, or fewer in one question",
                (state + longest).max(all),
                shown.join(", ")
            ),
            false,
        ));
    }
    Ok((held, all))
}

/// `req` as sent: each picture checked again and given its ID, which
/// replaces its reference everywhere, and an asset for each. Without
/// pictures, `req` as it is.
pub fn prepare(req: &Request) -> Result<Request, ApiError> {
    if req.pictures.is_empty() {
        return Ok(req.clone());
    }
    let mut ids = HashMap::new();
    let mut assets = BTreeMap::new();
    for p in &req.pictures {
        // an `--img` picture is in every request that uses it
        let fail = |e: String| match &p.img {
            Some(name) => ApiError::Picture(format!("--img {name}: {e}"), true),
            None => ApiError::Picture(e, false),
        };
        p.check().map_err(fail)?;
        let id = p.id().map_err(fail)?;
        let path = p.path.clone();
        assets.insert(id.clone(), Asset { kind: "image", part: id.clone(), path, mime: p.format.mime() });
        ids.insert(p.reference.clone(), id);
    }
    let swap = |json: &RawValue| {
        let text = json.get();
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for f in scan(text, "<image:").into_iter().flatten() {
            if let Some(id) = ids.get(&f.reference) {
                out.push_str(&text[at..f.start]);
                out.push_str(&format!("\"{id}\""));
                at = f.end;
            }
        }
        out.push_str(&text[at..]);
        RawValue::from_string(out).expect("a string for a string is still JSON")
    };
    // a reference that could be text: the ID is in the request already
    let before = req.body();
    if let Some(id) = ids.values().find(|id| before.contains(id.as_str())) {
        return Err(ApiError::Picture(format!("the picture ID {id} is already in the request's text"), false));
    }
    let state = swap(&req.state);
    let questions = req.questions.iter().map(|(k, q)| (k.clone(), swap(q))).collect();
    Ok(Request { model: req.model.clone(), state, questions, assets, pictures: Vec::new() })
}

/// A reference found in JSON text: the span of its JSON string, quotes
/// included, and the string.
#[derive(Debug, PartialEq)]
pub struct Found {
    pub start: usize,
    pub end: usize,
    pub reference: String,
}

/// Every place `prefix` occurs in `json`, the JSON text of one value: each
/// a whole string (`Ok`), or text inside a longer string or an object's key
/// (`Err`, with that string).
pub fn scan(json: &str, prefix: &str) -> Vec<Result<Found, String>> {
    let b = json.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = json[from..].find(prefix).map(|i| i + from) {
        // the string it is in: back to the quote that opens it
        let start = (0..i).rev().find(|&k| b[k] == b'"' && escaped(b, k).is_none_or(|e| !e)).unwrap_or(0);
        let end = (i..b.len()).find(|&k| b[k] == b'"' && !escaped(b, k).unwrap_or(false)).map_or(b.len(), |k| k + 1);
        let text: String = serde_json::from_str(&json[start..end]).unwrap_or_else(|_| json[start..end].to_owned());
        let key = json[end..].trim_start().starts_with(':');
        out.push(match start + 1 == i && !key {
            true => Ok(Found { start, end, reference: text }),
            false => Err(text),
        });
        from = end.max(i + prefix.len());
    }
    out
}

/// Whether the quote at `k` is escaped: preceded by an odd number of
/// backslashes. `None` at the start.
fn escaped(b: &[u8], k: usize) -> Option<bool> {
    (k > 0).then(|| b[..k].iter().rev().take_while(|&&c| c == b'\\').count() % 2 == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = PNG.to_vec();
        b.extend([0, 0, 0, 13]);
        b.extend(b"IHDR");
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0]);
        b
    }

    fn jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut b = vec![0xff, 0xd8];
        // an APP1 (EXIF) segment, then a Huffman table (C4, not a frame)
        b.extend([0xff, 0xe1, 0, 6, b'E', b'x', 0, 0]);
        b.extend([0xff, 0xc4, 0, 4, 0, 0]);
        b.extend([0xff, 0xc2, 0, 11, 8]);
        b.extend(h.to_be_bytes());
        b.extend(w.to_be_bytes());
        b.extend([3, 1, 0x11, 0]);
        b
    }

    fn file(name: &str, bytes: &[u8]) -> String {
        let dir = std::env::temp_dir().join(format!("deem-picture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p.to_str().unwrap().to_owned()
    }

    #[test]
    fn kinds() {
        assert_eq!(kind(&png(400, 300)), Kind::Picture(Format::Png));
        assert_eq!(kind(&jpeg(400, 300)), Kind::Picture(Format::Jpeg));
        assert_eq!(kind(b"GIF89a\x90\x01\x2c\x01\xf7\x00\x00"), Kind::Other("a GIF"));
        assert_eq!(kind(b"GIF89a is how a GIF starts\n"), Kind::Text, "text that looks like a GIF is text");
        assert_eq!(kind(b"RIFF\xfe\xff\x00\x00WEBPVP8 "), Kind::Other("a WebP"));
        assert_eq!(kind(b"hello\n"), Kind::Text);
        assert_eq!(kind(b"caf\xe9\n"), Kind::Text, "Latin-1 is text, read as today");
        assert_eq!(kind("na\u{ef}ve".as_bytes()), Kind::Text);
        assert_eq!(kind(b""), Kind::Text);
    }

    #[test]
    fn pictures_in_lines() {
        assert!(starts_in(b"\xff\xd8\xff\xe0\x00\x10JFIF"));
        assert!(starts_in(b"some text then \xff\xd8\xff\xe0"));
        assert!(starts_in(b"\x89PNG\r"));
        assert!(starts_in(b"notes\x89PNG\r"), "a PNG after text without a newline");
        assert!(!starts_in(b"caf\xe9 au lait"));
        assert!(!starts_in("\u{89}PNG\r".as_bytes()), "U+0089 then PNG is text");
        assert!(!starts_in(b"PNG and JPEG"));
    }

    #[test]
    fn headers() {
        assert_eq!(header(&file("a.png", &png(800, 600))), Ok((Format::Png, 800, 600)));
        assert_eq!(header(&file("a.jpg", &jpeg(1568, 1176))), Ok((Format::Jpeg, 1568, 1176)));
        let small = file("small.png", &png(200, 200));
        assert!(header(&small).unwrap_err().contains("is 200×200; the API takes 280 to 1568 pixels a side"));
        assert!(header(&file("big.jpg", &jpeg(2000, 1000))).unwrap_err().contains("is 2000×1000"));
        // the fix the message suggests: shrink, enlarge, or crop
        let fix = |w, h| header(&file("fix.png", &png(w, h))).unwrap_err();
        assert!(fix(2000, 1000).ends_with("-resize '1568x1568>' …)"), "{}", fix(2000, 1000));
        assert!(fix(200, 100).ends_with("-resize '280x280^<' …)"), "{}", fix(200, 100));
        assert!(fix(100, 500).ends_with("-resize '280x280^<' …)"), "1:5 enlarges to 280×1400");
        assert!(fix(300, 2000).ends_with("(crop it: it is too narrow for any size to fit)"));
        assert!(header(&file("t.txt", b"hello\n")).unwrap_err().ends_with("is not a PNG or JPEG"));
        assert!(header(&file("cut.png", &png(800, 600)[..12])).unwrap_err().contains("no size in the PNG"));
        let mut no_frame = jpeg(800, 600);
        no_frame.truncate(14);
        no_frame.extend([0xff, 0xda]);
        assert!(header(&file("noframe.jpg", &no_frame)).unwrap_err().contains("no size in the JPEG"));
        assert!(header("/nonexistent/x.png").is_err());
    }

    #[test]
    fn ids_are_hashes_of_the_bytes() {
        let t = Pictures::new();
        let a = t.add(&file("id1.png", &png(800, 600))).unwrap();
        let b = t.add(&file("id2.png", &png(800, 600))).unwrap();
        let c = t.add(&file("id3.png", &png(801, 600))).unwrap();
        let id = a.id().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
        assert_eq!(id, b.id().unwrap(), "the same bytes, the same ID");
        assert_ne!(id, c.id().unwrap());
        assert_eq!(a.tokens(), 1231);
        assert_ne!(Pictures::new().prefix(), t.prefix(), "a token for each run");
    }

    #[test]
    fn the_table_forgets_what_nothing_holds() {
        let t = Pictures::new();
        let path = file("held.png", &png(400, 400));
        let held = t.add(&path).unwrap();
        assert!(held.reference.starts_with(&t.prefix()) && held.reference.ends_with(&format!("{path}>")));
        assert!(Arc::ptr_eq(&held, &t.add(&path).unwrap()), "one picture while held");
        for i in 0..200 {
            let p = file(&format!("many{i}.png"), &png(400, 400));
            t.add(&p).unwrap();
        }
        assert_eq!(t.len(), 1);
        assert!(crate::lock(&t.held).len() < 130, "swept as it grows");
        assert_eq!(t.get(&held.reference).map(|p| p.width), Some(400));
        drop(held);
        assert_eq!(t.len(), 0);
    }

    // A file that is an `--img` and an input picture too has one reference,
    // so it is one picture: the input's record ending doesn't take it from
    // the run, which holds it throughout.
    #[test]
    fn an_img_that_is_also_input_is_one_picture() {
        let t = Pictures::new();
        let path = file("both.png", &png(400, 400));
        let img = t.add_as(&path, Some("baseline")).unwrap();
        let input = t.add(&path).unwrap();
        assert!(Arc::ptr_eq(&img, &input));
        drop(input);
        assert!(t.get(&img.reference).is_some(), "still held by the run");
    }

    #[test]
    fn references_print_as_paths() {
        let t = Pictures::new();
        let p = t.add(&file("show \"me\".png", &png(400, 400))).unwrap();
        let path = p.path.clone();
        let show = |s: &str| {
            let mut b = s.as_bytes().to_vec();
            t.show(&mut b);
            String::from_utf8(b).unwrap()
        };
        assert_eq!(show(&format!("{}\n", p.reference)), format!("{path}\n"));
        assert_eq!(show(&format!("0.9\t{}\n", p.reference)), format!("0.9\t{path}\n"));
        let json = serde_json::to_string(&serde_json::json!({"photo": p.reference})).unwrap();
        let want = serde_json::to_string(&serde_json::json!({"photo": path})).unwrap();
        assert_eq!(show(&json), want, "escaped inside JSON");
        let forged = format!("{}elsewhere.png>", t.prefix());
        assert_eq!(show(&forged), forged, "only references to pictures deem holds");
        assert_eq!(show("plain"), "plain");
    }

    // A picture is read again when its request is sent: one that is gone
    // fails its request; an `--img` one, every request, so the run.
    #[test]
    fn prepared_as_sent() {
        use crate::transport::api::Kind;
        let t = Pictures::new();
        let path = file("prep.png", &png(800, 600));
        let p = t.add(&path).unwrap();
        let state =
            RawValue::from_string(format!(r#"{{"photo":{}}}"#, serde_json::to_string(&p.reference).unwrap())).unwrap();
        let req = Request {
            model: "m".into(),
            state,
            questions: BTreeMap::new(),
            assets: BTreeMap::new(),
            pictures: vec![p.clone()],
        };
        let (shown, tokens) = resolve(&req, &t, 2.0).unwrap();
        assert_eq!((shown.len(), tokens > p.tokens()), (1, true));
        let sent = prepare(&req).unwrap();
        let id = p.id().unwrap();
        assert_eq!(sent.state.get(), format!(r#"{{"photo":"{id}"}}"#));
        assert_eq!(sent.assets[&id].path, path);
        std::fs::remove_file(&path).unwrap();
        let fresh = |img: Option<&str>| {
            let q = Arc::new(Picture { img: img.map(str::to_owned), id: OnceLock::new(), ..(*p).clone() });
            Request { pictures: vec![q], ..req.clone() }
        };
        assert_eq!(prepare(&fresh(None)).unwrap_err().kind(), Kind::Refused);
        assert_eq!(prepare(&fresh(Some("badge"))).unwrap_err().kind(), Kind::BadPicture);
        let header = ApiError::Status(400, "Images are experimental. Send the header …".into());
        assert!(header.kind().setup(), "a missing header stops the run");
    }

    #[test]
    fn scanning() {
        let p = "<image:t0:";
        let r = |s: &str| s.to_owned();
        let whole = |json: &str| -> Vec<Result<String, String>> {
            scan(json, p).into_iter().map(|f| f.map(|f| json[f.start..f.end].to_owned())).collect()
        };
        assert_eq!(whole(r#""<image:t0:a.png>""#), [Ok(r(r#""<image:t0:a.png>""#))]);
        assert_eq!(
            whole(r#"{"photo":"<image:t0:a.png>","n":["<image:t0:b \"c\".png>"]}"#),
            [Ok(r(r#""<image:t0:a.png>""#)), Ok(r(r#""<image:t0:b \"c\".png>""#))]
        );
        assert_eq!(whole(r#""is the <image:t0:x.png> friendly?""#), [Err(r("is the <image:t0:x.png> friendly?"))]);
        assert_eq!(whole(r#"{"<image:t0:x.png>":1}"#), [Err(r("<image:t0:x.png>"))], "a key");
        assert_eq!(whole(r#""say \"<image:t0:x.png>\"""#), [Err(r(r#"say "<image:t0:x.png>""#))]);
        assert_eq!(whole(r#""<image:t1:x.png>""#), [], "another run's token");
        let found = scan(r#"["<image:t0:café.png>"]"#, p);
        assert_eq!(found[0].as_ref().unwrap().reference, "<image:t0:café.png>", "escapes are decoded");
    }
}
