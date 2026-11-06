# Live tests

Every other test runs against a fake model. These run against the real
API, so they cost money and never run unless asked: they are `#[ignore]`d,
and they refuse to start without `DEEM_LIVE=1`. A key in the environment is
not enough, because the README tells every user to export one.

They are the only tests that can reach it. Every deem another test starts
has a proxy where nothing listens in its environment (`OFFLINE`, in
tests/common), so a request to any host but this machine is refused;
`tests/layers.rs` checks that no test starts one without it. `just test`
runs without a network at all, and CI with the API's name pointing at
itself. So run these with `cargo test` itself, as below, not `just test`.

deem sends one value per request, in the request's state, as the API
documents. These tests are how to find out when a new model changes what
deem relies on.

## What they check

**`versus.rs`: where `like` and `unlike` examples go.** `like_and_unlike`
measures them none, in the state, after the question, and in the
question's criteria, as the API's docs put them ("Structured Noul
criteria") and deem does: three yes
and three no texts of each dataset as the examples, the others judged on
its judgement call, twice for the noise, as the decisions right at 0.5,
the Brier score (mean squared distance from the label, lower is better)
and the AUC (the chance a yes scores above a no). On 2026-10-19 the
criteria were level with after the question (Brier 0.048 against 0.046 on
urgency, 0.052 against 0.051 on logs, 0.033 against 0.039 on sarcasm).

The datasets, each with its own questions, so a shape isn't judged on one
kind of text: support messages (`labels.tsv`: a problem, billing, urgent,
topic), log lines (`logs.tsv`: would a user notice, a security event, must
someone act today, which part of the system) and app reviews
(`reviews.tsv`: recommends it, describes a bug, sarcastic, what about).
`DEEM_DATASETS=logs,reviews` runs only those. All three were written or
labelled by Claude on 2026-09-29, with judgement calls marked `-` and left
out; review them before trusting a result, as a wrong label misjudges
every shape alike. New datasets are welcome: a TSV with three yes/no
columns, a topic and the text, and an entry in `DATASETS`.

**`versus.rs`: how far text in a record moves answers.** `robustness`
runs deem itself on each dataset, plain and attacked: orders to answer
yes or no (at the end, at the start, on borderline texts, hidden in 3 KB
of text of its kind), a plausible paragraph for yes, text imitating the
request (in prose, as its JSON, as tags, and our own question echoed with
an answer), and JSON records of the dataset's kind with a field that asks
a question of its own (`question`, `instructions`, `prompt`), a field full
of `^`, and chat logs. It prints the decisions and mean score before and
after each, then fails if an order or imitation moved more than 14
decisions, "answer no" lowered the mean by more than 0.25, or a field
added more than 25 false yeses: about twice what it measured on
2026-10-19 (at most 7, 0.13 and 13). The manual's Safety section quotes
its numbers.

**`pictures.rs`: do pictures still work as deem assumes?** On the example
screenshots (`examples/screens/`): the README's error example (a page
that only mentions errors isn't one that shows one), a page against a
baseline with `--img`, `screen-check` naming each page's state and boxing
the error where its banner is, `locate` boxing the Pay button where it is
and keeping only the page that has one, and a picture's input tokens
within a factor of two of deem's estimate. The `box` tests set
`JEV_MODEL=jev-preview`. They assert. On 2026-10-20 every answer was where
the README shows it, the boxes within 0.01 of the banner's and button's
edges, and the 960×600 picture cost 1,854 tokens against 1,477 estimated.
On 2026-11-06 (`jev-1.14.0`) they passed again, at the same cost, with
`orders.png` against itself as the baseline at 0.57 to 0.86 over 14 runs
and the unstyled page at 0.01; the README shows one of those runs.

**`api.rs`: does the real API still behave as deem assumes?** The README's
first example works end to end, and the model reads `group`'s `{a, b}`
records as a comparison. These assert, and fail if not.

They don't judge whether the model is right about your data; that is
between you and your labels (`deem tune`). Run them after changing how deem
builds requests, and when the model changes. The noise floor is also worth
comparing with `mix`'s 0.15: a gap it calls clear should be well above how
much answers vary between runs.

## Running them

```sh
DEEM_LIVE=1 JEV_API_KEY=… cargo test --test live -- --ignored --nocapture like_and_unlike
```

`--nocapture` shows the numbers. To measure another model or endpoint, set
`JEV_MODEL` or `JEV_BASE_URL` too; they are passed through to deem. Name a
test to run only it (`robustness`, `like_and_unlike`, `quick_start`), or a module (`pictures::`), or leave the name out to run them all, which costs more.

Scores vary between runs, so compare a difference with the noise printed
next to it, not with a fixed number.

## Measured by hand

Some checks have no live test; these runs were made by hand, with deem
built at the date given. The photos aren't in the repository, so their
checks stay here; the example screenshots' are in `pictures.rs`.

- **`deem trace compare` on the examples** (2026-10-22): `sh trace.sh
  jev` and `sh trace.sh kev8` (Kev-4B Q8_0 on llama-server, 4 slots, a GPU
  of 8 GB), then `deem trace compare --with jev traces/*.jsonl`, gave the
  manual's report. The hosted API's responses named the model
  `jev-1.14.0`. On `jev-latest`, all six of `screen-check`'s requests were
  answered, its `box` too, and `run locate "the Pay button" screens/*.png`
  on `jev-latest` had all five requests answered by `jev-1.14.0` with a
  box, though the API's model page listed `jev-latest` as 1.13.0.

- **A photographed broken phone** (2026-10-16, a 640×427 JPEG from the
  API's playground, `jev-latest`). Multipart through `ureq` was accepted;
  the four answers fit the picture (damage `Severe` at 0.99, the photo
  supports the claim 0.73). The request cost 1,254 input tokens, about 760
  of them the picture, against deem's estimate of 701 (width × height /
  390). On `jev-preview`, `box` found the crack's origin (0.63), the phone
  (0.90), the speaker slot (0.68) and a blurred plant in the background
  (0.81), every box on its target, all five questions in one request of
  1.1k tokens.
- **Three photographed Magic cards** (2026-10-18, 577×768 JPEGs on carpet,
  `jev-preview`; the photos aren't in the repo: the card art isn't ours).
  Every box of six (the card, its name, mana cost, art, text box and set
  symbol; the cost about 5% of the width) was on its target. A card with no
  mana cost scored 0.03 for having one and got a box on its set symbol, so
  `box` needs the `ask` beside it, as in `locate`. With two pictures in the
  value, the card's symbol matching a sample card's was found and boxed
  (0.61), and a card with none scored 0.24. An EXIF orientation tag changed
  nothing: boxes are in the stored pixels. A request cost 1.5k tokens on
  either model, about 1.2k of them the picture against deem's 1,136. A
  program that waits for an answer before its next question sends the
  picture again each wave: three waves cost three times as much. Asked a
  card's colour, names alone called a Plains white (0.87); with a
  description per colour, colourless (0.90), as the game has it, and the
  other two cards' right answer rose from 0.58 to 0.71 and 0.82 to 0.99.
  Blurring the mana cost dropped "can you read the mana cost?" from 0.89 to
  0.04, but `choose` still named its colours: a blur hides shapes, not
  colour.
- **Replay's CPU on a long `group` run** (2026-10-20, branch `replay`,
  release build, `jev-latest`): `deem run --stats group "is this the same
  issue?" sample.txt` in `examples/` found 194 groups in 300 lines, so 193
  waves, 22,692 requests and 6.9M input tokens, in 469 s. deem used 38.7 s
  of CPU (27.4 s user, 11.3 s system), 8% of the run, and 123 MB at most:
  one record that runs again 194 times, as DESIGN.md's "Replay: running
  a record again" bounds it (about 0.04% of a run per round). It is the most rounds a
  shipped program makes on the manual's own sample; costly to repeat.
