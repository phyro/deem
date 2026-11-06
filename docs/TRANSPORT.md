# The transport's rules

The transport is the part of deem between a program's questions and the
API: it builds requests from questions, decides when they go, sends them,
retries what can be retried, and decides when to give up
(`src/transport/`). This file states the rules it must keep, how the code
keeps each one, and the test that checks it. [DESIGN.md](DESIGN.md)
explains how it works; this file says what must stay true when it changes.

## Terms

Record, question, request, attempt, run (an invocation), abort and halt
are as [DESIGN.md](DESIGN.md#terms) defines them. The question cap only
halts: it refuses new questions but sends the ones already asked (rule 7).

## Failures

Two questions decide what deem does with a failure:

- **Could trying again succeed?** Then retry, with backoff.
- **Would every other request fail the same way?** Then abort, instead of
  failing the rest of the input one record at a time.

Backoff starts at 250 ms, doubles, is capped at 5 s, and is up to a
quarter shorter at random, as in the official SDKs, so senders that failed
together don't retry together.

**Setup errors.** Every request would fail the same way: deem aborts on
the first one. The setting it names is the variable, or, for an API from
the config file, that API's (`check the key of API kev`;
tests/apis.rs, `setup_errors_name_the_api`).

| What went wrong | Examples | What deem does |
|---|---|---|
| The URL is wrong | a redirect (3xx), 404, 405, a malformed URL, a TLS certificate error | aborts, naming `JEV_BASE_URL` |
| The model is wrong | 400 `Unknown model: …` | aborts, naming `JEV_MODEL` |
| The key is wrong | 401 (invalid key), 403 (missing key) | aborts, naming `JEV_API_KEY` |
| A picture every request needs is wrong | an `--img` file gone or changed since deem read it; 400 `Images are experimental` (deem forgot the header) | aborts, naming the file or the header |

**API errors.** The API is busy or failing: deem waits and retries.

| What went wrong | Examples | What deem does |
|---|---|---|
| The API asks deem to slow down | 429, 529 | halves its rate; up to 20 attempts, while some request has succeeded in the last 60 s; waits `Retry-After` or `retry-after-ms`, at most 60 s |
| The API can't be reached | DNS, connection refused, no connection within 10 s | up to 6 attempts |
| The API failed or didn't reply | 408, 5xx, no reply within 60 s, connection dropped | up to 6 attempts |

**Request and answer errors.** Retrying can't help, and only the records
involved fail.

| What went wrong | Examples | What deem does |
|---|---|---|
| The API refused the request | any other 4xx, e.g. 400 `max_tokens_exceeded`, 400 `Invalid request.` | no retry; fails the request's questions |
| The reply is broken | not JSON, an answer missing | fails every question in the request |
| The answer doesn't fit the question | a score outside [0,1], an option that wasn't offered, a box upside down | fails that question |
| A picture can't be sent | an input picture gone or changed when its request is sent; a request over the token limit with its pictures | no attempt reaches the API; fails the request's questions |

Ten requests in a row with no usable answer abort, whatever the reason: a
cause that affects every request fails them one after another, while a
cause tied to some records is interrupted by the others' answers.

## Invariants

Each rule says what holds, **how** the code makes it hold, **why** that is
enough, and the **test** that checks it.

### 1. Bounded amplification

Attempts ≤ 20 × questions asked.

- **How.** A request is built only by `Batcher::send`, from questions taken
  out of the pending list at once, so each question enters exactly one
  request, and a request is never sent again as another. `Worker::send`
  counts attempts and stops at the limit for the failure: 20 at most.
- **Why.** No other path makes a request. A question answered from the memo
  makes none; a question asked again after it was dropped or swept from the
  memo counts as a new question.
- **Test.** `attempts_are_bounded`: a stub answers with a random mix of every
  failure; attempts stay within the bound.

### 2. Bounded lookahead

At most a window of records is in progress (two requests' worth for each
sender: 192 records at 80 requests a second), each with at most 64
unanswered questions; deem reads no further input until one finishes.

- **How.** The reader calls `Batcher::begin` before each record, which
  blocks while a window of records is in progress or 4 windows wait to
  print. A run of a record stops before its 65th unanswered question
  (`Asker::ask` gives `Full`), and its round goes; an `input: "all"`
  program, one record for the whole input, may have a window's worth.
- **Why.** Only records in progress ask, so pending questions and queued
  requests are bounded too, and so is how far spending can run ahead of the
  output.
- **Test.** `waiting_to_print_is_bounded`; `the_reader_stops_at_the_window`;
  `a_run_is_full_at_max`; `unanswered_questions_are_bounded`.

### 3. Bounded memory

Nothing grows with the length of the input.

