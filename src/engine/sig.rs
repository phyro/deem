//! What a program declares: `module {input: …, defaults: {…}};` on its
//! first line, and `main`'s parameters. jaq parses the directive but keeps
//! it to itself, so it is read here from the tokens.

use super::{J, report};
use jaq_core::load::lex::{Lexer, StrPart, Tok, Token};
use jaq_core::load::{self, parse::Term};

/// What `main` gets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Input {
    /// one record: `main` runs once per record
    #[default]
    Each,
    /// an array of every record: `main` runs once
    All,
}

/// A program's signature.
#[derive(Debug, Default)]
pub struct Sig {
    pub input: Input,
    /// named values, in declared order, with their defaults
    pub defaults: Vec<(String, J)>,
    /// `main`'s parameters, without `$`; `None` for plain jq
    pub params: Option<Vec<String>>,
}

impl Sig {
    /// Read the directive and find `main`. A program that doesn't lex is
    /// plain jq here; compiling it reports why.
    pub fn of(src: &str) -> Result<Self, String> {
        let Ok(tokens) = Lexer::new(src).lex() else { return Ok(Self::default()) };
        let mut sig = Self::default();
        let rest = match &tokens[..] {
            [Token("module", Tok::Word), Token(meta, Tok::Block(_)), Token(semi @ ";", Tok::Sym), ..]
                if meta.starts_with('{') =>
            {
                sig.directive(src, meta)?;
                &src[load::span(src, semi).end..]
            }
            [Token("module", Tok::Word), Token(at, _), ..] => {
                return Err(report(
                    src,
                    vec![(at, "the module directive is an object: module {input: \"each\"};".into())],
                ));
            }
            _ => src,
        };
        // Only definitions: a tool. Definitions and a body: plain jq.
        if let Some(defs) = load::parse(rest, |p| p.defs()) {
            let main = defs.iter().rev().find(|d| d.name == "main");
            let Some(main) = main else {
                let at = defs.last().map_or(rest, |d| d.name);
                return Err(report(src, vec![(at, "a program of only definitions needs `def main: …;`".into())]));
            };
            sig.params = Some(main.args.iter().map(|a| a.trim_start_matches('$').to_owned()).collect());
        } else if let Some((defs, _)) = load::parse(rest, |p| Ok((p.defs()?, p.term()?)))
            && let Some(main) = defs.iter().find(|d| d.name == "main")
        {
            let msg = "a program that defines main is only definitions: move the code after it into main";
            return Err(report(src, vec![(main.name, msg.into())]));
        }
        for (name, _) in &sig.defaults {
            if sig.params.iter().flatten().any(|p| p == name) {
                let at = src.find(&format!("{name}:")).map_or(src, |i| &src[i..]);
                return Err(report(src, vec![(at, format!("{name} is both a named value and an argument of main"))]));
            }
        }
        Ok(sig)
    }

    /// `{input: "each" | "all", defaults: {NAME: number | string, …}}`
    fn directive(&mut self, src: &str, meta: &str) -> Result<(), String> {
        let fail = |at: &str, msg: String| Err(report(src, vec![(at, msg)]));
        // One that doesn't parse fails compiling too, with jaq's message.
        let Some(Term::Obj(entries)) = load::parse(meta, |p| p.term()) else { return Ok(()) };
        for (k, v) in &entries {
            let (key, at) = string(k).unwrap_or_default();
            let at = at.unwrap_or(meta);
            match (key.as_str(), v) {
                ("input", Some(v)) => {
                    self.input = match string(v).map(|s| s.0).as_deref() {
                        Some("each") => Input::Each,
                        Some("all") => Input::All,
                        _ => return fail(at, "input is \"each\" or \"all\"".into()),
                    }
                }
                ("defaults", Some(Term::Obj(vals))) => {
                    for (k, v) in vals {
                        let (name, at) = string(k).unwrap_or_default();
                        let at = at.unwrap_or(meta);
                        if !is_ident(&name) {
                            return fail(at, format!("a named value is an identifier, not {name:?}"));
                        }
                        if self.defaults.iter().any(|(n, _)| *n == name) {
                            return fail(at, format!("{name} is declared twice"));
                        }
                        let Some(val) = v.as_ref().and_then(constant) else {
                            return fail(at, format!("the default of {name} is a number or a string"));
                        };
                        self.defaults.push((name, val));
                    }
                }
                ("defaults", _) => return fail(at, "defaults is an object: defaults: {threshold: 0.5}".into()),
                ("input", None) => return fail(at, "input is \"each\" or \"all\"".into()),
                _ => {
                    return fail(
                        at,
                        format!("unknown key {key:?} in the module directive (it takes input and defaults)"),
                    );
                }
            }
        }
        Ok(())
    }
}

impl Sig {
    /// A named value's type comes from its default.
    pub fn value(&self, name: &str, text: &str) -> Result<J, String> {
        let Some((_, default)) = self.defaults.iter().find(|(n, _)| n == name) else {
            let names: Vec<_> = self.defaults.iter().map(|(n, _)| n.as_str()).collect();
            let has = if names.is_empty() { "it has none".into() } else { format!("it has: {}", names.join(", ")) };
            return Err(format!("the program has no value {name:?} ({has})"));
        };
        match default {
            J::TStr(_) | J::BStr(_) => Ok(J::from(text.to_owned())),
            _ => number(text).ok_or_else(|| format!("--arg {name}: {text:?} is not a number")),
        }
    }
}

/// A key or string literal, and where it is.
fn string<'s>(t: &Term<&'s str>) -> Option<(String, Option<&'s str>)> {
    let Term::Str(None, parts) = t else { return None };
    let mut s = String::new();
    for part in parts {
        match part {
            StrPart::Str(p) => s.push_str(p),
            StrPart::Char(c) => s.push(*c),
            StrPart::Term(_) => return None,
        }
    }
    let at = parts.iter().find_map(|p| if let StrPart::Str(p) = p { Some(*p) } else { None });
    Some((s, at))
}

/// A number or a string literal.
fn constant(t: &Term<&str>) -> Option<J> {
    match t {
        Term::Num(n) => number(n),
        Term::Neg(n) => match &**n {
            Term::Num(n) => number(&format!("-{n}")),
            _ => None,
        },
        t => string(t).map(|(s, _)| J::from(s)),
    }
}

fn number(s: &str) -> Option<J> {
    let f: f64 = s.trim().parse().ok().filter(|f: &f64| f.is_finite())?;
    let j = jaq_json::read::parse_single(s.trim().as_bytes()).ok().filter(|j| matches!(j, J::Num(_)));
    Some(j.unwrap_or(J::from(f)))
}

pub(super) fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
