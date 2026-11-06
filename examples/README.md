# examples

Sample inputs and example programs for the [README](../README.md) and the
[manual](../docs/MANUAL.md). Every command there runs as written from this
folder, and so does every command here: `tests/readme.rs` runs them all.

```sh
cargo install --locked --path .
deem api add jev https://api.typesafe.ai/v1 --pictures --key   # no offline mode
cd examples
```

## Programs

The example programs are saved in [`.deem/`](.deem/), so from here they run
by name, and `deem programs` lists them with their descriptions. Each file starts with a comment that says what it shows.

**The README's and the manual's**, from their programs sections:

```sh
deem run hello-world feedback.txt   # the smallest program file
deem run route billing tickets.txt --arg urgent 0.9   # an argument, a value
deem run problem feedback.txt       # a question saved with its threshold
deem run triage app.log             # a regex first, then two questions
```

**What jq's language adds** over a single question: grouping records,
nested JSON, choosing what the model sees, a follow-up that depends on
the first answer, questions that are data, and counting the answers.

```sh
deem run -s failed-requests requests.log   # group lines by request id
deem run -J labels tickets.jsonl           # three questions, one request
deem run -J mislabeled issues.jsonl        # send only title and body
deem run -J moderation threads.jsonl       # ask about nested comments
deem run -J ticket-kinds tickets.jsonl     # the next question fits the kind
deem run decision-tree alerts.log          # walk a decision tree of questions
deem run topics tickets.txt                # count 300 tickets by topic
```

**Pictures**: `.` is each picture, and `--img` adds one of your own. `locate`
and `screen-check` use `box`, which needs Jev 1.14: `jev-latest` was 1.14
on 2026-10-22; if yours refuses a box, `deem api edit jev --model
jev-preview`.

```sh
deem run screen-check screens/*.png    # loaded, an error, broken or loading
deem run locate "the Pay button" screens/*.png    # where, if it is there
deem run --img baseline screens/orders.png same-page screens/orders*.png
```

To change one, copy it under a new name: `deem programs labels >
.deem/my-labels.deem`.

## Comparing APIs

`sh trace.sh API` runs these programs against an API, with a trace each in
`traces/`, and `deem trace compare` compares the traces of two or more
APIs; the manual's [Comparing APIs](../docs/MANUAL.md#comparing-apis)
shows how. The script lists what it leaves out, and why.

## Data

| file | what it is | used by |
|---|---|---|
| `feedback.txt` | nine short customer messages | the README's first examples, `group` |
| `tickets.txt` | 300 one-line support tickets for an online shop's software, all different | the README's programs, `route`, `topics` |
| `sample.txt` | the same 300 tickets, shuffled (`shuf -n 300 tickets.txt`) | `histogram`, `mix`, `disagree`, `tune` |
| `week38.txt`, `week39.txt` | two weeks of 40 tickets each; week 39 had a checkout outage | comparing histograms |
| `tickets.jsonl` | tickets with `id`, `customer` and `body` | JSON examples, `labels`, `ticket-kinds` |
| `app.log` | five lines of application log | the manual's examples, `triage` |
| `alerts.log` | log lines for every leaf of the tree: security and other errors, warnings, info | `decision-tree` |
| `auth.log` | ssh authentication log, with a run of failed passwords | `tests/deem/bruteforce.deem` |
| `labeled.tsv` | log lines labelled by hand, `1` or `0` | the manual's calibration check |
| `big.jsonl` | one ticket over the 32 KiB judging limit | the manual's gotcha 6 |
| `requests.log` | interleaved lines from concurrent requests | `failed-requests` |
| `issues.jsonl` | GitHub-style issues with labels | `mislabeled` |
| `threads.jsonl` | threads with nested comments | `moderation` |
| `screens/*.png` | five 960×600 screenshots of a shop's admin pages: orders, the same with its stylesheet missing, a declined payment, a status page that only mentions errors, one still loading | the README's "Pictures", `screen-check`, `locate`, `same-page` |

## A note on the numbers

Scores drift by a few hundredths between runs (the model is consistent, not
deterministic), so the values printed in the docs are illustrative. Some
examples print nothing: "nothing selected" is a real and common result, and
exits 1, like `grep`. Reach for `--stats` before assuming the worst.

These files are small: a five-line log is not evidence that a threshold
generalises. To pick a threshold, run `deem tune` on a sample of your own
data, or label one and score it; the manual's "Check the calibration"
section shows how.
