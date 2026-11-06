---
name: deem-programs
description: Write deem commands — jq programs that ask a model typed questions (ask, choose, rate, box, judge) about each line, JSON record or picture — to filter, score, label, rate or triage logs, tickets, feedback, commits, screenshots or photos by meaning. Use this whenever someone wants to find, rank, tag, count or route records by what they mean rather than by keywords (e.g. "which of these log lines would a user notice", "tag these tickets by product area", "which screenshots show an error", "where in the picture is the button", "grep but semantic"), whenever they mention deem, ask(), choose(), rate(), box() or judge(), or when a deem program is slow, costs many waves or requests, fails, or prints nothing, or which API it sends to (`deem api`, `--api`, a local server that speaks the Jev API), or how close one API's answers are to another's (`deem trace compare --with NAME`).
---

# Writing deem programs

The everyday forms need no program: `deem "Q?" file` keeps the lines where
the answer is yes, `-p` prints `score<TAB>line`, `-o a,b,c` prints
`option<TAB>line`, `-l low,mid,high` prints `level<TAB>line` (`0` the
first level; it can fall between two), and `-t N` sets the threshold.
For anything else, `deem run PROGRAM` runs a program: a built-in one
(`group`, `histogram`, `mix`, `disagree`), a saved one (`.deem/NAME.deem` here or in the
nearest directory above that has one, then `~/.deem/NAME.deem`), a path
to a `.deem` file, or jq text. `deem programs` lists them; `deem
programs NAME` prints one. A program is jq, run on every input record,
and it can ask a model questions:

- `ask(q; ctx)`: a calibrated score in [0,1] for a yes/no question about ctx
- `choose(q; opts; ctx)`: one of the options; `opts` is a list of names, or `{name: "what it means", …}`
- `rate(q; levels; ctx)`: a number in [0, n−1] on a scale of 2–10 levels,
  lowest first (`["can wait", "this week", "now"]`)
