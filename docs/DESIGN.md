# Design

How deem turns a jq program into requests to the model, and the rules for
what it sends and when. [`docs/MANUAL.md`](MANUAL.md) is the user's
reference; this file is how it works inside.

Three ideas carry almost everything:

- **Waves**: `ask` doesn't wait for its answer. A record runs until it
  can't go on without an answer, then sends everything it has asked so far
  together: a *round*. Wave k is every record's k-th round.
- **Replay**: once its round is answered, the record runs again from the
  start, with those answers. It redoes the whole computation, but none of
  its effects: nothing is asked, printed or written twice
  ([Replay: running a record again](#replay-running-a-record-again)). The
  run that has every answer is plain jq.
- **One value per request**: a request carries one judged value, in its
  state, and every question about it, so an answer depends on that value
  alone ([Requests](#requests)).

---

## Terms

The words the docs use, each in one sense. TRANSPORT.md and the manual use
these too.

- **Run** (of deem): one `deem` command, from start to exit; also called
  an *invocation*. "Once per run" means this.
- **Record**: what the program runs on once: an input line, a JSON value
  with `-J`, or a picture file. With `input: "all"`, the array of every
  record is one record.
- **Program**: the jq program deem runs on each record. `deem "Q?"`, `-p`
  and `-o` are one-line programs.
- **A record's run**: one evaluation of the program on a record, from the
  start, with the answers the record has so far. A record runs until one
  of its runs ends clean ([Replay](#replay-running-a-record-again)).
- **Clean**: a record's run is clean until its first question without an
  answer. Only what a clean run does is committed.
- **Question**: one call of `judge`, which `ask`, `choose`, `rate` and
  `box` make: what to ask, and the value to judge.
- **Value** (the judged value): the last argument of `ask`, all the model
  sees of the record; a request's `state`.
- **Answer**: the model's reply to one question, `{value, confidence,
  probabilities}`; `ask` gives its `value`. A **failed answer** is one
  that can't be had; it is a jq error at its `ask`.
- **IOU**: what `ask` returns in a run that has no answer to its question.
  Looking inside one stops the run.
- **Journal**: what a record's runs have committed, which later runs repeat
  instead of doing again: answers, outputs printed, lines written to
  stderr, times read.
- **Round**: the questions a record's run asked and has no answer to, sent
  together when the run stops.
- **Wave**: every record's k-th round, for one k. A run's waves are its
  round trips.
- **Request**: one HTTP call to the API, carrying one value, in its state,
  and the questions about it in one round. A request that shows pictures
  carries them as file parts of a `multipart/form-data` body.
- **Attempt**: one sending of a request; a retry is another attempt of the
  same request.
- **Memo**: the run's answers, by model, value and question, so that a
  question is asked once per run.
- **Window**: how many records are in progress at once: 192 at 80 requests
  a second.
- **Worker**: a thread that runs records (one per core, at most 8); each
  record stays on one. **Sender**: a thread that sends requests, within
  the rate.
- **Abort**: deem gives up on the run before its input is done: a setup
  error, a rejected key, 10 requests in a row with no usable answer, or the
  output closing (`| head`). It reads no more input, sends nothing more, and
  exits with 2 (141 for a closed output). In the code, `Batcher::stop`.
- **Halt**: the question cap (`--max-questions`) is reached: no new record
  starts and no new question is asked, but what was asked within the cap
  is still sent. Not an abort.
- **Picture reference**: how a picture appears in a program, a string
  `<image:TOKEN:PATH>`; deem replaces it with the picture when it sends a
  request, and with its path when it prints.
- **Threshold**: the score above which `deem "Q?"` keeps a record (`-t`,
  0.5 by default).

---

## The big picture

```
  input ──▶ reader ──▶ workers run each record ──▶ questions queue up
                       (the jq program, from              │
                        the start each time)              ▼
                              ▲                  a run that stops: a round
                              │                           │
                     a round answered:              a request per value
                     its record runs again                │
                              │                           ▼
                              └─────────────────── 96 senders ──▶ API

  output ◀── main thread prints each record's results in input order
```

- The **reader** splits the input into records and hands each to a worker,
  at most a window of them at a time: 192.
- The **workers** (one per core, at most 8) run the program on the records.
  A run never waits: `ask` gives the answer if the record has it, and
  otherwise queues the question and carries on. The run stops when the
  program needs an answer the record doesn't have (or, rarely, at a limit:
  see [Limits](#limits)), and the record waits, as data, until its round
  is answered; then it runs again.
- The **batcher** keeps the questions until their record's run stops, then
  builds their requests.
- The **senders** (96 threads at the default rate) send requests within the API's rate limits
  and retry what can be retried.
- The **main thread** prints outputs in input order.

---

## Requests

A request has one `state` and any number of questions. The judged value
*is* the state, and every question about it goes in the same request, as
the program wrote it:

```json
{
  "model": "jev-latest",
  "state": "api-7f4 ERROR upstream timeout",
  "questions": {
    "q0": {"type": "noul",   "instructions": "would a user notice?"},
    "q1": {"type": "choice", "instructions": "which service?",
           "criteria": {"api": null, "db": null}}
  }
}
```

This is how the API is meant to be used. Its guide says to "ask many
narrow, independent questions about the same state in one request", that
"state contains the content and supporting facts; questions define the
judgments", and to "include only the context relevant to the current
questions. This helps the model avoid distractions and context rot"
(docs.typesafe.ai, "How to build with System One", "State"). Its cookbooks
judge many items with one request per item. A question can name a field
of the value in backticks (`` `photo` ``), as the API's examples do.

The value is the whole state, not under a name: against
`{"records": {"r0": value}}`, with each question beginning "Considering
only `records.r0`:", the bare state measured as well or better
(tests/live, 2026-10-16).

### Request rules (`transport/request.rs`)

- **A value is sent once per request**, however many questions in that
  round are about it. A question asked in a later round (a cascade) sends the
  value again.
- **What is sent is the value you judge**, exactly as `tojson` prints it
  (but for pictures, below):
  `ask(q; .)` sends the record, `ask(q; .msg)` sends only `.msg`. Two values
  with the same bytes are the same value, even from different records, so
  `ask(q; .msg)` and `ask(q; .title)` are two requests, and the same line
  twice in the input is one.
- **The judged value only goes in `state`**, as the whole state. The API
  takes a string, an object or an array as a state, so any other value
  (`42`, `true`, `null`) goes as its text (`"42"`). The question's text
  goes into the instructions as the program built it, so `ask("is a
  \(.level) bad?"; .msg)` puts `.level` there (the manual's
  [Safety](MANUAL.md#safety) section says what to do about it).
  `like`/`unlike` examples go in the question's criteria, beside what
  yes and no mean, as the API's docs write them:
  `{"true": {"what": "…", "examples": […]}, "false": {"examples": […]}}`
  (`yes:`/`no:` alone go as `{"true": "…"}`). Measured level with the
  other places tried (`like_and_unlike`, 2026-10-19).
- **A value is at most `DEEM_MAX_RECORD_BYTES`** (32 KiB) when judged: a
  longer string is cut, anything else is an error at the call. The model
  takes 32,000 tokens of state and the longest question, which 32 KiB
  fits unless it is denser than about a byte a token (digits, hex); such a
  request is refused, with the API's reason. For the token rate, a
  request's tokens are estimated from its bytes at the ratio measured from
  the API's replies (a log line is about 2 bytes a token, English prose
  4.6).
- **A round is sent in the order its record asked**, so the same input and
  answers make the same requests every time, whatever order the records
  ran in.

### Pictures

A picture file named on the command line (a PNG or JPEG, known by its first
bytes, which no UTF-8 text starts with) is one record; `--img NAME FILE`
binds one to `$NAME`, and `-I` reads pictures' paths from stdin. A program
holds a picture as its *reference*, the string `<image:TOKEN:PATH>`, TOKEN
random for each run: jq values are JSON, and by the time `judge` gets its
value the variables are gone, so deem recognises a picture by the value
itself; the token means data can't write one. The engine never sees
anything new.

How a picture travels, from the program's value to the API, one step a
line (the step on the right):

```
  .  =  "<image:9f2c…:cam/1.jpg>"                   the program holds it
  {photo: ., claim: $c}  →  judge                   the run checks each
                                                    reference is a whole
                                                    string
  state {"photo":"<image:…>","claim":…}             the batcher keys and
                                                    groups the question
                                                    by this text
  state {"photo":"6c1e…","claim":…}                 the sender swaps the
  + assets {"6c1e…": {"type":"image",               reference for an ID,
                      "part":"6c1e…"}}              a hash of the bytes,
                                                    written as a UUID
  multipart: the request, then file part "6c1e…"    the client sends it,
                                                    with the images header
```

- **A reference is a whole string**: the value, a field or element in it,
  an option's description, a Score level, an example. One inside a longer
  string, or in the question's text, is an error at `judge` (the API would
  show the picture there, but its docs put pictures in the state, named
  in backticks, and the token would otherwise reach the model as text).
  A value that is one picture alone goes as
  `{"image": ref}`, the state the docs show.
- **Read when sent.** The reader reads only a picture's header, to check
  the API's limits (280–1568 px a side, 16 MiB, PNG or JPEG) and to count
  tokens (width × height / 390 a use). The sender hashes the file the
  first time a request shows it, checks its header again, and streams it.
  A question never sent never reads its picture.
- **Deterministic.** The ID is a hash of the bytes, not a fresh UUID: the
  docs suggest a new UUID per request so a reference can't match other
  text, and deem checks that directly, as it builds the request. So the
  same input and files make the same requests, as text does.
- **Printed as its path.** Output never shows a reference: each is
  printed as its picture's path, so a picture record prints as the file
  that was named.

### Why one value per request

A request has one state, and values that share it move each other's
answers: 24 values in one state, each question naming its own, moved
scores 0.07–0.08 against one value per request, and flipped 142 of 200
decisions on near-copies of log lines (tests/live, 2026-09-29). The
shifts scattered, so no threshold corrected them. So a run goes at the
request rate, about 80 values a second by default (ISSUES.md, "Large
inputs run at the request rate").

---

## Asking without waiting

In plain jq a function returns a finished value. deem's `ask` returns the
answer if its record already has it; otherwise it returns an **IOU**: a
value that says "the answer will be here". The program keeps going with the
IOU until it needs to look inside it. There the run stops ([Running
again](#replay-running-a-record-again)).

jaq lets deem do this because it only touches values through a trait
(`ValT`), so deem's value type is either a normal JSON value or an unknown
one (`engine/val.rs`):

```rust
enum Val {
    Ready(jaq_json::Val),   // a normal JSON value
    Unknown(u64),           // an answer this run doesn't have, or something built from one
}
```

Some operations only **move a value around**, so an IOU stays an IOU.
Others must **look inside it**, and stop the run:

| keeps it an IOU | needs the answer: the run stops |
|---|---|
| building `{a: …}` and `[…]` | comparing: `==`, `<`, `sort` |
| `.key`, `.[0]` | truth: `if`, `select`, `and`, `or`, `//` |
| `"\(…)"`, `tostring` | iterating: `.[]`, `length`, `test` and other builtins |
| arithmetic: `+`, `-`, `*`, `/` | `@csv`, `@tsv`, … |
| `as $x`, `.[] \|= …` | using it inside another question |
| | printing it |

An object or array that contains an IOU is itself an IOU. Looking inside
an IOU is never a jq error: no `try`, `?` or `.[]?` sees it, and the run
just stops. An error there would let `try` take a branch the run with the
answer wouldn't.

A picture is not an IOU: in a program it is a string,
`<image:TOKEN:PATH>` ([Pictures](#pictures)), so a normal JSON value from
the start. What may be unknown is an answer, about a picture or about text
alike; the picture itself is only read when its request is sent.

A run goes on past its first IOU for up to 64 outputs before it stops,
printing none of them. So `.[] | ask(q; .)` queues a question for each
element first, and they go out together. Without that, each element would
be its own round. The 64 keeps a program with endless output bounded.

---

## Replay: running a record again

**The problem.** A run of the program reaches a point where it needs an
answer that hasn't arrived: it compares it, tests it, prints it. jaq
evaluates a program in one go; it can't pause partway and resume later.
So deem has two choices: keep the half-finished computation alive until
the answer comes (a thread blocked in the middle of jaq), or throw it away
and start over once the answer is here. deem starts over. That is
*replay*.

**Why starting over is safe.** A program depends only on its record and
its answers ([What a program depends on](#what-a-program-depends-on)).
Run it again with the same answers and it does the same things, in the
same order, up to the point where it stopped; then, with the new answers,
it gets further.

**The cycle**, for each record:

1. **Run** the program from the start. `ask` gives the answer if the
   record has it. If not, it queues the question and returns an IOU, and
   the run carries on ([Asking without waiting](#asking-without-waiting)).
2. **Stop** when the run needs an answer it doesn't have: it looks inside
   an IOU. (Also when it has 64 unanswered questions, or has gone 64
   outputs past its first unanswered question, so a run's look-ahead is
   bounded.) A run that ends with a question it hasn't had answered isn't
   finished either.
3. **Send** every question the run queued, together: the record's next
   round ([When a round is sent](#when-a-round-is-sent)).
4. **Wait** until the whole round is answered. The record waits as data
   (its input, its journal, its answers), not as a thread.
5. **Run again**, from step 1, now with more answers.

A run that ends without ever having met an unanswered question is the
record's last. It is plain jq on known answers, and its outcome (outputs,
an error, `halt`) is the record's.

**Clean runs and the journal.** A run is *clean* until its first question
without an answer. Every effect of a clean run happens for real, and is
written in the record's **journal** (`engine/mod.rs`). A later run
recomputes everything, but where it reaches an effect the journal has, it
uses the journal instead of having the effect again:

| the journal keeps | so that a later run |
|---|---|
| the answers `judge` gave, by its arguments | gets the same answer to the same question, without asking anyone |
| how many outputs were printed | skips that many: nothing is printed twice |
| how many lines went to stderr (`debug`, `stderr`) | skips that many |
| the times `now` read, in order | gets the same times |

After its first unanswered question a run only **looks ahead**: it queues
the questions it meets, so that independent questions go out together,
but it prints nothing, writes nothing, and keeps nothing. Its outputs are
thrown away with it. This is what makes the journal sound. Up to its first
unanswered question, a run has the same answers to the same questions as
the run before it, so it repeats exactly what that run committed. What
happens after it may change once the answer is known (the answer may
fail, a comparison may go the other way), so none of it is committed.

**An example**, one record:

```
  debug | select(ask(q1; .) > 0.5) | "kept", ask(q2; .)
```

| run | answers it has | what it does | ends | journal after it |
|---|---|---|---|---|
| 1 | none | writes the `debug` line; asks q1, gets an IOU; `> 0.5` must look | stops: round 1 is q1 | 1 stderr line |
| 2 | q1 = 0.9 | skips the `debug` line; q1 from the journal; passes; prints `"kept"`; asks q2, gets an IOU, and outputs it, unprinted | ends with q2 unanswered, so not the last: round 2 is q2 | q1; 1 output; 1 stderr line |
| 3 | q1, q2 | skips the `debug` line and `"kept"`; prints q2's answer | ends clean: the last run | q1, q2; 2 outputs; 1 stderr line |

What was printed: the `debug` line once, `"kept"` once, q2's answer once,
the same as jq with the answers given at once. What was sent: two rounds,
two waves.

**What it gives.**

- **jq's semantics.** The run that prints has every answer, so a failed
  answer is a jq error where it was asked, which `try` catches
  ([When an answer fails](#when-an-answer-fails)), and arithmetic on an
  answer fails inside its `try`. `tests/deferred.rs` checks that programs
  print the same as with the answers ready at once, with no exceptions.
- **Rounds that don't depend on timing.** A run asks what its program, its
  record and its answers make it ask, and a record waits for its whole
  round before it runs again.
- **No thread per waiting record.** A few workers run every record in
  flight ([The big picture](#the-big-picture)).

**Checks.** Two things would mean deem is broken, and each ends the record
with an internal error instead of a wrong output or a hang: a run that
doesn't repeat what the journal holds, and a run that stops with no
question to wait for (it would run the same way again, for ever).

**What it costs**, measured against a stand-in API that answers in 20 ms
(2026-10-20):

- **jq work, again in each run.** A record with d rounds runs d + 1 times,
  and each run redoes the work before its new questions. For a line that is
  microseconds. For one record with many dependent rounds it grows as the
  square of the rounds: `group` on 300 lines in 40 groups used 1.4 s of CPU
  where waiting on a thread per record used 0.4 s; a tree of 511 nodes with
  a `select` at each, 1 s against 0.04 s; one record streaming 20,000
  questions, 64 a round, 10 s against 0.2 s. A round trip to the API is
  about half a second, so this is about 0.04% of a run per round: it shows
  only with hundreds of dependent rounds in one record. Live, `group` on
  the manual's 300-line sample made 194 groups, so 193 rounds: 38.7 s of
  CPU in a 469 s run, 8% (tests/live/README.md, "Measured by hand").
  Asking everything first and looking after makes it two runs.
- **Memory.** The journal keeps every answer the record has had until the
  record ends.
- **A question the program asks is sent**, even if nothing looks at its
  answer: its failure is a jq error at the `ask`, so the run that has every
  answer needs it.
- **`now` read after an unanswered question is read again** by the run that
  has the answer, so a question built from it is asked twice, the first
  time for nothing.

---

## Waves

A wave is one round trip: every record's round of that depth. The program
decides how many waves it needs, through what it looks at and when. Nobody
analyses the program; running it finds out.

**Independent questions: one wave.**

```
  {a: ask(q1; .), b: ask(q2; .)}

  ask q1 ─▶ IOU ┐
  ask q2 ─▶ IOU ┴─▶ print: needs both ─▶ wait        wave 1: q1, q2
```

**A question behind a filter: two waves, and the second only for the
records that passed.** This is a *cascade*: put the cheap or strict filter
first and the second question is asked less.

```
  select(ask(q1; .) > 0.5) | ask(q2; .)

  ask q1 ─▶ IOU ─▶ `> 0.5` needs it ─▶ wait          wave 1: q1
                   passed ─▶ ask q2 ─▶ print ─▶ wait  wave 2: q2
                   failed ─▶ record ends, q2 never asked
```

**A question that contains an answer: two waves.**

```
  ask("Given \(ask(a; .)), …"; .)                   wave 1: a,  wave 2: the outer question
```

**Looking too early costs waves.** Here each `select` looks at its answer
before `map` moves on to the next element, so each element is its own wave:

```
  .xs | map(select(ask(q; .) > 0.5))               one wave per element
  .xs | map(ask(q; .)) | map(select(. > 0.5))      one wave: ask everything, then look
```

Waves count depth, not moments: wave 2 is every record's second round,
whenever each record got there. One record's second round can go while
another's first is still out.

**Example:** 500 records × `{notice: ask(…), severe: ask(…)}`. Each record
asks both questions and waits: one round, a request with its text once and
both questions. The window runs 192 records at a time, and as they finish
the next ones start: 500 requests, all in wave 1.

---

## When a round is sent

A record's questions go when its run stops: every question the run asked
that the record has no answer to goes, as its next *round*, a request for
each value. The record waits until the whole round is answered, then runs
again. That's the whole rule (`Asker::send` in `transport/asker.rs`). No
record waits for another, and no timer decides anything.

**Wave k is every record's k-th round.** A question that had no answer
when a run asked it stays unanswered to that run, however soon its answer
comes, so what a run asks follows from its program, its record and its
answers; so do the record's rounds, and the run's waves and requests. Two runs that get the same answers
make the same waves and the same requests, however the answers' timing
differs. The tests count them exactly; `deem trace` groups each wave's
rounds together.

```
  time ──────────────────────────────────────────────▶
  record 1   round 1 ├──────┤ round 2 ├──────┤             waves 1, 2
  record 2   round 1 ├─────────────────────────────┤ (slow)
                            └ record 1 goes on: its round 2 doesn't wait for record 2's
```

A round is answered when its last request is back, and its record runs
again at once, while the rest keep waiting.

What this means in practice:

- **With streaming input** (`journalctl -f`), a line's question goes the
  moment its record's run stops, however much more input is coming.
- **A record busy computing holds back only the records on its worker**,
  and only while it computes.
- **Questions about the same bytes from records running together** go in
  each record's own request. A repeat, the same question about the same
  value, is still asked once: whichever record's run stops first sends it.
  The requests don't depend on which record that is; which wave the repeat
  is counted in can.

Why not gather every record's questions into one send: a request carries
one value, so it would save almost no requests, and when it went would
depend on when answers came back, and so would the waves.

---

## What is sent, and what isn't

When a round goes, its questions pass through these rules:

- **Identical questions are asked once per run.** "Identical" means the same
  model, the same value bytes and the same question, keyed by a hash of the
  request that would ask it alone, so the other questions asked of the
  value don't change the key. A repeat reuses the first answer, even if it is still on its way. Key order
  in an object matters: `{"a":1,"b":2}` and `{"b":2,"a":1}` are different
  values to the model. (Past 100,000 remembered answers, which takes a
  raised question cap, the ones no running record holds are dropped, and a
  repeat of one is asked again.)
- **A question the program asks is sent**, even if nothing looks at its
  answer: its failure would be a jq error at the `ask`. `ask(q; .) as $x |
  "done"` sends one. A question a run asked while looking ahead, and that
  no later run asks (an earlier answer failed, say), is sent for nothing:
  at most a round's worth. A record that ends without its round going (the
  output closed, the run halted) drops what it queued, and a later repeat
  is asked afresh.
- **The question cap** (`--max-questions`, default 100,000) counts questions
  queued for the API. Repeats are free and don't count. When a question
  would go past it, the run *halts*: no new records start, and any record
  that asks a new question from then on stops and goes to `--failed`. What
  is already queued within the cap is still sent, and records that need
  nothing more finish. Exit 2.

The cap is a safety stop, not an exact budget. Records run in step, so when
it is reached many are part-way through; in `select(ask(q1; .) > 0.5) |
ask(q2; .)`, records whose `q2` is refused have already paid for `q1`. At
most one round of such earlier questions is lost per record (up to a
window of records). Which records finish in a capped run depends on
timing: a record that hasn't asked by the time another uses the cap up
doesn't get to; the
records in `--failed` plus the unread rest of the input are the rerun.

---

## Sending

A round's requests go in a queue that the sender threads take from, oldest
first (`transport/sched.rs`). [TRANSPORT.md](TRANSPORT.md) states the rules
this part keeps (what is retried, when deem gives up, what is bounded) and
the tests that check them. In short:

- **Rate.** Every attempt, retries included, takes from two budgets: at
  most `JEV_REQUESTS_PER_SECOND` requests (80) and `JEV_TOKENS_PER_SECOND`
  input tokens (100,000) a second (an API's `rps` and `tps`, from the
  config file), the hosted API's default limits for an
  organization, each with a burst of one second's worth. The request budget
  halves when the API answers 429 or 529 and grows back while requests
  succeed, and a `Retry-After` pauses every sender: two deems on one
  organization settle on sharing it.
- **Retries.** Each failure is classified once (`transport/api.rs`,
  `Kind`): a wrong URL, key or model aborts the run on the first reply; a
  busy, failing or silent API is retried with backoff; a refused request
  or a broken reply is not, and fails only its own record's questions.
- **Giving up.** Ten requests in a row with no usable answer abort the run.
  After an abort nothing more is sent: queued requests are dropped, and
  waits for a turn, a `Retry-After` or a backoff end at once.

---

## Limits

Everything deem holds is bounded ([TRANSPORT.md](TRANSPORT.md), rule 3):
the memo of answers too, which past 100,000 entries drops the ones no record
holds. Questions left queued by records that ended without sending them
are dropped when the last of those records ends. One thing grows with a
record rather than the input: its journal keeps every answer the record
has had until it ends, as many as the question cap allows.

| what | limit | what happens at it |
|---|---|---|
| records running | the window: 192 | the reader waits for one to finish |
| records started but not yet printed | 4 windows: 768 | the reader waits for the main thread to print one |
| unanswered questions per run of a record | 64 | the run stops, and its round goes |
| outputs a run looks ahead | 64 past its first unanswered question | the run stops, and its round goes |
| a record's outputs ahead of printing | 16 chunks of 64 | the record being printed waits for the main thread; any other pauses, and runs again when its turn comes |
| requests in flight | 96 at 80 a second, at most 256, at most `parallel` if set | one per sender |
| request rate | 80/s and 100k tokens/s, or as set | senders wait |
| questions per run | 100,000 | the run halts (above) |
| a record read | 64 MiB | reading stops with an error; records already read finish |
| a value judged | 32 KiB | strings are cut, other values fail |
| a picture | PNG or JPEG, 280–1568 px a side, 16 MiB | its record fails (`--img`: the run stops), before anything is sent |
| a request with pictures | 32,000 tokens of the value and any one question, 64,000 in all | refused before it is sent |

**Why two limits on starting a record.** Output is printed in input order,
so a record that finishes early holds its output until the ones before it
print. A record frees its window slot when it *finishes*, not when it is
printed, so the window alone doesn't stop records piling up behind one slow
request. The second limit (4 windows) does: past it, deem waits instead of
growing. Freeing the slot at print time instead would stall the whole run
behind any slow request.

**Why these numbers.** They follow from the request rate,
`JEV_REQUESTS_PER_SECOND`, or the API's `rps` (`Config::new`). The senders are enough to
keep that rate going at up to 1.2 s latency: 96 at 80 a second (the live
API measured 0.3–0.9 s), and at most 256, so a high setting can't ask for
thousands of threads. A server of your own that runs only a few requests
at once says so with `parallel` (`JEV_PARALLEL`), and the senders are at
most that many, so no request waits in its queue (TRANSPORT.md, rule 8).
The HTTP client keeps a connection open for each
sender, so requests don't each open a new connection.

**The window is two requests' worth of records for each sender**: 192
records at 80 a second. It doesn't set how fast deem sends (the senders'
budgets do); it sets how many records have asked and are waiting. It has
to keep every sender busy, and anything beyond that only queues behind the
senders, where a record's second question waits for every other record's
first. Against a stub API (2026-09-30, 20 requests a second, 24 senders,
480 records), two requests' worth a sender took as long as a window of all
480, and a two-question chain printed its first output after 2.8 s instead
of 24.0 s. At 80 a second the window of 192 kept the rate: 2,000 log lines
went at 82 a second against the live API (2026-10-19).

**`input: "all"` programs** are one record holding every record, so they
get the unanswered questions a whole window of records could have, 64 each,
instead of 64: `[.[] | ask(q; .)]` over 1,000 lines is still one wave.

---

## What a program depends on

A program's output depends on three things: its record, the model's
answers to its questions, and the clock, if it reads it. Nothing else gets
in, and its only effects are its output and what it writes to stderr.

```
   record ──────┐
                ▼
         ┌────────────┐
         │ jq program │ ───▶ output
         └────────────┘
           ▲        ▲
   answers ┘        └── the clock, if it reads it
           ▲
 ──────────┼─────────── from deem, only answers
           │
         ┌────────────┐
         │    deem    │   memo · waves · retries · failures
         └────────────┘
                │
               API
```

jq's builtins can reach more than that, because a command-line tool finds
it useful. deem closes what could leak secrets or tie records together:

| a jq program could | in deem | because |
|---|---|---|
| read files (`import`, `include`) | refused when the program is compiled | a file's contents could end up in a question |
| read the environment (`env`, `$ENV`) | `{}` | it holds secrets, `JEV_API_KEY` among them |
| read other records (`input`, `inputs`) | not defined | records run in parallel, and failed ones are rerun alone |
| stop the whole run (`halt`) | ends only its record | one record can't end the others |
| read the clock (`now`, `localtime`, `strflocaltime`) | kept | it ties no records together; a program can put the time in a question and send it |
| write to stderr (`debug`, `stderr`) | kept, once per record however often it runs | only you see it |
| ask the model | only through `judge` | and only questions the program wrote |

**One answer per question per run.** deem remembers every answer for the
run, keyed by the model, the value judged and the whole question (its
text, options, criteria and examples). Looking a question up and, if it is
new, storing a promise for it are one step under the batcher's lock, so
two records asking the same thing at once share one promise, and one
request goes out. Within a run, a question can't get two different
answers. (Past 100,000 remembered answers, the ones no running record
holds are dropped, and a repeat of one is asked again.) Within a record
the journal keeps it too: each run of the record gets the answer its
first one did, whatever the memo has dropped.
`one_answer_per_question_per_run` in `tests/waves.rs` checks it against a
fake model that never answers the same way twice.

What this makes possible:

- **Asking early can't be seen.** deem doesn't run a program the way it
  reads: an `ask` doesn't wait, the program runs on, and answers are
  fetched later, several at once ([Asking without
  waiting](#asking-without-waiting)). That gives the same output as
  waiting at each `ask` only if the program can't tell when things
  happened, and it can't: it sees no files or other records, a question
  has the same answer whenever it is asked (above), and the API judges
  each question on its own, so what else is in its request doesn't change
  the answer. `{a: ask(q1; .), b: ask(q2; .)}` builds the same object
  either way. The clock too: `now` is read by the first run that gets to
  it with every answer before it, which is when waiting at each `ask`
  would read it. `tests/deferred.rs` checks that programs print the same
  with answers that arrive later as with answers ready at once.
- **A failed record can be rerun alone.** A program can't read other
  records and does nothing but print, so a record from `--failed`, run on
  its own, computes what it would have in the full run, and repeats no
  effect. Three things can differ: the answers, by about 0.01 between
  runs; the lines the record printed before it failed, printed again; and
  the cost of its questions, paid again.

What the output can still depend on, besides the record:

- **The answers**, which can differ between runs: two runs over 150
  messages moved scores by about 0.01 (2026-09-29). Within a run they
  don't.
- **The time and the time zone**, for a program that reads them.
  `todate`, `gmtime` and `strftime` are UTC and depend only on their
  input.
- **Picture files**, which deem assumes don't change during a run: the
  memo knows a picture by its reference, so a file changed after its
  first question keeps that answer.

---

## When an answer fails

A failed answer is a jq error where it was asked, as if `ask` had called
`error`. The run that prints has every answer, so it is plain jq: `try` and
`?` catch the failure, and uncaught, it fails its record; the other records
are unaffected. (`//` doesn't catch it: jaq's `//` passes errors on.)

```
  try ask(q; .) catch 0

  run 1   ask q ─▶ IOU ─▶ ends with a question unanswered       round 1: q
  run 2   ask q ─▶ failed: an error here ─▶ caught ─▶ 0
```

Arithmetic on an answer fails where it is written, inside its `try`: the
run that gets that far has the answer as a plain value. A run that looks
ahead past an IOU never fails on it ([Asking without
waiting](#asking-without-waiting)), so no `try` can catch what the run
with the answer wouldn't.

Errors in the program itself are jq errors too: bad arguments to `judge`
are caught at the call. A failed record prints what it produced before the
error, then the message, and the run exits 2. Failed answers a program
caught are counted on stderr at the end of the run, since the output
can't show them.

---

## deem and jaq

deem has no jq interpreter of its own. It uses jaq, a jq interpreter
written in Rust, as a library: `jaq-core` runs programs, `jaq-std` is the
standard library, and `jaq-json` the JSON value. All three are used
unmodified, pinned to exact versions. So a program that asks no question
prints exactly what jaq prints; `tests/jq_diff.rs` checks this against
jq's own test suite. Where jaq differs from jq 1.7.1, deem follows jaq, and
the manual lists the differences
([Differences from jq](MANUAL.md#differences-from-jq)).

jaq is built to be embedded: the program using it chooses the value type,
can add or replace builtins, and decides how modules load. deem changes
things in those four places, and nowhere else:

- **Values that may not be here yet.** jaq runs on any value type that
  implements its `ValT` trait. deem's is either a JSON value or an answer
  the run doesn't have ([Asking without waiting](#asking-without-waiting),
  `engine/val.rs`).
- **Builtins written in Rust** (`engine/natives.rs`):
  - `judge`, the one builtin that asks the model;
  - `env` and `$ENV`, which are `{}`, so a program can't read secrets such
    as `JEV_API_KEY`;
  - `fromjson`, `debug`, `stderr` and `now`, replaced: jaq's `fromjson`
    reads several values and loops on some invalid text; `debug` and
    `stderr` print as jq 1.7 does; and `debug`, `stderr` and `now` go
    through the record's journal, so each happens once however many
    times the record runs ([Replay](#replay-running-a-record-again));
  - `tojson`, `length`, `has` and four others from `jaq-json`, which work
    only on plain JSON: an IOU in their input stops the run, and the run
    with the answer runs them.
- **Builtins written in jq**: `ask`, `choose`, `rate` and `box`, each a
  one-line call to `judge` (`engine/prelude.jq`), and jq 1.7 builtins jaq
  lacks, such as `@tsv`, `@csv`, `IN` and `tostream`
  (`engine/compat.jq`).
- **Loading modules**: `import` and `include` are refused, so a program
  can't read files ([What a program depends on](#what-a-program-depends-on)).

**What deem relies on.** jaq expects looking inside a value to be instant;
an answer may not be here yet. Two things make that work, neither promised
by jaq, each checked by tests that a jaq upgrade breaking it would fail:

- **A run can be stopped anywhere, and run again.** Looking inside an IOU
  unwinds the run (a Rust panic, caught where the run started,
  `engine/mod.rs`), as jaq can't pause a computation and resume it later.
  jaq keeps nothing between runs, and evaluates a program the same way
  each time (an object keeps its keys' order), so a rerun takes the same
  path up to its first unanswered question; the journal's checks say so if
  it doesn't. `tests/deferred.rs` checks that programs print the same with
  answers that arrive later as with answers ready at once, and
  `tests/replay.rs` that an effect of a clean run happens once;
  `panics_unwind_in_release` keeps the release build able to unwind.
- **A program's waves depend on how jaq evaluates it.** jaq builds
  `{a: x, b: y}` as `{a: x} + {b: y}`, so `+` on an answer stays an IOU;
  otherwise `[.[] | {k: ., v: ask(q; .)}]` would take a wave per element.
  `tests/waves.rs` counts the waves (`objects_in_arrays_one_wave`,
  `forced_before_next_question`).

---

## Programs

Everything deem runs is a program: `deem "Q"`, `-p` and `-o` are one-line
programs (`programs/forms.rs`), the built-ins are `.deem` files in
`src/programs/`, and users' are in the nearest `.deem/` from the current
directory up, or in `~/.deem/`. One engine path runs them all.

- The first line may be `module {input: "each" | "all", defaults: {…}};`.
  `"each"` (default) runs once per record; `"all"` runs once on the
  array of all records.
- A program that is only definitions is a *tool* and must define `main`.
  `main`'s parameters are its arguments, a fixed number, so deem knows where
  arguments end and files begin.
- Arguments and named values (`--arg`, `-t`) are bound as variables
  (`$__0`, `$threshold`), never pasted into the source.
- Tools print strings raw.
- `deem tune` is the one command that isn't a program: it asks the user
  questions on the terminal.

---

## Seeing a run

`--stats` prints totals. `--trace FILE` writes every event as a JSON line
as it happens: a record starts, asks, reuses an answer, waits, resumes,
ends; a wave goes; a request is sent and answered. Each question records
which earlier answers its record waited for since its last question, which
is the dependency that decided its wave. A request's event holds its body as
sent (pictures by their IDs, with the file each was read from), and its
answer's event the reply as received, so the trace is also the run's
requests and responses.

`deem trace FILE` reads it back grouped by wave and request, and
`--visual` draws it as a timeline in the terminal. `deem trace compare`
reads several, each of the API its first line names, and pairs their
answers by what was asked: a request's state and a question's text, kind
and options, so traces of different programs, files or days pair wherever
they asked the same thing. That is why a trace keeps the body as sent.

---

## Decisions

Decisions not argued elsewhere in this file.

| question | decision | why |
|---|---|---|
| how a record goes on after waiting | it runs again from the start, on one of a few workers ([Replay](#replay-running-a-record-again)) | jaq can't pause a computation; rerunning keeps jq's semantics, failures included, and needs no thread per waiting record (10,000 records: 2 s of CPU, against 35 s with a thread each) |
| a failed answer | a jq error at its `ask` | it's what jq would do with an input that couldn't be had, and `try` can catch it |
| identical questions | asked once per run | one question, one answer: records that ask the same thing can't disagree, and repeated lines (common in logs) cost nothing |
| output order | input order | output lines up with input: `paste` it beside the input, `diff` two runs |
| where jq and jaq differ | follow jaq, and document the difference (2026-09-24) | matching jq cost about 400 lines of overrides of jaq's builtins |
| built-in commands (`group`, `histogram`, …) | deem programs, no Rust per command | each can be read, copied and changed |
| how a picture is named | a picture file needs no flag; `-I` reads paths from stdin; `--img NAME FILE` binds one to `$NAME` | a picture file is known by its first bytes; a path can't be, so a list of paths is declared |
