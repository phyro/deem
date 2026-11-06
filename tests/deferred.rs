//! An answer that arrives later behaves like the plain value it becomes.
//! Every program runs twice against a model with fixed answers: through
//! deem's `judge`, whose answers arrive later, and with `judge`, `ask` and
//! `choose` redefined in jq to return the same answers at once. Both runs
//! must print the same.

mod common;

use common::*;
use deem::RunOptions as Opts;
use deem::api::{Answer, ApiError, Judge, Request, Response};
use serde_json::{Value as Json, json};
use std::sync::Arc;

/// Every score is the same number; every choice is the first option in
/// sorted order (the programs below list their options sorted).
struct Fixed(Json);

impl Judge for Fixed {
    fn judge(&self, req: &Request) -> Result<Response, ApiError> {
        let answers = req
            .questions
            .iter()
            .map(|(k, q)| {
                let q: Json = serde_json::from_str(q.get()).unwrap();
                let value = match q.get("criteria").filter(|_| q["type"] == "choice") {
                    Some(c) => json!(c.as_object().unwrap().keys().next().unwrap()),
                    None => self.0.clone(),
                };
                (k.clone(), Answer { value, confidence: None, probabilities: None })
            })
            .collect();
        Ok(Response { answers, input_tokens: None, raw: None })
    }
}

/// The same answers, at once. `$` parameters take their arguments in the
/// order `judge` does, so generators give the same outputs in the same order.
fn eager(v: &Json, program: &str) -> String {
    format!(
        "def judge($s; $c): {{value: (if $s.among then $s.among[0] else {v} end), confidence: null, probabilities: null}}; \
         def ask(q; c): judge({{q: q}}; c).value; \
         def choose(q; o; c): judge({{q: q, among: o}}; c).value; {program}"
    )
}

fn run(program: &str, input: &str, opts: &Opts, v: &Json) -> (i32, String, String) {
    let r = run_with(program, input, opts, &config(), Arc::new(Fixed(v.clone())));
    (r.code, r.stdout, r.stderr)
}