- `box(q; ctx)`: where in a picture, `{top, bottom, left, right}` as fractions
  (needs Jev 1.14: `jev-preview` if the model refuses it; always answers, so also `ask` if it's there)
- `judge(spec; ctx)`: the full answer, `{value, confidence, probabilities}`;
  spec is `{q, among?, levels?, box?, yes?, no?, like?, unlike?}`; `yes`,
  `no` (what each means) and `like`, `unlike` (examples) are for yes/no
  questions only: a choice or scale describes each option or level

The context (the last argument) is required: there is no `ask(q)`.

Pictures: a PNG or JPEG file given as input is one record (`deem "is there a
person?" cam/*.jpg` prints matching paths; `-I` reads paths from stdin), and
`--img NAME FILE` binds one to `$NAME`. Put pictures in the value and name
them in backticks: `ask("does `photo` show `claim`?"; {claim: .c, photo: $p})`.
A picture can't be inside a longer string or the question's text. Each
wave that asks about a picture sends it again (about 1.2k tokens), so ask
a picture's questions together and follow up only when that skips most
pictures.

The last argument is all the model sees: `ask(q; .body)` sends the body,
`ask(q; .)` the whole record; nothing else is sent. The question itself is
computed from the record, so `ask("is a \(.level) bad?"; .msg)` asks
about the level but sends only the message. A record is a line, as text,
unless `-J` is given: then each JSON value is a record, however it is laid
out (an array is one value; split it with `jq -c '.[]'`). JSON input always
needs `-J`, also for saved programs. Output follows input: raw lines for
text, compact JSON with `-J`. Exit codes
are grep's: 0 output, 1 nothing selected, 2 an error.

## What to give the user

A command they can paste, then one or two sentences on why it is shaped
that way (how many round trips it takes, what a cheap filter saves), and
what its output will look like. Use the user's file names; if they
didn't give any, say which file the command expects.

**Don't run deem against the API yourself.** Every question costs the
user money, and a program over a large file can ask thousands. Hand over
the command and let the user run it; suggest `--stats` on a sample
(`head -200 file | deem --stats …`) when cost is a concern. The one thing
safe to run is the compile check below, which can't reach the API. The
same goes for `deem tune`: it asks the user to label lines, so the user
runs it.

## Pick the shape

| the user wants | shape |
|---|---|
| keep the matching records | `deem "Q?" -t 0.7 file` (= `select(ask("Q?"; .) > 0.7)`) |
| a ranking | `deem "Q?" -p file \| LC_ALL=C sort -rn` |
| a label or category | `deem "Q?" -o a,b,c file \| cut -f1 \| sort \| uniq -c` |
| a level on a scale, ranked | `deem "how urgent?" -l "can wait,this week,now" file \| sort -rn` (= `rate`; levels lowest first, 2 to 10) |
| data plus answers | `deem run '{text: ., score: ask("Q?"; .), kind: choose("Q?"; [...]; .)}' file` |
| one field of JSON judged | `deem run -J 'select(ask("Q?"; .body) > 0.8)' tickets.jsonl` |
| a count, no threshold | `deem "Q?" -p file \| awk '{s+=$1} END {print s}'` (scores are calibrated, so their sum estimates the count) |
| the whole file as one question | `deem run -s 'ask("Q?"; .)' file` |
| multi-line records | `deem --split '\n\n+' "Q?" file` |
| route unsure answers | `deem run 'judge({q: "Q?", among: [...]}; .) \| if .confidence < 0.75 then ... end'` |
| lines that mean the same, grouped | `deem run group "is this the same issue?" file` |
| is the question any good? | `deem run histogram "Q?" sample` (options: `deem run mix "Q?" a,b,c sample`), `deem run disagree "Q1?" "Q2?" sample`; then `deem tune "Q?" sample` and read the lines it disagrees with you on. `histogram` and `mix` print a table, then what else they found in labelled rows behind a line of dashes (`above 0.5`, `baseline`, `close a/b`), all on stdout |
| a threshold | `deem tune "Q?" sample` (interactive: the user labels ~20 lines) |
| a level, not yes or no | `deem run '{text: ., level: rate("how urgent?"; ["can wait", "this week", "now"]; .)}' file` |
| the pictures that show X | `deem "Q?" shots/*.png`, or `find shots -name '*.png' \| deem -I "Q?"` |
| where X is in a picture | `deem run '{shot: ., seen: ask("is there X?"; .), at: box("X"; .)} \| select(.seen > 0.5)' shots/*.png` on Jev 1.14: one request a picture, and `seen` because a box always comes back |
| a picture against a reference | ``deem run --img good ok.png 'select(ask("does `shot` look like `good`?"; {shot: ., good: $good}) > 0.5)' shots/*.png`` |
| a program worth keeping | a file in `.deem/NAME.deem`, then `deem run NAME file` (see below) |

## A program file

```jq
module {input: "each", defaults: {threshold: 0.5}};
# name: what it does (the line `deem programs` shows)
def main($q): select(ask($q; .) > $threshold);
```

`main`'s parameters are required arguments (the words after the program
name, then files); `defaults` are named values, set with `--arg NAME V`
(`-t N` is `--arg threshold N`). Values are bound as variables, never
pasted into the program. `input: "all"` runs `main` once with `.` the array
of every record, for counting, sorting or comparing records: build every
question first (`[.[] | ask(q; .)] | ...`), since `.[] | select(ask(q; .) >
t)` is a wave per record. A program that defines `main` prints strings raw.

Write questions as plain yes/no or pick-one questions about the record,
the way you'd ask a colleague: "would a user notice this?", not
"classify severity 1-10". Thresholds are the user's call; 0.5 is a
neutral start, and suggest looking at raw scores before picking one.

When the names of `choose`'s options leave room, describe each one:
`{loaded: "...", broken: "unstyled (default fonts, blue underlined links), overlapping or cut off"}`.
The description says what counts. Names alone called an unstyled page
loaded, and a land card white; described, both came out right.

## Mistakes that are easy to make

Each of these either fails to compile or silently selects nothing, so
check for them before answering:

- **Numbers need a leading digit.** `> 0.5`, never `> .5` (a syntax
  error in deem, though jq accepts it).
- **Arguments are separated by `;`.** `choose("Q?"; ["a","b"]; .)`; a comma
  makes it a one-argument call that doesn't exist.
- **Compare numbers with numbers.** `ask(q; .) > "0.5"` is legal jq and
  always false.
- **deem runs jaq's dialect of jq.** Setting a field under a missing
  parent fails the record: `.meta.score = ask(q; .)` on a record without
  `.meta`. Write `. * {meta: {score: ask(q; .)}}` (a recursive merge, safe at
  any depth). Never `.meta.score? = …`: that silently sets `.meta` to
  null and drops the score. `.meta += {score: …}` is fine one level down
  only; deeper, `+=` replaces the nested object. It can also print values that aren't JSON
  (`1 / 0` is `Infinity`, `nan` is `NaN`, `{(1): x}` has a number key),
  and says so on stderr; guard divisions and `tostring` computed keys when
  the output feeds another JSON tool. The manual's "Differences from jq"
  (`docs/MANUAL.md`) has the rest.
- **`sort -rn` needs `LC_ALL=C`** in locales with a decimal comma.
- **Judged values are capped at 32 KiB** (`DEEM_MAX_RECORD_BYTES`). Long
  strings are cut, objects over the limit fail: judge a field or a slice,
  `.text | .[0:4000] | ask(q; .)`.
- **`env` and `$ENV` are empty**, and there is no `input` or `import`: a
  program sees only its record (or, with `input: "all"`, every record), and
  the clock (`now`, `localtime`), which it can put in a question.
- **A question that starts with `-`** goes after `--`: `deem -t 0.7 -- "-1?" file`.

## Keep questions in one round trip

A record's questions go out together the moment it waits for an answer
(a round); wave k is every record's k-th round, whatever the timing. When
the round is answered, the record runs again from the start with those
answers (what it printed is not printed twice). A question doesn't wait
for another question: it waits only when the program **looks at** an
answer. Comparing it, `select`, `if`, `//`,
`and`/`or`, sorting, `length`, `test`, printing, or putting it in
another question's text all look. Building objects and arrays, `.value`,
arithmetic, string interpolation into output, and `as $x` do not.

So:

- `{a: ask(q1; .), b: ask(q2; .)}`, `[ask(a; .), ask(b; .)]`, `ask(a; .) * ask(b; .)`,
  `.xs | map(ask(q; .))`: one wave, the record's text sent once.
- `select(ask(a; .) > 0.5) | ask(b; .)`: two waves, and `b` is asked only for
  records that passed. That's a **cascade**, and it is what you want
  when `a` rules out most records.
- `ask("given \(ask(a; .)), is it b?"; .)`: two waves; the second question
  needs the first answer. Only do this if the text really needs it.
- `.xs | map(select(ask(q; .) > 0.5))`: **one wave per element**, and each
  wave runs the record again from the start. Ask first, look later:
  `.xs | map(ask(q; .)) | map(select(. > 0.5))` is one wave. The same goes
  for recursion that looks at an answer at every node: collect the answers
  over the structure first, then decide.
- Order matters: jq evaluates left to right, so an independent question
  placed after a dependent one waits with it. Put independent questions
  first, or bind them early: `ask(a; .) as $a | ... $a ...`.
- Identical questions about identical values are asked once per run and
  reused, so repeated lines and repeated `ask(q; .)` calls are free.
- A question the program asks is sent even if nothing uses its answer
  (`ask(q; .) as $x | "done"` sends one); one it never reaches is not.

**Put the cheap test first.** A regex or field check in front of `ask`
saves every request it filters out:
`select(test("ERROR")) | select(ask("would a user notice?"; .) > 0.8)`.

## A decision tree as data

When the next question depends on the last answer, and there are more than
a couple of branches, keep the tree as data and walk it: a leaf is a
string (the result), a node is `{q, yes, no}`.

```jq
module {input: "each"};
# decision-tree: route each line down a decision tree, one wave per level
def tree:
  { q: "is this an error?",
    yes: {q: "is it about security?", yes: "page-security", no: "page-oncall"},
    no:  {q: "is it a warning?",      yes: "ticket",        no: "ignore"} };
def decide($line):
  if type == "string" then .
  elif ask(.q; $line) > 0.5 then .yes | decide($line)
  else .no | decide($line)
  end;
def main: . as $line | {line: $line, action: (tree | decide($line))};
```

Every record walks its own path, but the records running together are at
the same depth at the same time, so they cost **one wave per level** (two
here), not one per record. Up to 192 records run together; a longer
input goes through that many at a time. Changing the routing means editing the
tree, not the walker. Bind the record first (`. as $line`) and pass
`$line`: an argument is evaluated against the function's input, so
`tree | decide(.)` would judge the tree, not the record. deem's
`examples/.deem/decision-tree.deem` is this program.

## Failures

An answer that can't be had (the API is down or refuses the request, the
model's answer is invalid) is a jq error at its `ask`, as if `ask` had
called `error`: `try` and `?` catch it, `//` doesn't (in jaq, `//` passes
errors on). Uncaught, it fails its record: the record's output is missing,
the reason goes to stderr, and the exit code is 2. deem retries network
errors and 5xx, isolates a refused request to the bad record, and stops the
run once if the API is down. When a program catches failed answers, deem
says how many on stderr at the end, since the output can't show it.

For a fallback, choose by what reads the output. `try ask(q; .) catch null`
gives a default in the same run, and is fine when a missing answer may
look like any other. When the output must be complete, don't catch: run
with `--failed retry.log`, then rerun `retry.log` later or give those
records a default in a second step. (A rerun that collects failures again
names another file: deem refuses a `--failed` file that is also its input.)
Mistakes in the question itself, such as `choose` with more than 255
options, are jq errors at the call, and arithmetic on an answer fails where
it is written, so `try (ask(q; .) + "s") catch 0` gives 0, as in jq.

## Cost and seeing a run

- `--stats` prints records, questions, requests, waves and input tokens
  on stderr, and the tokens per 1000 records (to size the full run). Suggest
  running on a sample first:
  `head -200 big.log | deem --stats "Q?" ...`.
- Judge only what each question needs (`.body`, not `.`; `.body[0:2000]`
  for long text): fewer tokens, often better answers, and less room for a
  hostile record to argue.
- `group` costs up to a request per line per group, so a strict question
  on varied lines (nearly every line its own group) is its worst case. It
  stops at 20 groups and prints the lines left under `-`; `--arg groups N`
  sets another limit. Try a looser question or fewer lines first.
- `--max-questions N` caps a run (default 100000; 0 is no limit). It is a
  safety stop, not a sample: a capped run is partial (exit 2), no more
  records are read, and which records finish depends on timing, so it can
  differ between runs. To judge a sample, cut the input with `head`.
- deem sends one value per request, with every question about it (about
  80 values a second, the hosted API's default limit for an organization;
  an API's `--rps` and `--tps` in `deem api` set others: a
  plan's higher limits, or room for other users of the same
  organization). A server of your own that runs a few requests at a
  time needs `--parallel N` at its number (llama-server's `--parallel`),
  or requests time out in its queue and are sent again. To see whether a local model is close
  enough to the hosted one, run the same programs against each with
  `--trace`, then `deem trace compare --with NAME FILE...`: each API's
  answers against NAME's, question by question: APIs side by side (of
  the values all answered, how many each answers as NAME does; under 20
  is a smoke test; what not all answered, like pictures one API doesn't
  take, in a table of its own), then each API, on the values it and
  NAME both answered: its AUC, mean difference, how many it
  says yes to where NAME says no and the other way, and a table of its
  choices against NAME's. `sh trace.sh API` in `examples/` traces the
  examples. A mean difference, or many more yes-where-NAME-says-no than
  the other way, says it needs another threshold. A large input runs at that rate: 100,000 short lines take
  about 21 minutes; there is no faster mode.
- `--trace run.jsonl`, then `deem trace run.jsonl`, shows each wave, which
  records and questions each request carried, and what each question waited
  for ("after #M"). Use it to explain why a program took
  more waves than expected. A run that hangs can be killed and its trace
  still read: it says which request was still out. Each `request` event
  also holds the body as sent (`.body.state` is the value judged), and
  each `answered` event the API's `response` as received, answering each
  question by its name (`q0`, …): use them to show exactly what the model
  saw and said.

## Which API a run sends to

`deem api` lists the APIs in `~/.config/deem/config.toml` (`*` marks the
default); `--api NAME` (or `DEEM_API=NAME`) sends a run to another, such
as a local server that speaks the Jev API. Only an API added with
`--pictures` is shown pictures: a picture run against one without stops
before sending, so pick the API before blaming the program. With APIs
added, `JEV_API_KEY`, `JEV_BASE_URL` and the other `JEV_` variables are
refused; without any, they set the API. For one run against another
server, `XDG_CONFIG_HOME=/nonexistent JEV_BASE_URL=… deem …` leaves the
user's APIs alone. A model on the user's own machine is Kev (text) or
vllm-d3-mini (pictures too, behind a proxy) on llama-server, not Jev: its
scores differ, so a threshold set on Jev needs checking there (the
manual's "A server of your own"). A URL is `https://`, or `http://`
only to this machine (`localhost`, `127.0.0.1`, `[::1]`), with no
password in it (the key goes in with `--key`, and is kept in
`~/.local/state/deem/keys`); moving an API with a key
to another server takes `--key` or `--no-key` too; a config file
or folder others can change is refused (`chmod go-w` it, as the error
says). `box` needs Jev 1.14
(`jev-latest` was on 2026-10-22); a model that refuses it gets
`deem api edit NAME --model jev-preview`, or `JEV_MODEL=jev-preview`
without APIs.

## Checking a program before handing it over

If the deem binary is available, compile-check the program without
sending anything. With empty input no question is asked, and a compile
error is printed with line:column. The fake key and the closed port make
sure that even a program that does ask (an `input: "all"` program runs
once on `[]`) can't reach the API; keep both. `XDG_CONFIG_HOME` names a
folder that isn't there, so a config file of the user's isn't read (it
would refuse the variables), and saved programs are found as usual:

```bash
XDG_CONFIG_HOME=/nonexistent JEV_API_KEY=x JEV_BASE_URL=http://127.0.0.1:9 deem run 'PROGRAM' </dev/null; echo $?
```

Exit 1 with no output means it compiled (an `input: "all"` program runs
once on `[]`, so it may print). For more, see the deem repository's
`docs/MANUAL.md`: "Program files" (arguments, `input: "all"`),
"Programs that come with deem", "When an answer fails", "Gotchas" and
"Differences from jq".
