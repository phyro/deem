//! The output: compact JSON as jaq prints it, and strings raw for text
//! input or `-r`.

use jaq_json::{Num, Val as J};

/// What jaq prints that isn't JSON: it prints a value as it is rather than
/// change it into something JSON can hold.
pub const NOT_JSON: &str = "NaN, Infinity, a non-string object key, or a byte string";
use std::io::{self, BufWriter, Write};

/// One record's outputs, rendered on its own thread.
#[derive(Default)]
pub struct Chunk {
    pub bytes: Vec<u8>,
    pub n: u64,
    /// For `-e`: whether the last output was `false` or `null`.
    pub last_falsy: bool,
    /// Whether an output isn't JSON (`NOT_JSON`).
    pub not_json: bool,
}

impl Chunk {
    pub fn push(&mut self, v: &J, raw: bool) {
        match v {
            J::TStr(s) | J::BStr(s) if raw => self.bytes.extend(String::from_utf8_lossy(s).as_bytes()),
            v => {
                self.not_json |= !is_json(v);
                let _ = jaq_json::write::write(&mut self.bytes, &Default::default(), 0, v);
            }
        }
        self.bytes.push(b'\n');
        self.n += 1;
        self.last_falsy = matches!(v, J::Null | J::Bool(false));
    }
}

pub struct Output<W: Write> {
    w: BufWriter<W>,
    pub count: u64,
    pub last_falsy: bool,
}

impl<W: Write> Output<W> {
    pub fn new(w: W) -> Self {
        Self { w: BufWriter::new(w), count: 0, last_falsy: false }
    }

    pub fn write(&mut self, c: &Chunk) -> io::Result<()> {
        if c.n > 0 {
            self.count += c.n;
            self.last_falsy = c.last_falsy;
        }
        self.w.write_all(&c.bytes)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

/// Whether jaq prints `v` as JSON.
pub fn is_json(v: &J) -> bool {
    match v {
        J::Num(Num::Float(f)) => f.is_finite(),
        J::BStr(_) => false,
        J::Arr(a) => a.iter().all(is_json),
        J::Obj(o) => o.iter().all(|(k, v)| matches!(k, J::TStr(_)) && is_json(v)),
        _ => true,
    }
}
