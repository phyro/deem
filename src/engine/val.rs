//! `Val`: a jq value, or an answer this run doesn't have.
//!
//! jaq-core touches values only through `ValT`, so a value can be
//! `Unknown`: the answer to a question this run of the record has queued,
//! or anything computed from one. Within a run an answer never arrives, so
//! an unknown value stays unknown. Methods that only move a value around
//! (building an object or array, `.key`, `tostring`, `|=` over children,
//! arithmetic) give `Unknown` again. Methods that must look inside it
//! (comparison, truthiness, iteration, every native) stop the run: it is
//! abandoned, its questions are sent, and the record runs again from the
//! start once they are answered.
//!
//! Looking at an unknown value always stops the run; it is never a jq
//! error, which `try`, `?` or `//` could catch and take a branch the run
//! with the answer wouldn't.

use core::cmp::Ordering;
use core::fmt;
use jaq_core::box_iter::BoxIter;
use jaq_core::path::Opt;
use jaq_core::{Exn, ValR, ValX, val::Range};
use std::cell::{Cell, RefCell};

pub type J = jaq_json::Val;
type Error = jaq_core::Error<Val>;

#[derive(Clone)]
pub enum Val {
    Ready(J),
    /// The answer to question `.0`, or something computed from it.
    Unknown(u64),
}

/// Why a run ends early, raised via `panic_any` and caught where the run
/// started (`Program::step`).
pub enum Interrupt {
    /// The run needs answers it doesn't have: the one to this question, if
    /// it looked at one.
    Rerun(Option<u64>),
    /// The run as a whole is stopping: the record ends.
    Stopped,
    /// A rerun didn't do what the run before it did: a bug in deem.
    Diverged(&'static str),
}

pub fn interrupt(i: Interrupt) -> ! {
    std::panic::panic_any(i)
}

impl Val {
    /// The value; an unknown one stops the run.
    pub fn force(&self) -> &J {
        match self {
            Self::Ready(j) => j,
            Self::Unknown(q) => interrupt(Interrupt::Rerun(Some(*q))),
        }
    }

    pub fn owned(self) -> J {
        match self {
            Self::Ready(j) => j,
            u => u.force().clone(),
        }
    }

    /// The question an unknown value waits on.
    fn unknown(&self) -> Option<u64> {
        match self {
            Self::Ready(_) => None,
            Self::Unknown(q) => Some(*q),
        }
    }

    /// An array, unknown if any element is.
    fn arr(xs: Vec<Val>) -> Self {
        match xs.iter().find_map(Val::unknown) {
            Some(q) => Self::Unknown(q),
            None => Self::Ready(xs.into_iter().map(Val::owned).collect()),
        }
    }

    /// An object, unknown if any key or value is.
    fn obj(kvs: Vec<(Val, Val)>) -> Self {
        match kvs.iter().find_map(|(k, v)| k.unknown().or(v.unknown())) {
            Some(q) => Self::Unknown(q),
            None => Self::Ready(J::obj(kvs.into_iter().map(|(k, v)| (k.owned(), v.owned())).collect())),
        }
    }
}

/// jaq errors on `J` become errors on `Val` with the same message.
fn lift(e: jaq_core::Error<J>) -> Error {
    Error::new(Val::Ready(e.into_val()))
}

pub fn liftr(r: ValR<J>) -> ValR<Val> {
    r.map(Val::Ready).map_err(lift)
}

/// jaq's own update on a ready value, with `f` on deem's values. Where `f`
/// gives an unknown value, if `lazy`, jaq gets `null` in its place and the
/// update goes on, so `.[] |= ask(q)` queues every element's question; the
/// result is unknown. Where jaq needs to look inside `f`'s outputs (`lazy`
/// false), an unknown one stops the run. An exception from `f` can't cross
/// value types, so it waits beside the update and is raised after it.
fn update<'a, I: Iterator<Item = ValX<'a, Val>>>(
    lazy: bool,
    f: impl Fn(Val) -> I,
    run: impl FnOnce(&dyn Fn(J) -> std::vec::IntoIter<ValX<'a, J>>) -> ValX<'a, J>,
) -> ValX<'a, Val> {
    let (held, unknown) = (RefCell::new(None), Cell::new(None));
    let g = |x: J| {
        let mut out = Vec::new();
        for y in f(Val::Ready(x)) {
            match y {
                Ok(Val::Ready(j)) => out.push(Ok(j)),
                Ok(u) if lazy => {
                    unknown.set(unknown.get().or(u.unknown()));
                    out.push(Ok(J::Null));
                }
                Ok(u) => out.push(Ok(u.owned())),
                Err(e) => {
                    *held.borrow_mut() = Some(e);
                    out.push(Err(Exn::from(jaq_core::Error::new(J::Null))));
                    break;
                }
            }
        }
        out.into_iter()
    };
    let r = run(&g);
    if let Some(e) = held.take() {
        return Err(e);
    }
    let j = r.map_err(|e| match e.get_err() {
        Ok(e) => Exn::from(lift(e)),
        Err(_) => Exn::from(Error::str("unexpected exception in an update")),
    })?;
    Ok(unknown.get().map_or(Val::Ready(j), Val::Unknown))
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.force().fmt(f)
    }
}

macro_rules! from {
    ($($t:ty),*) => {$(
        impl From<$t> for Val {
            fn from(x: $t) -> Self {
                Self::Ready(x.into())
            }
        }
    )*};
}
from!(bool, isize, usize, f64, String);

