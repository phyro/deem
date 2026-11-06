//! The question forms: `deem QUESTION`, `-p`, `-o` and `-l` run these one-line
//! programs, with the question (and the options or levels) as arguments.

/// What a question form prints.
#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// the lines where the answer is yes
    Filter,
    /// `-p`: `score<TAB>record`
    Score,
    /// `-o A,B`: `option<TAB>record`
    Label(String),
    /// `-l A,B,C`: `level<TAB>record`, the level a number on the scale
    Rate(String),
}

/// `deem QUESTION`: the lines where the answer is yes.
pub const FILTER: &str = r#"module {input: "each", defaults: {threshold: 0.5}};
def main($q): select(ask($q; .) > $threshold);"#;

/// `deem QUESTION -p`: `score<TAB>record`.
pub const SCORE: &str = r#"module {input: "each"};
def show: if type == "string" then . else tojson end;
def main($q): "\(ask($q; .))\t\(show)";"#;

/// `deem QUESTION -o A,B`: `option<TAB>record`. An option may hold spaces;
/// those around a comma are dropped.
pub const LABEL: &str = r#"module {input: "each"};
def show: if type == "string" then . else tojson end;
def main($q; $opts): "\(choose($q; $opts | split(",") | map(trim); .))\t\(show)";"#;

/// `deem QUESTION -l A,B,C`: `level<TAB>record`, `0` the first level and
/// `n-1` the last; it can fall between two. A level may hold spaces, as an
/// option may.
pub const RATE: &str = r#"module {input: "each"};
def show: if type == "string" then . else tojson end;
def main($q; $levels): "\(rate($q; $levels | split(",") | map(trim); .))\t\(show)";"#;

/// The program a question form runs, and its arguments: the question, and
/// the options or levels. They have no name: `deem programs` doesn't list them.
pub fn program(question: &str, mode: &Mode) -> (&'static str, Vec<String>) {
    match mode {
        Mode::Filter => (FILTER, vec![question.into()]),
        Mode::Score => (SCORE, vec![question.into()]),
        Mode::Label(opts) => (LABEL, vec![question.into(), opts.clone()]),
        Mode::Rate(levels) => (RATE, vec![question.into(), levels.clone()]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        assert_eq!(program("q", &Mode::Label("a,b".into())), (LABEL, vec!["q".into(), "a,b".into()]));
        assert_eq!(program("q", &Mode::Filter), (FILTER, vec!["q".into()]));
        assert_eq!(program("q", &Mode::Rate("a,b".into())), (RATE, vec!["q".into(), "a,b".into()]));
    }
}