- **Servers of your own that speak the Jev API** (2026-10-21, deem's debug
  build, an RTX 2060 SUPER with 8 GB). Four log lines (`ERROR login bypass
  via token reuse`, `ERROR db pool exhausted`, `WARN disk 85% full`, `INFO
  user signed in`), asked four ways with `deem -p`, `JEV_API_KEY=` and
  `JEV_BASE_URL` at the server; every request answered. With `JEV_API_KEY=`
  deem sent no `Authorization` header (`nc -l` in the server's place). [Kev](https://github.com/jaredpalmer/kev) at `1f04cfa`
  accepts deem's `"model": "jev-latest"` as an alias. Kev-4B (CPU, bf16)
  ranked the lines ERROR, ERROR, WARN, INFO for "is it critical?" (0.66,
  0.76, 0.44, 0.27), the four requests together in 24 s. Kev-0.8B (GPU,
  fp32, `KEV_CUDA_GRAPHS=0`: with CUDA graphs it ran out of the card's
  memory) ranked them the same way for every question but the security one,
  where only the login bypass stood out, with scores closer to 0.5 (0.60,
  0.54, 0.41, 0.32), in about 0.2 s a batch. [Laya](https://github.com/NandhaKishorM/laya)
  0.4.12 (`laya-serve`, GPU) answered in about 55 ms but scored every line
  under 0.5 for "is it critical?" and "db pool exhausted" low for every
  question. Pictures were sent to none of them.
- **Kev from GGUF on llama-server, and the examples on three models**
  (2026-10-21, deem's release build, the same RTX 2060 SUPER). llama.cpp's
  `llama-server` at `ead9f32` (as LM Studio's CUDA 12 runtime 2.62.0
  ships it) served `ggml-org/Kev-4B-GGUF` (Q8_0, converted from Kev-4B's
  `v1.0` snapshot `6cfce5c2`) on `/v1/systemone`, 4 slots by default,
  7.1 of the card's 8 GB in use. On the four log lines above it scored
  within 0.015 of Kev's own server running Kev-4B in bf16 on CPU, in
  0.11 s instead of 14–22 s. `Kev-9B-GGUF` Q4_K_M ran out of the card's
  memory with `-ngl 99`, also with `-ub 256`, and started with `-ngl` left
  out, llama-server fitting the layers itself; it answered about 3
  requests a second. LM Studio's own API (port 1234) answered
  `/v1/systemone` with HTTP 200 and `{"error": "Unexpected endpoint or
  method. (POST /v1/systemone)"}`. The 14 text programs in
  `examples/README.md` and the built-ins, on Jev, Kev-9B Q4_K_M, Kev-4B
  Q8_0 and Kev-0.8B (Kev's server, GPU, fp32): `decision-tree` and
  `group` gave Jev's answers on Kev-4B and 9B; programs with thresholds
  set on Jev (`problem` 0.7, `route` 0.9) kept fewer records on every
  Kev, which ranks much as Jev does and scores lower; Kev-0.8B's scores
  stayed nearest 0.5. With 300 records sent at once and no `--parallel`,
  Kev-9B had 64 to 249 requests time out in llama-server's queue and sent
  again, and 686 tasks cancelled in its log. With `--parallel 4`, Kev-4B
  took 28.2 s for 300 one-line tickets (`sample.txt`), three questions
  each in one request, as with 256, and a request 281 ms at the median
  instead of 13.6 s.
- **Kev and vllm-d3-mini from `-hf`, at two quantizations** (2026-10-26,
  deem's release build, the same RTX 2060 SUPER, llama.cpp `a37cc46`).
  `llama-server -hf ggml-org/Kev-4B-GGUF:Q8_0 --host 127.0.0.1 --port 8010
  -ngl 99` loaded the file from `~/.cache/huggingface/hub`, answered
  `/health` after 23 s, and `deem --api kev8 "is it critical?" app.log`
  kept the timeout and the 502. With `LLAMA_CACHE` at an empty folder,
  `-hf ggml-org/Kev-4B-GGUF:Q4_K_M` started downloading at once, into
  `$LLAMA_CACHE` in the Hugging Face layout, the blob named as the cached
  file's hash; stopped after 20 s at 36 MB. With `-m` on the cached files,
  `-ngl 99` and 4 slots, the manual's table in "A server of your own":
  every quantization filled the card to 7.1–7.4 GB, llama-server sizing
  each slot's context to what was left (Kev-4B 18,944 tokens on Q8_0,
  59,136 on Q4_K_M; d3-mini 4,096 and 9,984, its mmproj Q8_0, `-b 8192
  -ub 8192`), and the four log lines above asked `deem -p "is it
  critical?"` kept the same order on all four. d3-mini through deemkit's
  `proxy/pictures.py` scored the five example screenshots for "does the
  screenshot show an error?" as Jev orders them, `checkout-error.png`
  0.99 and the rest 0.14 or less, in about 2.4 s. Kev's and d3-mini's
  GGUF files and Kev's `LICENSE` say Apache-2.0.