const PROGRAMS: &[&str] = &[
    // an error in arithmetic on, or indexing into, an answer is caught by
    // the `try` or `?` around it
    r#"try (ask("q"; .) + "s") catch "caught""#,
    r#"(ask("q"; .) + "s")?"#,
    r#"[ask("q"; .)] | getpath([0, "x"])?"#,
    r#"ask("q"; .) | getpath(["x"])?"#,
    r#"ask("q"; .) | .x?"#,
    r#"[ask("q"; .) | .x?] | length"#,
    // looking at an answer the run doesn't have is never an error a `try`
    // or `?` could catch
    r#"try (ask("q"; .) | .[]) catch "caught""#,
    r#"[ask("q"; .) | .[]?]"#,
    r#"(ask("q"; .) | tostring | ltrimstr("0"))?"#,
    r#"[.[]?, ask("q"; .)] | try sort catch "unsortable""#,
    r#"try (ask("q"; .) | keys) catch "no keys""#,
    r#"ask("q"; .) as $a | isempty($a | select(. > 2))"#,
    r#"first(ask("q"; .), 1)"#,
    r#"label $f | ask("q"; .) | ., break $f"#,
    // truthiness and logic
    r#"if ask("q"; .) then "t" else "f" end"#,
    r#"select(ask("q"; .)) | "kept""#,
    r#"ask("q"; .) and false"#,
    r#"ask("q"; .) or error("x")"#,
    r#"ask("q"; .) | not"#,
    r#"[ask("q"; .) // "alt"]"#,
    r#"[(ask("q"; .), null, false) // "alt"]"#,
    // comparison and ordering
    r#"[ask("q"; .), 0.5, null, "s", [], {}, true] | sort"#,
    r#"[ask("q"; .), ask("r"; .), 0.3] | unique"#,
    r#"[ask("q"; .), 0.3] | (.[0] == .[1]), (.[0] < .[1])"#,
    r#"[1, ask("q"; .), 0] | min, max"#,
    r#"[{k: ask("q"; .)}, {k: 0.4}] | sort_by(.k) | map(.k)"#,
    r#"[{k: ask("q"; .)}, {k: 0.4}] | group_by(.k > 0.35) | map(length)"#,
    r#"[{k: ask("q"; .)}, {k: 0.3}] | unique_by(.k) | length"#,
    r#"[ask("q"; .)] | index(0.3), (map(tostring) | index("0.3"))"#,
    r#"ask("q"; .) | [. == 0, . == 0.0, . == 1, . == 1.0]"#,
    r#"[ask("q"; .), 0.3] | bsearch(0.3)"#,
    r#"ask("q"; .) | IN(0.3, 0.5, 1)"#,
    r#"[ask("q"; .)] | inside([0.3, 0.5, 1, 0, 0.0, 1.0])"#,
    r#"{k: ask("q"; .)} | contains({k: 0.3}), has("k")"#,
    r#"[ask("q"; .), 2, 1] | min_by(.), max_by(.), (sort_by(-.) | first)"#,
    r#"[ask("q"; .), 0.9] | any(. > 0.5), all(. > 0.5), any, all"#,
    r#"[ask("q"; .), 0.9] | map(select(. >= 0.3)) | length"#,
    r#"[ask("q"; .)] | indices(0.3), indices([0.3]), index([0.3])"#,
    r#"[ask("q"; .), ask("q"; .)] | indices(ask("q"; .))"#,
    // arithmetic and numbers
    r#"ask("q"; .) | . + 1, . - 1, . * 3, . / 3, . % 1"#,
    r#"ask("q"; .) | 1 / ."#,
    r#"ask("q"; .) | 5 % ."#,
    r#"ask("q"; .) | floor, ceil, round, fabs, sqrt, -."#,
    r#"ask("q"; .) | log"#,
    r#"ask("q"; .) | pow(.; 0), pow(0; .)"#,
    r#"ask("q"; .) | tostring, tojson, @text, @json, @html"#,
    r#"ask("q"; .) | tostring | tonumber"#,
    r#"ask("q"; .) * 1e308 * 10"#,
    r#"ask("q"; .) | [., .] | add"#,
    r#"[ask("q"; .), "s"] | add"#,
    r#"ask("q"; .) | length, utf8bytelength?"#,
    r#"ask("q"; .) | isinfinite, isnormal, significand?"#,
    r#"ask("q"; .) | frexp, modf"#,
    r#"ask("q"; .) | @base64?"#,
    r#"ask("q"; .) | [limit(3; repeat(.))]"#,
    r#"ask("q"; .) | todate?"#,
    r#"ask("q"; .) | . * nan | [., isnan, (. < 1), (. == .)]"#,
    r#"[ask("q"; .) * nan, 1, null] | sort"#,
    r#"ask("q"; .) | . * -1 | [., (. == 0), tostring, tojson]"#,
    r#"ask("q"; .) | -(.) * 0 | [., tostring]"#,
    r#"ask("q"; .) | . * infinite | [., isinfinite, tostring]"#,
    r#"ask("q"; .) | 0 * infinite * . | isnan"#,
    r#"[ask("q"; .) - ask("q"; .)] | .[0] | [., . == 0]"#,
    r#"ask("q"; .) | . + 0.1 + 0.2 | tostring"#,
    r#"ask("q"; .) | . * 3 / 3 == ."#,
    r#"ask("q"; .) | [floor, ceil, round, trunc?, nearbyint, rint] | map(tostring)"#,
    r#"ask("q"; .) * 1e9 | todate"#,
    r#"ask("q"; .) * 1e9 | strftime("%Y-%m-%d")"#,
    r#"ask("q"; .) * 1e9 | gmtime | mktime"#,
    r#"ask("q"; .) | (. * 65 + 65 | floor) | [.] | implode"#,
    // answers as indices, keys, counts and paths
    r#"[10, 20, 30] | .[ask("q"; .)]"#,
    r#"[10, 20, 30] | .[ask("q"; .) * 3]"#,
    r#"[10, 20, 30] | .[ask("q"; .):]"#,
    r#"[10, 20, 30] | .[:ask("q"; .) * 3]"#,
    r#""abcdef" | .[ask("q"; .) * 6:]"#,
    r#"[range(5)] | .[ask("q"; .) * 4 | floor:] | length"#,
    r#"[range(5)] | .[-(ask("q"; .) * 4 | floor)]"#,
    r#"[range(5)] | del(.[ask("q"; .) * 4 | floor])"#,
    r#"[range(5)] | to_entries | map(select(.key < ask("q"; .) * 5)) | length"#,
    r#"[ask("q"; .)] | .[ask("q"; .) | floor]"#,
    r#"[range(ask("q"; .) * 10)] | length"#,
    r#"[limit(ask("q"; .) * 10 | floor; range(100))] | length"#,
    r#"[limit(ask("q"; .) * 3 | floor; 1, 2, 3)]"#,
    r#"[3, 1, 2] | nth(ask("q"; .) * 2 | floor)"#,
    r#"[nth(ask("q"; .) * 2 | floor; 10, 20, 30)]"#,
    r#"[first(range(10) | select(. >= ask("q"; .) * 5))]"#,
    r#""ab" * (ask("q"; .) * 10)"#,
    r#""ab" * ask("q"; .)"#,
    r#"{} | .a.b.c = ask("q"; .)"#,
    r#"null | .[ask("q"; .) * 2 | floor] = 1"#,
    r#"[1, 2] | .[ask("q"; .) * 4 | floor] = 9"#,
    // containers holding answers
    r#"{a: ask("q"; .)} | del(.a), to_entries, [paths]"#,
    r#"{a: ask("q"; .), b: [ask("r"; .)]} | [tostream]"#,
    r#"{a: ask("q"; .), b: [ask("r"; .)]} | fromstream(tostream)"#,
    r#"{a: ask("q"; .), b: [ask("r"; .)]} | [.. | numbers]"#,
    r#"{a: ask("q"; .), b: [ask("r"; .)]} | walk(if type == "number" then . * 2 else . end)"#,
    r#"{a: ask("q"; .)} | with_entries(.key |= ascii_upcase)"#,
    r#"{a: [ask("q"; .), {b: ask("r"; .)}]} | [paths], [paths(numbers)], (tostream | select(length == 2) | .[1])"#,
    r#"{a: [ask("q"; .), {b: ask("r"; .)}]} | [getpath(paths(numbers))] | add"#,
    r#"{a: ask("q"; .)} | to_entries[0] | .value += 1 | .value"#,
    r#"{a: ask("q"; .)} + {a: 1}, {a: 1} + {a: ask("q"; .)}"#,
    r#"{a: {b: ask("q"; .)}} * {a: {c: 1}}"#,
    r#"[ask("q"; .), 0.3, 1] - [0.3]"#,
    r#"[ask("q"; .), ask("r"; .)] - [ask("q"; .)]"#,
    r#"[[ask("q"; .)], [[ask("r"; .)]]] | flatten(1), flatten"#,
    r#"[[ask("q"; .)], [2]] | flatten, transpose, combinations"#,
    r#"[ask("q"; .), 1] | transpose?"#,
    r#"[ask("q"; .)] | implode?"#,
    r#"[ask("q"; .)] | tojson | fromjson"#,
    r#"[ask("q"; .)] | @csv, @tsv, @sh"#,
    r#"[ask("q"; .)] | pick(.[0])"#,
    r#"[ask("q"; .)] | paths(type == "number")"#,
    r#"[ask("q"; .)] | .[0] |= . * 2"#,
    r#"[ask("q"; .)] | .[1:] | length"#,
    r#"[ask("q"; .)] | tostream"#,
    r#"[[ask("q"; .)] | tostream] | fromstream(.[])"#,
    r#"[ask("q"; .), .] | tojson | fromjson | .[0]"#,
    r#"{(ask("q"; .) | tostring): ask("q"; .)} | keys[0] | tonumber"#,
    r#"[{key: choose("c"; ["x", "y"]; .), value: ask("q"; .)}] | from_entries"#,
    r#"{a: 0.1, b: 0.9} | map_values(select(. > ask("q"; "x")))"#,
    r#"{a: 0.1, b: 0.9} | del(.[] | select(. < ask("q"; "x")))"#,
    r#"{a: 0.1, b: 0.9} | to_entries | map(select(.value > ask("q"; "x"))) | from_entries"#,
    r#"[0.1, 0.9] | map(. > ask("q"; "x"))"#,
    r#"[0.1, 0.9] | .[] |= ask("q"; .)"#,
    r#"{a: "x"} | .a |= ask("q"; .)"#,
    r#"{a: "x"} | .a += ask("q"; .)"#,
    r#"{a: 1} | .a //= ask("q"; .)"#,
    r#"{a: ask("q"; .)} | .b = .a | .a = 1 | .b"#,
    r#"[ask("q"; .)] | .[0] as $x | .[0] = 5 | [., $x]"#,
    r#"ask("q"; .) as $x | {x: $x} | .x |= . + 1 | [.x, $x]"#,
    // choices as strings, keys and patterns (options sorted)
    r#"{x: 1, y: 2} | .[choose("c"; ["x", "y"]; .)]"#,
    r#"{x: 1, y: 2} | has(choose("c"; ["x", "y"]; .))"#,
    r#"{(choose("c"; ["x", "y"]; .)): ask("q"; .)}"#,
    r#"{} | .[choose("c"; ["x", "y"]; .)] = 5"#,
    r#"{} | setpath([choose("c"; ["x", "y"]; .)]; ask("q"; .))"#,
    r#"{x: {y: 1}} | getpath([choose("c"; ["x", "y"]; .), "y"])"#,
    r#"[ask("q"; .), choose("c"; ["x", "y"]; .)] | join("-")"#,
    r#"choose("c"; ["x", "y"]; .) | test("x"), ascii_upcase, explode, ltrimstr("x"), split("")"#,
    r#"choose("c"; ["x", "y"]; .) | [match("."; "g").string]"#,
    r#"choose("c"; ["x", "y"]; .) | sub("x"; "z"), gsub("(?<l>.)"; "\(.l)\(.l)")"#,
    r#"choose("c"; ["x", "y"]; .) as $k | {x: 1} | has($k), .[$k], (keys | index($k))"#,
    r#"choose("c"; ["x", "y"]; .) | @base64, @uri, (@base64 | @base64d)"#,
    r#"choose("c"; ["x", "y"]; .) | tojson | fromjson"#,
    r#"choose("c"; ["false", "true"]; .) | if . then "truthy" else "falsy" end"#,
    r#"choose("c"; ["1", "2"]; .) | tonumber + 1"#,
    r#"choose("c"; ["null", "x"]; .) | [., fromjson, (fromjson | type)]"#,
    r#"choose("c"; ["", "x"]; .) | [., length, (. == "")]"#,
    r#"choose("c"; [" ", "x"]; .) | [., ltrimstr(" "), test("^\\s$")]"#,
    r#"choose("c"; ["a\"b", "c"]; .) | [., tojson, @sh, @csv "\([.])"]"#,
    r#"choose("c"; ["1e2", "x"]; .) | [., tonumber, (tonumber | tostring)]"#,
    r#"choose("c"; ["__proto__", "x"]; .) as $k | {} | .[$k] = 1 | keys"#,
    r#"choose("c"; ["é", "ü"]; .) | [., length, utf8bytelength, explode, ascii_downcase]"#,
    r#"choose("c"; ["(", "x"]; .) as $re | try ("a(" | test($re)) catch "bad regex""#,
    r#"choose("c"; ["a+", "x"]; .) as $re | "caaab" | [test($re), (match($re).length), sub($re; "-")]"#,
    r#"choose("c"; [".", "x"]; .) as $sep | "a.b.c" | [splits($sep)] | length"#,
    r#"choose("c"; ["x", "y"]; .) as $k | [{x: 2}, {x: 1}] | sort_by(.[$k]) | map(.x)"#,
    r#"choose("c"; ["x", "y"]; .) as $k | {x: 1, y: 2} | del(.[$k]), with_entries(select(.key != $k))"#,
    r#"[choose("c"; ["x", "y"]; .), "x", "y"] | unique, group_by(.), (map({(.): 1}) | add)"#,
    r#"choose("c"; ["x", "y"]; .) | . as $a | {a: $a} * {a: {b: $a}}"#,
    r#"choose("c"; ["x", "y"]; .) | indices("x"), index("x"), rindex("x")"#,
    r#"choose("c"; ["x", "y"]; .) | @html, @uri"#,
    // errors, try, labels: what jq catches, deem catches
    r#"try error(ask("q"; .)) catch ."#,
    r#"try error({v: ask("q"; .)}) catch .v"#,
    r#"try (ask("q"; .) | error) catch (. * 2)"#,
    r#"[.[]?] | try error(ask("q"; .)) catch type"#,
    r#"ask("q"; .) | error"#,
    r#"ask("q"; .) | error? // "err""#,
    r#"[ask("q"; .) | try error catch .]"#,
    r#"ask("q"; .) | try error("e: \(.)") catch ."#,
    r#"try error([ask("q"; .)]) catch .[0]"#,
    r#"try (ask("q"; .) | ltrimstr(1) | . + "s") catch "caught""#,
    r#"[.[]? // ask("q"; .)]"#,
    r#"ask("q"; .) | keys?, has(0)?, .[]?, to_entries?, splits("x")?, test("0")?, ltrimstr(.)?"#,
    r#"ask("q"; .) | ltrimstr("x"), rtrimstr("x"), startswith("x")?"#,
    r#"[ask("q"; .), {x: 1}] | .[] | select(type == "object") | .x"#,
    r#"label $f | ask("q"; .) | if . < 2 then ., break $f else . end"#,
    r#"label $out | [ask("q"; .), ask("r"; .)] | .[] | if . >= 0 then ., break $out else . end"#,
    r#"first(ask("q"; .) | select(. > 0.4)) // "none""#,
    r#"isempty(ask("q"; .) | select(. > 0.4))"#,
    r#"[ask("q"; .) | select(. > 0.4)] | length"#,
    r#"[ask("q"; .), ask("r"; .)] | first(.[] | select(. >= 0)), last"#,
    r#"[ask("q"; .), ask("r"; .)] | [limit(1; .[])], [first(.[])], isempty(.[])"#,
    // bindings, definitions, loops
    r#"ask("q"; .) as $x | reduce range(3) as $i ($x; . * 2)"#,
    r#"[foreach (ask("q"; .), ask("r"; .)) as $x (0; . + $x; [., $x])]"#,
    r#"ask("q"; .) as $x | [range(3) | $x] | add"#,
    r#"[ask("q"; .)] as [$a, $b] | [$a, $b]"#,
    r#"{a: ask("q"; .)} as {a: $a, $b} | [$a, $b]"#,
    r#"ask("q"; .) as $v | [1, 2, 3] | map(. * $v) | add"#,
    r#"ask("q"; .) as $v | {a: 1} | map_values(. + $v) | .a"#,
    r#"[ask("q"; .), ask("r"; .), ask("s"; .)] | [while(length > 0; .[1:])] | length"#,
    r#"ask("q"; .) as $v | [recurse(if . < 3 then . + 1 else empty end)] | map(. + $v) | length"#,
    r#"def f($n): if $n > 0 then ask("q\($n)"; .), f($n - 1) else empty end; [f(3)] | add"#,
    r#"def f: .[0] + .[1]; [ask("q"; .), ask("r"; .)] | f"#,
    r#"def g(x): x + x; g(ask("q"; .))"#,
    r#"def h(f): [f, f]; h(ask("q"; .))"#,
    r#"reduce (ask("q"; .), ask("r"; .), 1) as $x ([]; . + [$x]) | length"#,
    r#"reduce empty as $x (ask("q"; .); . + 1)"#,
    r#"[foreach (1, 2) as $x (ask("q"; .); . + $x)]"#,
    r#"ask("q"; .) | [limit(3; repeat(. + 1))] | length"#,
    // strings from answers
    r#"ask("q"; .) | tostring | split(".")"#,
    r#"ask("q"; .) | @text "v=\(.)" | ascii_downcase"#,
    r#"ask("q"; .) | "\(.)" as $s | $s | length"#,
    r#"ask("q"; .) | tostring | ltrimstr("0.") | length"#,
    r#"[ask("q"; .)] | tostring | [scan("[0-9]")] | length"#,
    r#"ask("q"; .) | tojson == tostring"#,
    r#"ask("q"; .) | type, (. | numbers), (. | strings), [scalars, values, nulls, arrays]"#,
    // the environment stays empty; time still works
    r#"ask("q"; .) | $ENV.PATH, (env | length), (builtins | length > 100), (now | type)"#,
    // memo and generators
    r#"[ask("q"; .), ask("q"; .)] | .[0] == .[1]"#,
    r#"ask("q"; ask("r"; .)) == ask("q"; ask("r"; .))"#,
    r#"[ask("q"; .), ask("q"; "other"), ask("q"; null), ask("q"; [])] | length"#,
    r#"[ask(("a", "b"); (1, 2))] | length"#,
    r#"[choose(("a", "b"); (["x", "y"], ["y", "z"]); (1, 2))] | length, .[0]"#,
    r#"[judge(({q: "a"}, {q: "b", among: ["x", "y"]}); .) | .value] | map(type)"#,
    // the judge object
    r#"judge({q: "q"}; .) | .value, .confidence, .probabilities, keys"#,
    r#"judge({q: "q", among: ["x", "y"]}; .) | .value"#,
    r#"judge({q: "q"}; .) | .value = 7 | .value"#,
    r#"judge({q: "q"}; .) | del(.value) | keys"#,
    r#"[judge({q: "q"}; .)] | sort_by(.value) | length"#,
    r#"judge({q: "q"}; .) | to_entries | map(.key)"#,
];

/// Every program, on a text record and a JSON one, with every score `v`.
fn same_as_eager(v: Json) {
    let text = Opts::default();
    let json_in = Opts { json_in: true, ..Opts::default() };
    let obj = "{\"a\":[3,1,2],\"b\":\"x\",\"n\":null,\"o\":{\"x\":1,\"y\":2}}\n";
    let mut wrong = Vec::new();
    for &p in PROGRAMS {
        for (input, opts) in [("a\n", &text), (obj, &json_in)] {
            let deferred = run(p, input, opts, &v);
            let eager = run(&eager(&v, p), input, opts, &v);
            // a compile error prints the program under its message
            let ok = !deferred.2.contains("\n    ") && deferred == eager;
            if !ok {
                wrong.push(format!("{p}\n  on {input:?}\n  deferred: {deferred:?}\n  eager:    {eager:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "with every score {v}:\n{}", wrong.join("\n"));
}

#[test]
fn score_0_3() {
    same_as_eager(json!(0.3));
}

#[test]
fn score_0() {
    same_as_eager(json!(0.0));
}

#[test]
fn score_0_5() {
    same_as_eager(json!(0.5));
}

#[test]
fn score_1() {
    same_as_eager(json!(1.0));
}