- **How.** Everything kept per record or per question is bounded by rule 2
  or trimmed: output waits in 16 chunks per record and 4 windows of records
  (a record that isn't being printed pauses rather than wait for room, and
  holds at most one chunk more);
  the memo, past 100,000 entries, drops the answered ones no record holds,
  and sweeps again once it has doubled, so it stays within twice the larger
  of 100,000 and what records hold;
  the statistics are fixed-size histograms; the trace goes to its file; a
  record is at most 64 MiB when read.
- **Why.** The memo may only drop answered entries: a pending question's
  entry is how a record that asks it again finds it, and how a question
  nobody can read is recognised (`Demand::unread`).
  A picture's entry in the picture table is held by its record and the
  requests that show it, and swept once nothing holds it; a picture's
  bytes are streamed from its file when sent, never held whole.
- **Test.** `the_memo_keeps_what_is_held`; `statistics_are_fixed_size`: 100,000
  requests keep a few hundred numbers; `the_table_forgets_what_nothing_holds`.

### 4. Every question gets an outcome in bounded time

Each question ends with an answer or a failure within a bounded time, so
deem exits once its input ends and the program has finished each record.

- **How.** In four steps. A pending question is sent: the moment a run of
  a record that asked it stops, its round goes (`Asker::send`); a record
  that ends without its round going (the output closed, the run halted)
  drops it, and once every record that holds it has, no record can read
  it. Those requests go: the rate never goes below one
  request a second, and a `Retry-After` wait is at most 60 s. Every request
  ends: its attempts are bounded (rule 1), each attempt by a 10 s connect
  and a 60 s reply timeout (the whole call, a picture's upload included),
  and backoff by 5 s. When a request ends,
  `Batcher::resolve` sets the outcome of every question in it.
- **Why.** Each question passes all four steps, and each step is bounded.
- **Test.** `every_failure_ends`: each kind of failure, run to its end under
  a timeout; `a_stream_is_not_held_back`: at 30 lines a second, the first
  output comes before the input ends.

### 5. Nothing new is sent after an abort

- **How.** An abort (`Batcher::stop`) fails the pending questions and
  cancels the sender pool. A sender checks for the cancel before each
  attempt, before and after waiting for its turn. Every wait (the turn,
  `Retry-After`, backoff) goes through `Worker::pause`, which returns when
  the pool is cancelled.
- **Why.** Every path to the network passes that check. An attempt already
  on the wire still gets its reply.
- **Test.** `a_cancel_sends_nothing_more`; `an_abort_ends_a_retry_after_wait`.

### 6. A doomed invocation aborts early

A setup error or a rejected key aborts on the first reply that shows it;
any other failure that repeats aborts after 10 requests in a row with no
usable answer. Requests already sent by then still reach the API: at most
the senders' worth (96 at 80 requests a second).

- **How.** `api.rs` classifies each failure once, as in the tables above.
  Setup errors and a rejected key call `Batcher::stop` at once.
  `Batcher::resolve` keeps a streak: a request with no usable answer (a
  failure after its retries, a refusal, a broken reply, or every answer
  invalid) adds one; a usable answer resets it.
- **Why.** A wrong URL, key or model, or a server that speaks another
  format, fails every request, so the streak grows without a break. One bad
  record fails only its own requests.
- **Test.** `setup_errors_abort_at_once`; `nothing_usable_stops_the_run`;
  `a_rejected_key_stops_the_run`.

### 7. The question cap refuses new questions only

- **How.** `Batcher::ask` refuses a question past the cap and stops the
  reader, but fails nothing pending and cancels nothing.
- **Why.** It is a spending guard, not an abort: what was asked within it
  is worth its answer.
- **Test.** `the_cap_sends_what_fits` (the batcher); end to end,
  `the_cap_prints_or_fails_every_record`.

### 8. Rate

At most `JEV_REQUESTS_PER_SECOND` requests and `JEV_TOKENS_PER_SECOND`
input tokens a second per invocation (80 and 100,000 unless set, the hosted
API's defaults for an organization), and fewer while the API asks deem to
slow down.

- **How.** Every attempt takes from two budgets before it goes. The request
  budget halves on a 429 or 529, at most once a second, grows back each
  second without one by a twentieth or by one request a second, whichever
  is more, and stays between 1 and the setting. A `Retry-After` pauses
  every sender. The token budget estimates a request with the measured
  bytes per token (rule 9). The senders follow from the request rate: as
  many as keep it at 1.2 s a request, at most 256, and at most
  `JEV_PARALLEL` (an API's `parallel`) when it is set. Each sender has one
  request out at a time, so no more than that are at the API at once.
- **Why.** An organization's limits are shared by all its keys and differ
  by plan, so its 429s, not deem's settings, are the real limit; two
  invocations on one organization each settle near half. At the defaults,
  two invocations against a stub limited to 40 requests a second took
  35 s for what the limit allows in 24, and against 80, 32 s (2026-10-19),
  every record answered. A server of your own works on a few requests at
  a time and queues the rest, and the reply timeout (rule 4) runs while a
  request waits there: against llama-server with 4 slots and Kev-9B,
  300 records sent at once had 64 to 249 requests time out in its queue
  and sent again (2026-10-21). With `parallel` at its slots, none waits,
  and the run is no slower: against Kev-4B with 4 slots, 300 requests took
  28.2 s at `parallel` 4 and at 256, a request 281 ms at the median at 4
  and 13.6 s at 256 (2026-10-21).
- **Test.** `keeps_to_the_rate`: 40 requests at a setting of 10 a second
  take about 3 s, none throttled. `parallel_caps_requests_at_once`: at
  `JEV_PARALLEL=3`, a stub that takes 100 ms a request never holds more
  than 3, and every request is sent once. `two_invocations_share_the_rate`: two
  invocations at 20 a second against a stub limited to 10 both finish,
  with no failed records. `the_rate_is_a_setting`: the defaults, what
  follows from a setting, and the settings deem refuses.

### 9. Tokens are estimated from measured density

A request's tokens, for the token rate (rule 8) and for an attempt the API
reports no usage for, are estimated at the bytes per token measured so far.

- **How.** deem measures bytes per token from its own answered requests of
  16 KiB or more (body bytes ÷ `usage.input_tokens`), starting at 2, a
  conservative value. A request carries one value of at most
  `DEEM_MAX_RECORD_BYTES` (32 KiB), so it fits the model's 32,000 tokens
  for the state unless the value is denser than about 1 byte a token
  (digits, hex); one that doesn't is refused and fails with the API's
  reason, alone (rule 10).
- **Why.** Density varies by more than 4× between inputs (below), so no
  fixed number of bytes a token is right for all of them.
- **Test.** `tokens_are_estimated_as_measured`: a stub reports 4 bytes a
  token on one request and no usage on the next, which is counted at 4.

### 10. Failures stay local

A failed answer fails only the records that asked that question.

- **How.** Each question has its own promise, and a request's failure sets
  only its own questions' promises. A request carries one record. Records
  share nothing but the memo.
- **Test.** `client_errors_are_isolated_to_their_record`.

### 11. Nothing missing goes unreported

- **How.** Every record ends with an outcome, even when its worker fails
  on it.
  The main thread shows the first five records that fail and counts the
  rest; a run of more than one record ends with how many failed of how
  many were read, and how many more a stop left unfinished, and
  `--failed` lists them all. When failing attempts have
  held the invocation up for 10 s, the pool leaves a note, and the main
  thread, which looks for one every 100 ms while it waits, passes it on
  once.
- **Test.** `missing_records_are_counted`; `an_outage_stops_the_run_once`;
  `long_delays_are_announced`.

### 12. Pictures go as the API takes them

A request that shows a picture carries it once, as an asset whose ID is
a hash of the picture's bytes and which replaces every reference to it;
it goes as `multipart/form-data` with the images header; every asset is
shown and every reference replaced; the picture is read when the request
is sent, not before; and a request whose pictures put it over the API's
token limits is refused before it is sent.

- **How.** `picture::resolve`, as the batcher builds a request, finds each
  reference (a whole JSON string with the run's token) in the table and
  counts its tokens, a picture once per place it is shown;
  `picture::prepare`, in the sender before the first attempt, checks each
  file's header again, hashes it, and swaps the references for IDs;
  `Http::judge` sends the files as parts named by their IDs. The run's
  token is random, so only references deem made are found.
- **Why.** The API fails a request with an unshown asset or a missing
  header; a fresh random ID per request would make the same run send
  different bytes, and deem can check directly that an ID occurs nowhere
  else in the request, which is what a fresh UUID guards against.
- **Test.** `sending_pictures` (one asset per picture, shown twice, the
  same bytes on two runs); `pictures_go_as_multipart` (the header, the
  parts, no path in the body); `an_unasked_picture_isnt_sent`;
  `data_cant_name_a_picture`; `pictures_count_against_the_token_limit`;
  `prepared_as_sent` (a picture gone: its request fails; an `--img` one:
  the run stops).

### 13. What the API says is shown as text

What a server sends that deem shows, in an error (stderr, the trace's
`result`, the error a program's `try` catches) or from `deem models`,
has its control characters but newline and tab written out (`\u{1b}`),
so it can't move the cursor, retitle the terminal or reach its
clipboard. And a reply that repeats the key, as sent or JSON-escaped,
has it replaced with `<key>`, a successful reply too, before it is cut
short or traced; a key shorter than 8 characters isn't looked for.

- **How.** `ApiError::describe`, which makes every error's text, ends in
  `printable`; `Http::models` too. `Http::read` redacts the key from the
  reply's body and a redirect's `Location` as they arrive.
- **Why.** A server's text reached the terminal byte for byte. A stub
  answering 500 with `\x1b]0;PWNED\x07\x1b[2J\x1b[31mfake: all good`, and
  `deem models` run in tmux 3.2a: before this rule the pane's title became
  `PWNED` and the screen was cleared, deem's own `api 500:` with it,
  leaving a red `fake: all good`; with it, the title and the screen stay
  and the codes show as text (2026-10-21, deem at 0c17040 and 70fcbbc). kubectl's
  CVE-2021-25743 was this (NVD, CWE-150), and git's "Sanitize sideband
  channel messages" patches mask such characters from the server too
  (git mailing list, 2025–2026). Redaction is best effort, as secret
  masking in CI is: it catches a header echoed whole, not the key
  encoded or in part. GitLab masks only values of 8 characters or more
  (GitLab's CI variables docs, 13.8 and later), for the same reason as the
  threshold here.
- **Test.** `what_the_api_says_is_shown_as_text`.

### 14. What the transport does not bound

- A program's own loop: the question cap bounds it, and
  `--max-questions 0` removes that bound.
- Endless input: spending grows with what is read (rule 2).
- A loop over questions already asked: answered from the memo, free at the
  API, and it runs until stopped.
- A record's journal: every answer the record has had, kept until it ends,
  as each run of it needs them; one record that asks many questions holds
  them all, up to the question cap. And 8 bytes for each time a record
  reads `now`, which an endless stream that reads it per output grows
  without bound.
- The CPU of running a record again: a record with d rounds runs d + 1
  times, redoing its jq work each time, so for one record with many
  dependent rounds it grows as the square of the rounds (DESIGN.md,
  "Replay: running a record again").

## Efficiency goals

Not rules: missing one is a performance bug, not a broken guarantee.

- **A question goes the moment its record's run stops**, whatever other
  records do (`a_stream_is_not_held_back`, `slow_request_does_not_block`).
- **A record's questions travel together**, in one request.

## API facts

The rules rest on these, checked against the API and its documentation on
2026-09-29. If one changes, so may the rule it supports.

| Fact | Source | Supports |
|---|---|---|
| Limits are per organization, shared by all its keys, "measured in tokens per second and requests per second"; the default is 80 requests and 100,000 input tokens a second (the console, 2026-10-19). Higher limits come with custom plans. 600 requests at 115 a second drew no 429 (2026-10-19): deem doesn't rely on that. | docs (Models), the console; measured | rule 8 |
| 429 is the rate limit, 529 an overload; both are to be retried with backoff. The official SDKs retry 408, 429, 5xx, connection errors and timeouts, and honour `Retry-After` and `retry-after-ms` up to 60 s. | docs (Errors, SDK retry policy) | API errors table |
| An invalid key is 401, a missing key 403. | measured | setup errors table |
| Without `/v1` the URL is 404; `http://` is redirected (301) to https, and a redirected POST arrives as a GET (405). | measured | setup errors table |
| An unknown model is 400, `{"error_type": "api_usage_error", "message": "Unknown model: …"}`. | measured | setup errors table |
| A refused request is 400, not the documented 422: `max_tokens_exceeded`, `api_usage_error` (`Invalid request.`), or a plain message. It reports no token usage. | measured | request errors table |
| Per request: 64,000 tokens in all, 32,000 for the state and the longest question. A 34,000-token state is refused in 0.6 s; 30,000 is answered. | docs (Models); measured | rule 9 |
| Bytes per token: English 4.6, JSON 4.0, log lines 2.0, CJK 1.6, base64 1.4, hex 1.1, digits 1.0. | measured | rule 9 |
| A picture costs about width × height / 390 tokens each time a request shows it (280²: 234; 1568²: 6,300), on `jev-latest`. A request with a picture needs the header `TypeSafe-Danger-Zone-Images: true` (else 400); every asset must be referenced (else 400 `Invalid image request.`); a picture under 280 px a side is refused. References are replaced as plain text anywhere in a string, whole strings first; keys never. | docs (Images); measured 2026-10-16 | rule 12 |
| A request costs 272 input tokens before its content; a question about 30 beyond the value it carries. | measured | the manual's cost estimates |
| Questions in one request are evaluated independently: a hostile second question moved another's score by at most 0.01. The same request twice gave the same scores. | docs (State); measured | a record's questions in one request |
| Values in one shared state are not: against one value per request, 24 in the state moved scores by 0.07–0.08 and 7–9% of decisions on a judgement call, on 150 distinct messages, and 142 of 200 decisions with near-copies of log lines. Wording, other layouts, and 4 to a request didn't change that. | measured (tests/live, 2026-09-29) | one value per request |
| The API documents `state` for "the content and supporting facts" and instructions for the question and "the data that guides the question"; it has one state per request and no batch endpoint. | docs (State, How to build, API reference) | one value per request, in the state |