impl From<Range<Val>> for Val {
    fn from(r: Range<Val>) -> Self {
        let kv = |k: &str, v: Option<Val>| v.map(|v| (Val::from(k.to_owned()), v));
        Self::obj(kv("start", r.start).into_iter().chain(kv("end", r.end)).collect())
    }
}

impl FromIterator<Val> for Val {
    fn from_iter<T: IntoIterator<Item = Val>>(iter: T) -> Self {
        Self::arr(iter.into_iter().collect())
    }
}

impl PartialEq for Val {
    fn eq(&self, other: &Self) -> bool {
        self.force() == other.force()
    }
}
impl Eq for Val {}
impl PartialOrd for Val {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Val {
    fn cmp(&self, other: &Self) -> Ordering {
        self.force().cmp(other.force())
    }
}

// Arithmetic on an unknown value is unknown, not a look. jaq builds
// `{a: x, b: y}` as `{a: x} + {b: y}`, so a `+` that looked would stop the
// run as soon as the object is built, and `[.[] | {k: ., v: ask(q)}]` would
// cost one wave per element.
macro_rules! binop {
    ($tr:ident, $f:ident) => {
        impl core::ops::$tr for Val {
            type Output = ValR<Val>;
            fn $f(self, rhs: Self) -> ValR<Val> {
                match (self, rhs) {
                    (Self::Ready(l), Self::Ready(r)) => liftr(core::ops::$tr::$f(l, r)),
                    (l, r) => Ok(Self::Unknown(l.unknown().or(r.unknown()).unwrap_or_default())),
                }
            }
        }
    };
}
binop!(Add, add);
binop!(Sub, sub);
binop!(Mul, mul);
binop!(Div, div);
binop!(Rem, rem);

impl core::ops::Neg for Val {
    type Output = ValR<Val>;
    fn neg(self) -> ValR<Val> {
        match self {
            Self::Ready(j) => liftr(-j),
            u => Ok(u),
        }
    }
}

impl jaq_core::ValT for Val {
    fn from_num(n: &str) -> ValR<Self> {
        liftr(J::from_num(n))
    }

    fn from_map<I: IntoIterator<Item = (Self, Self)>>(iter: I) -> ValR<Self> {
        Ok(Self::obj(iter.into_iter().collect()))
    }

    fn key_values(self) -> BoxIter<'static, ValR<(Self, Self), Self>> {
        Box::new(self.owned().key_values().map(|kv| kv.map(|(k, v)| (Val::Ready(k), Val::Ready(v))).map_err(lift)))
    }

    fn values(self) -> Box<dyn Iterator<Item = ValR<Self>>> {
        Box::new(self.owned().values().map(liftr))
    }

    fn index(self, index: &Self) -> ValR<Self> {
        match (self, index) {
            (Self::Ready(j), Self::Ready(i)) => liftr(j.index(i)),
            // `.value` of an answer this run doesn't have: still unknown
            (v, i) => Ok(Self::Unknown(v.unknown().or(i.unknown()).unwrap_or_default())),
        }
    }

    fn range(self, range: Range<&Self>) -> ValR<Self> {
        let (s, e) = (range.start.map(Val::force), range.end.map(Val::force));
        liftr(self.owned().range(s..e))
    }

    fn map_values<'a, I: Iterator<Item = ValX<'a, Self>>>(self, opt: Opt, f: impl Fn(Self) -> I) -> ValX<'a, Self> {
        let j = self.owned();
        update(true, f, |g| j.map_values(opt, g))
    }

    fn map_index<'a, I: Iterator<Item = ValX<'a, Self>>>(
        self,
        index: &Self,
        opt: Opt,
        f: impl Fn(Self) -> I,
    ) -> ValX<'a, Self> {
        let (j, i) = (self.owned(), index.force().clone());
        // a range (`.[1:3]`) is updated with an array jaq looks inside
        let lazy = !matches!(i, J::Obj(_));
        update(lazy, f, |g| j.map_index(&i, opt, g))
    }

    fn map_range<'a, I: Iterator<Item = ValX<'a, Self>>>(
        self,
        range: Range<&Self>,
        opt: Opt,
        f: impl Fn(Self) -> I,
    ) -> ValX<'a, Self> {
        let (s, e) = (range.start.map(Val::force), range.end.map(Val::force));
        let j = self.owned();
        update(false, f, |g| j.map_range(s..e, opt, g))
    }

    fn as_bool(&self) -> bool {
        self.force().as_bool()
    }

    fn into_string(self) -> Self {
        match self {
            Self::Ready(j) => Self::Ready(j.into_string()),
            u => u,
        }
    }
}

impl jaq_std::ValT for Val {
    fn into_seq<S: FromIterator<Self>>(self) -> Result<S, Self> {
        match self.owned() {
            J::Arr(a) => Ok(a.iter().cloned().map(Val::Ready).collect()),
            j => Err(Val::Ready(j)),
        }
    }
    fn is_int(&self) -> bool {
        self.force().is_int()
    }
    fn as_isize(&self) -> Option<isize> {
        self.force().as_isize()
    }
    fn as_f64(&self) -> Option<f64> {
        self.force().as_f64()
    }
    fn is_utf8_str(&self) -> bool {
        self.force().is_utf8_str()
    }
    fn as_bytes(&self) -> Option<&[u8]> {
        self.force().as_bytes()
    }
    fn as_sub_str(&self, sub: &[u8]) -> Self {
        Self::Ready(self.force().as_sub_str(sub))
    }
    fn from_utf8_bytes(b: impl AsRef<[u8]> + Send + 'static) -> Self {
        Self::Ready(J::from_utf8_bytes(b))
    }
}
