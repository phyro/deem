# deem: the manual

The [README](../README.md) covers everyday use: the question forms, pictures,
a first look at programs, and getting a question right. This is the rest: how
programs are written, saved and run, the programs that come with deem, what
happens when an answer fails, how deem differs from jq, safety, and every
option.

## Programs are jq

`deem run` takes a program in [jq](https://jqlang.org/)'s language, as [jaq](https://github.com/01mf02/jaq) runs it: deem is built on jaq, and a program that doesn't ask anything prints what jaq prints. Everyday jq programs run unchanged, and you do not need jq installed. Where jaq and jq disagree, deem follows jaq; the cases you are likely to meet are listed in [Differences from jq](#differences-from-jq). The program runs once per record: `.` is the line, or with `-J` the JSON value ([Input](#input)). deem adds `ask`, `choose`, `rate` and `box` ([The whole interface](#the-whole-interface)); [Program files](#program-files) has saving a program, its arguments, and reading every record at once.

Five shapes cover most programs.

**Filter.** Keep the records where the answer is strong enough. `deem "Q" -t T` runs exactly this.

```bash
deem run 'select(ask("is this an error?"; .) > 0.8)' app.log
```

**Score.** Print the answer next to the record, tab-separated, ready for `sort -rn`. `deem "Q" -p` does the same, and prints JSON records unchanged.

```bash
deem run '[ask("is this an error?"; .), .] | @tsv' app.log
```

**Judge one field.** The last argument is what the model sees, so this sends only `body` and none of the rest of the record.

```bash
deem run -J 'ask("is this about billing?"; .body)' tickets.jsonl
```

**Label.** Build an object, with answers as fields next to the data you want to keep (`deem "Q" -o A,B` prints `option<TAB>record` instead). A record's questions go out together, so two questions cost no more waiting than one; see [How it works](#questions-that-dont-depend-on-each-other-go-together).

```bash
deem run '
  {text: ., svc: choose("which service?"; ["api","db","worker"]; .)}
' app.log
```

**Cheap test first.** A regex drops most records before the model sees any of them, which saves requests.

```bash
deem run 'select(test("ERROR"))
          | select(ask("would a user notice?"; .) > 0.5)' app.log
```

### Reading a program

| jq | meaning |
|---|---|
| `.` | the current record: the line, or the parsed JSON object |
| `.body` | one field of a JSON record |
| `a \| b` | feed the output of `a` into `b` |
| `select(c)` | keep the record if `c` is true, drop it otherwise |
| `ask(q; .)` | the model's answer to a yes/no question, as a score in [0,1] |
| `choose(q; opts; .)` | one of `opts`; arguments are separated by `;` ([gotcha 1](#gotchas)) |
| `[a, b] \| @tsv` | a tab-separated line |
| `{k: a}` | a JSON object; `{id}` is short for `{id: .id}` |
| `test("re")` | a regular expression match |
| `and`, `or`, `not` | combine conditions: `select(ask(q; .) > 0.5 and (test("api") \| not))` |

The [jq tutorial](https://jqlang.org/tutorial/) and [manual](https://jqlang.org/manual/) cover the rest.

### Program files

A program worth keeping goes in a `.deem` file. This is [`hello-world.deem`](../examples/.deem/hello-world.deem), one of the example programs in `examples/.deem/`:

```jq
module {input: "each"};
# hello-world: say hello to every line, and ask whether it is friendly

def main:
  "hello, \(.)! friendly: \(ask("is this message friendly?"; .))";
```

```console
$ deem run .deem/hello-world.deem feedback.txt
hello, charged twice for one order??! friendly: 0.08
hello, the checkout page keeps spinning forever! friendly: 0.21
...
```

- `main` is where the program starts.
- The first line, `module {…}`, is jq's syntax for describing a file. deem reads two things from it: `input`, what `main` gets, and `defaults`, the program's named values ([below](#arguments-and-named-values)).
- `input: "each"` runs `main` once per record, with `.` the record. Records run in parallel, a failed answer the program doesn't catch fails only its own record, and lines print as they are ready. Without a `module` line, a program is `"each"`.
- `input: "all"` runs `main` once, with `.` an array of every record ([below](#reading-every-record-input-all)).

Saved in `.deem/`, it also runs by name: `deem run hello-world feedback.txt` ([Saving a program](#saving-a-program)).

A program without `main` is plain jq, runs once per record, and takes no arguments: `deem run 'select(…)'` is one. A file can be plain jq too, but the example programs and the programs that come with deem always define `main`, even when it takes no arguments, so every program file has the same shape: the `module` line, a comment, `main`.

### Arguments and named values

A program gets values in two ways:

- **Arguments** are required. `main`'s parameters are the arguments, in order, so `def main($a; $b)` takes exactly two, from the words after the program.
- **Named values** are optional, and have defaults, declared in the `module` line. Each is a variable, and `--arg NAME VALUE` changes it.

`route` has both: the topic is an argument, and its two thresholds are named values:

```jq
module {input: "each", defaults: {urgent: 0.8, about: 0.6}};
# route: urgent tickets about a topic
def main($topic):
  select(ask("is this urgent?"; .) > $urgent
     and ask("is this about \($topic)?"; .) > $about);
```

```sh
deem run route billing tickets.txt   # urgent above 0.8, billing above 0.6
deem run route login tickets.txt --arg urgent 0.9
```

The word after the program is the argument, `$topic`, and the files come after it. Leave the topic out and deem can't tell: `deem run route tickets.txt` takes `tickets.txt` as the topic and reads stdin. With nothing after the program at all, it says `route takes 1 argument (topic), got 0`.

`-t N` is short for `--arg threshold N`. A program that decides one yes or no calls its value `threshold`, so `-t` works for it; a program with several decisions names each one, as `route` does.

Values are passed in as values, never pasted into the program's text, so a question with quotes in it can't break the program. An argument is always a string; a named value has its default's type, so `--arg urgent abc` is an error, and so is a name the program doesn't declare.

After `deem run PROGRAM`, the next words are its arguments, as many as `main` takes, and the rest are files. Words that start with `-` are deem's options, wherever they are: in `deem run disagree "is this a problem?" -t 0.7 "is the user complaining?" sample.txt`, `-t 0.7` is an option, the two questions are the arguments, and `sample.txt` is a file. An argument that starts with `-` goes after `--`: `deem run histogram -- "-1 means what?" sample.txt`.

### Reading every record: `input: "all"`

Some programs need every record at once: to count, sort or compare them. With `input: "all"`, `main` runs once and `.` is an array of every record, lines as strings or JSON values. All the programs that come with deem work this way. `disagree` is one, and `deem programs disagree` prints it:

```jq
module {input: "all", defaults: {threshold: 0.5}};
# disagree: lines where two wordings of a question decide differently

def show: if type == "string" then . else tojson end;

def main($q1; $q2):
  def yes: . > $threshold;
  [.[] | {line: ., scores: [ask($q1; .), ask($q2; .)]}]
  | map(select((.scores[0] | yes) != (.scores[1] | yes)))
  | sort_by(.scores | max - min) | reverse | .[]
  | "\(.scores[0])\t\(.scores[1])\t\(.line | show)";
```

It builds every question before it compares any answer, so all of them go out together, in one wave. (A program may have 12,288 questions unanswered at once: 64 for each record that could be running. Past that the rest follow in another wave, which costs only the wait: 13,000 lines through `histogram` are two.) Comparing inside the loop, as in `.[] | select(ask(q; .) > 0.5)`, waits for each answer before asking the next question: one wave per record. Build the array first, then compare.

The cost of `"all"`: nothing prints until the end, the whole input is held in memory (up to 64 MiB), and one failed answer the program doesn't catch fails the whole result. It suits the analysis programs, which run on a sample.



## Saving a program

Save a program as `NAME.deem` in a `.deem/` folder, and it runs as `deem run NAME`; the README's [Programs](../README.md#programs) saves one. A program often belongs with a project: it asks about the project's logs or tickets, with a threshold tuned on the project's data. Commit `.deem/` with the project, and everyone who clones it, and its scripts and CI, can run those programs by name; keep your own in `~/.deem/`.

deem looks for a name among the programs that come with it first, then in the nearest `.deem/` (in the current directory, or the closest directory above it that has one, below your home directory), then in `~/.deem/`. So a project's programs run by name from anywhere in the project, and `deem programs` shows where each one is from where you are: `../../.deem/triage.deem`. A program that comes with deem can't be replaced, so `deem run histogram` means the same thing everywhere, and a program in `.deem/` hides one with the same name in `~/.deem/`.

`deem programs` lists every program you can run: its arguments and named values, where it comes from, and its first comment line:

```console
$ deem programs
decision-tree                    .deem/decision-tree.deem
    route each line down a decision tree, one wave per level
failed-requests                  .deem/failed-requests.deem
    which requests ended in an error the user saw
hello-world                      .deem/hello-world.deem
    say hello to every line, and ask whether it is friendly
...
problem [threshold=0.7]          .deem/problem.deem
    messages that report a problem (threshold from deem tune)
route TOPIC [urgent=0.8] [about=0.6] .deem/route.deem
    urgent tickets about a topic
...
group Q [threshold=0.5] [groups=20] built-in
    lines that mean the same, numbered together; a request per line per group
histogram Q [threshold=0.5]      built-in
    how the scores are spread, instead of the lines
mix Q OPTS                       built-in
    how the options are spread, and which ones the model confuses
disagree Q1 Q2 [threshold=0.5]   built-in
    lines where two wordings of a question decide differently
```

In `examples/` the list starts with the example programs; [`examples/README.md`](../examples/README.md) says what each one shows. A program that another hides is listed with `(hidden by .deem/NAME.deem)`.

`deem programs NAME` prints a program, so you can copy one that comes with deem into `.deem/` under a new name and change it.

A saved program's name can't be one of jq's own functions (such as `length`), so a name after `deem run` always means one thing.


## Programs that come with deem

| Program | Does |
|---|---|
| `group Q` | Put lines that mean the same together, under one number |
| `histogram Q` | Show how the scores are spread, instead of the lines |
| `mix Q A,B,C` | Show how the options are spread, and which ones the model confuses |
| `disagree Q1 Q2` | Show only the lines where two wordings of a question give different answers |

`-t` sets the threshold of those that have one (`group`, `histogram`, `disagree`). `deem programs NAME` prints each, to read or to copy and change. Each reads every record at once ([`input: "all"`](#reading-every-record-input-all)), so run them on a sample: `shuf -n 300 tickets.txt > sample.txt`, saved so that every attempt sees the same lines. A random sample beats `head`, because the start of a file is often not typical (startup messages, sorted exports); on macOS, `shuf` comes with coreutils as `gshuf`.

The summaries (`histogram`, `mix`) are tab-separated tables. The table comes first; what else they found follows it, each part behind a line of dashes, with a label in front of every row. `sed '/^---/,$d'` keeps the table alone, and `grep '^baseline'` picks a part out.

### `group`

```sh
deem run group "is this the same issue?" feedback.txt | grep '^1	'
deem run group "is this the same issue?" feedback.txt | cut -f1 | uniq -c
```

The earliest line becomes group 1, and every line is asked, all at once, whether it is the same as that one. The lines that aren't form the rest, whose earliest line becomes group 2, and so on, until no lines are left or there are 20 groups. The lines left then print under `-`, in input order, so `cut -f1 | uniq -c` counts them too. Each round is one wave, so the number of waves grows with the number of groups, not the number of lines (up to about 12,000 lines: [Reading every record](#reading-every-record-input-all)).

A wave sends a request for each line still left, so the cost is set by how many groups there are: up to a request per line per group. A strict question on varied lines makes nearly every line a group of its own, the worst case: "is this the same issue?" put 300 tickets in 194 groups, which without the limit cost 22,692 requests (2026-10-20). With the default of 20 groups, 300 lines cost at most 5,790. `--arg groups N` sets another limit, and `--arg groups 0` none. To keep it cheap, try a looser question ("is this about the same part of the product?") or fewer lines first. Each group is represented by its first line, so the groups depend a little on the order of the input, and the 20 are the first 20 found, not the largest; treat the result as an overview, not an exact count.

### `histogram`

Each row is a bin, its count and a bar; after the dashes come the lines above the threshold and the baseline:

```console
$ deem run histogram "is this urgent?" sample.txt
0.0	25	███
0.1	69	█████████
0.2	46	██████
0.3	39	█████
0.4	31	████
0.5	28	████
0.6	33	████
0.7	8	█
0.8	12	██
0.9	9	█
--------------------
above 0.5	88	████████████
baseline	0.32
```

Here the scores spread across the middle: "urgent" is a judgement call, and a threshold anywhere between 0.3 and 0.7 moves dozens of tickets. A question about something the ticket says, such as "is something broken for the customer right now?", puts most lines at the two ends (the README shows it). The baseline is the score for an empty line: what the model says with nothing to judge. If it is well above 0, the question leans toward yes on its own.

A bar shows its bin's share of the lines, not its count, so histograms of files of different sizes compare fairly. To compare this week with last week, run it on each: in these two weeks of tickets, the lines above 0.5 go from 11 to 23, the week a checkout outage hit.

```sh
deem run histogram "is the user reporting a problem?" week38.txt
deem run histogram "is the user reporting a problem?" week39.txt
```

### `mix`

Each row is an option. After them come the pairs of options that are often close (the winner less than 0.15 ahead), with the number of lines where they were, and then the baseline: what the model picks for an empty line.

```console
$ deem run mix "topic?" bug,billing,refund,other sample.txt
bug	133	██████████████████
billing	58	████████
refund	10	█
other	99	█████████████
--------------------
close bug/other	3	█
close bug/billing	2	█
--------------------
baseline other	1.0
baseline billing	0.0
baseline bug	0.0
baseline refund	0.0
```

With no close calls there is no such part, only the baseline.

Here the options rarely come close, so the model tells them apart. A pair with many close calls overlaps in the model's eyes: merge the two options or reword one. A large share for "other" can mean your options are missing a category: here 99 tickets are questions, feature requests and praise, which none of the topics covers. The baseline goes to "other", as it should; a baseline on a real topic would mean the model leans toward it when it can't place a line.

### `disagree`

`disagree` asks two wordings of a question and prints only the lines where they would make different decisions: one keeps the line and the other drops it, at the threshold. Lines with the biggest disagreement come first.

```console
$ deem run disagree "is this a problem?" "is the user complaining?" sample.txt
0.87	0.16	our SSO certificate expires tomorrow, how do we update it?
0.83	0.19	my card expired, how do I update it before the next charge?
0.93	0.38	I lost my phone and with it my 2FA, how do I get back in?
...
```

There is one score column per question, in the order you gave them, and the record is `cut -f3-`. To count the lines that differ, add `| wc -l`: 18 here.

Reading the lines usually shows at once which wording means what you mean. Here "a problem" includes a situation the user needs help with, and "complaining" only the ones where something went wrong. To compare a third wording, run it against the better of the first two.

### `deem tune`

`tune` is a command rather than a program, because it asks you questions on the terminal; its input can come from a file or a pipe. The README shows a run. Twenty labels give a good first threshold. For a closer one, label more: `-n 50` shows 50 lines, 40% of them spread across the scores and the rest around where your "no"s turn into "yes"s, which is where the threshold is decided. More labels usually narrow the range of thresholds that agree with you equally well. It scores every line it reads, so run it on a sample, not a large file. The last line it prints, on stdout, is the command to use; everything else goes to the terminal. To keep a tuned question, save it as a program with the threshold as its default ([Saving a program](#saving-a-program)).


## How it works

[DESIGN.md's Terms](DESIGN.md#terms) defines the words used here (record, question, round, wave, a record's run, journal, …). Two decisions shape almost everything about how `deem` behaves: **waves** (what a record has asked goes out together when it needs an answer) and **cascades** (a chained `select()` skipping questions for records already filtered out). **[`DESIGN.md`](DESIGN.md)** has the diagrams: how requests are built, how a program's questions become waves, and why.

`deem` runs up to 192 records at a time. A program asks a question and carries on until it needs the answer; then every question its record has asked goes out together, a request for each value judged with all its questions in it: the record's next *round*. When the round is answered, the record runs again from the start, now with those answers; what it printed before is not printed again. Wave k is every record's k-th round, so a program's waves follow from what it asks and looks at, never from timing, and no record waits for another. Chain two questions with `select()` and a record that fails the first one never asks the second. That's a cascade, and it costs nothing extra to get: it falls straight out of how `select()` already works. Identical questions about identical records are asked once per run.

Each request carries one value, the record's context, and every question about it, as the API documents ([DESIGN.md](DESIGN.md#requests) shows a request). So each answer depends on its own value alone, and a question can name a field of a JSON value in backticks: ``ask("does `photo` support `claim`?"; {claim, photo})``. The hosted API takes 80 requests a second by default, so deem judges about 80 values a second ([Cost and rate limits](#cost-and-rate-limits)).

The [`decision-tree`](../examples/.deem/decision-tree.deem) example walks every log line down a tree of questions. The lines start down the tree together, so `deem trace --visual` shows the first level going out as one wave, and each line's next question following as soon as its own first answer is back:

<p align="center">
  <img src="demo/decision-tree.gif" width="90%" alt="deem classifies log lines with a decision tree, and its trace shows the first level's requests going out together and each line's second question following its first answer">
</p>

### Questions that don't depend on each other go together

**`ask` doesn't call the API.** It queues the question and returns a promise, and the program runs on. Queued questions are sent together when the program needs a promise's value, so every question a record asks before then goes out in the same round, in one request for each value it judges. Then the record runs again with the answers, and gets as far as the next answer it needs.

This only changes when questions are sent, never which ones: a question that depends on an answer always waits for it, and a program asks the same questions, and does the same with their answers, as it would if each `ask` waited for its answer.

```jq
{text: ., notice: ask("would a user notice?"; .),
 severe: ask("is this severe?"; .)}
```

Across 500 records this sends each record's text once, with both questions attached: 500 requests, 192 records at a time. Asking one question at a time would take twice the waves, one round trip after another. The same holds for `[ask(a; .), ask(b; .)]`, `ask(a; .), ask(b; .)`, `ask(a; .) as $x | ask(b; .)`, `ask(a; .) * ask(b; .)`, and `.[] | ask(q; .)` over an array inside one record: one wave.

Operations that need an answer's value, and so wait for it (waiting is never an error a `try` could catch; a failed answer is an error at its `ask`):

- comparisons: `ask(q; .) > 0.5`, `==`, sorting (`sort_by`, `max_by`, …)
- conditions: `select`, `if`, `//`, `and`, `or`, `not`
- iterating over it or measuring it: `.[]`, `length`, `test`, `@tsv`, and most other builtins
- asking a question built from it: `ask("given \(ask(a; .)), is this severe?"; .)` waits for `a` before it is sent
- printing it

Operations that pass a promise along without waiting: building objects and arrays, `.key`, `tostring` and string interpolation, arithmetic, and binding with `as $x`.

So *where* a program first needs an answer decides how many waves it takes. Two pairs of programs show it. Each pair asks the same questions and prints the same records.

**A chain of conditions.** Keep a record only if all three answers are yes:

```jq
# 3 waves: `and` compares one answer before it asks the next question
select(ask("is the user reporting a problem?"; .) > 0.5
   and ask("is their problem severe?"; .) > 0.5
   and ask("are they not happy?"; .) > 0.5)

# 1 wave: ask all three, then compare
[ask("is the user reporting a problem?"; .),
 ask("is their problem severe?"; .),
 ask("are they not happy?"; .)] as [$p, $s, $u]
| select($p > 0.5 and $s > 0.5 and $u > 0.5)
```

In the first, `and` needs to know whether the first answer is above 0.5 before it goes on, so the program waits after every question: three trips, one question each. That's a cascade: a record whose first answer is no stops there, and its other two questions are never asked. In the second, building the array doesn't need the answers, so all three questions are queued by the time `select` compares, and they travel together. Every record is asked all three.

That's the trade. Over 100 lines of which 10 report a problem, the chain asks about 115 questions in 3 waves, the array 300 questions in 1. Chain when an early question rules out most records, and put the one that rules out the most first; ask everything at once when there are few records, or when the waiting is what hurts.

**A map over an array.** Ask about each element of `.xs` and keep the ones above 0.5:

```jq
# a wave per element
.xs | map(select(ask(q; .) > 0.5))

# 1 wave
.xs | map(ask(q; .)) | map(select(. > 0.5))
```

`map(f)` finishes `f` on one element before it starts the next. In the first, `f` asks and then compares, so it waits for that element's answer before the next element's question is even written: for `.xs = ["a", "b", "c"]`, three trips of one question each. In the second, the first `map` only asks, and returns three promises with all three questions queued; the second `map` compares, and its first comparison sends all three together. Unlike the chain, the slow version saves nothing: every element is asked either way. It also costs work: each trip runs the record again from the start, so a record with a thousand trips redoes its earlier work a thousand times. That's small next to the trips themselves, but asking first, then comparing, saves both.

The same rule decides the cost of a program with `input: "all"`, where `.` is the array of every record: `[.[] | ask(q; .)] | map(select(. > 0.5))` is one wave for the whole input (a request per line), and `.[] | select(ask(q; .) > 0.5)` is a wave per record. Build the array of answers first, then compare. Such a program is one record, so it may have as many questions unanswered at once as a full window of records could, 64 each.

Waves are shared across records. Up to a window of records run at once (192), and a wave carries whatever all of them are waiting on, so the slow `map` takes as many waves as the longest `.xs`, not one per element of every record.

Waves overlap. A record whose round comes back early carries on while slower requests are still out, so one slow request doesn't hold up the records it doesn't concern.

### A cascade is a pipe of deems

A cascade sends the same requests as a pipe of deems, one deem per question. This program

```jq
select(ask("is the user reporting a problem?"; .) > 0.5)
| select(ask("is it about billing?"; .) > 0.5)
```

and the pipe `deem "is the user reporting a problem?" feedback.txt | deem "is it about billing?"` each send a request per line for the first question, then one for each line that passed for the second, and print the same lines (`a_cascade_sends_what_a_pipe_sends` in `tests/cli.rs`). The program takes two waves; each deem in the pipe takes one. So a program's waves read like the stages of a pipe: wave 2 is what a second deem would be asked about the lines the first one kept. The two deems run at the same time: the first prints a line once it and the lines before it are done, and the second asks about it while the first is still reading (`streaming_input_is_answered_as_it_comes`).

The pipe is two runs, though, and that shows:

- **Exit code.** The shell reports the last deem's. A record that fails in the first deem makes it exit 2, which you see only with `set -o pipefail`; a program has one exit code.
- **`--failed`, `--stats` and `--trace`** are per deem. A record that fails in the first deem never reaches the second, so the second's `--failed` doesn't list it.
- **What the second deem judges** is the first one's output line. After a filter that's the line as read; after `-p`, `-o` or `-l` the answer's column is in front of it, and the model sees that too. Put `cut -f2-` between them, or filter first.
- **Answers kept.** A deem that filters prints the line, not its score. A program can keep both answers: `{text: ., problem: ask(…), billing: ask(…)}`.
- **Repeats.** Identical questions are asked once per run, so once per deem, not once per pipe.
- **Rate.** Each deem paces itself to the API's whole rate and its `parallel` ([APIs](#apis)), so two deems can have twice as many requests at a server of your own, and at the hosted API they meet `429`s and slow down until they share it ([When an answer fails](#when-an-answer-fails)).

A pipe of question forms is enough for a filter followed by a filter. Use a program when a later question depends on what an earlier one answered (`choose` and then a question for each topic), when it judges a field of a JSON record, or when you want the answers kept together in one record.

### A record runs more than once

When a record needs an answer it doesn't have yet, its run stops, the questions it has asked go out, and once they are answered the record runs again from the start, with them. The run that finishes has every answer, so it does what jq would do if each `ask` had waited for its answer: a failed answer is an error at its `ask`, which `try` catches ([When an answer fails](#when-an-answer-fails)). [DESIGN.md](DESIGN.md#replay-running-a-record-again) shows a record's runs step by step. What it means for a program:

- **The work is done again; its effects aren't.** Each run computes everything from the start, but an output, a `debug` or `stderr` line happens once, in the first run that gets to it with every answer before it; later runs skip it. `now` is read there too, and later runs get the same time. A question already answered is answered from the record's journal, not asked again.
- **A question the program asks is sent, even if nothing uses its answer**: `ask(q; .) as $x | "done"` sends one, as its answer could fail, which is an error there. A question the program never reaches is never sent.
- **Every round trip runs the record again.** For a line that costs nothing you'd notice. A record that waits hundreds of times (a comparison inside a loop over a large array, a `select` at every node of a tree, `group` with hundreds of groups) redoes its work each time: about 8% more on `group`'s 193 rounds. Asking first and looking after saves the rerun along with the round trip.
- **A time read after an unanswered question** is read by the run that has the answer, so `ask(q; .) as $a | ask("at \(now)?"; .)` asks its second question twice, once with a time that is thrown away. Read the time first: `now as $t | …`.

### Seeing a run

`--trace FILE` records the run, one event per line, and `deem trace FILE` prints it: each wave, the requests it sent, which record each request carried with which questions, and what each question had to wait for.

```console
$ deem run --trace run.jsonl '
    {notice: ask("would a user notice?"; .),
     service: ask("which service is this about?"; .),
     severe: ask("given \(ask("would a user notice?"; .)), is it severe?"; .)}
  ' app.log
$ deem trace run.jsonl
5 records · 15 questions · 10 requests · 2 waves · 1023.8 ms
#N numbers a question; "after #M": it waited for #M's answer since its last

wave 1 at 4.9 ms: 10 questions in 5 requests
  request 1 · 4.9 ms–559.2 ms · 1 record, 2 questions · ok
    record 3     #3 "would a user notice?", #5 "which service is this about?"
  request 2 · 4.9 ms–571.5 ms · 1 record, 2 questions · ok
    record 2     #1 "would a user notice?", #4 "which service is this about?"
  …

wave 2 at 379.1 ms: 5 questions in 5 requests
  request 6 · 379.1 ms–970.4 ms · 1 record, 1 question · ok
    record 1     #11 "given 0.94, is it severe?"   after #2
  request 7 · 485.3 ms–984.6 ms · 1 record, 1 question · ok
    record 4     #12 "given 0.07, is it severe?"   after #7
  …

records
  record 1      965.8 ms  done · waited on #2 for 374.1 ms, #11 for 591.3 ms
      · reused #2
  record 2      979.2 ms  done · waited on #1 for 566.7 ms, #15 for 412.2 ms
      · reused #1
  …
```

Read it top down. Each wave is a round trip, and each request carries one record with the questions it had asked by then: record 2 asked #1 and #4 in request 2. `after #1` is why #15 waited for wave 2: its text needed #1's answer. Wave 2 is every record's second round: each record's `severe` question went the moment its own first round came back, record 1's at 379 ms and record 2's at 572 ms, none waiting for another's. Which records' questions make up each wave depends only on the program and its answers, so it is the same on every run; only the times differ. Questions are numbered in the order they were asked, across records. The `records` part shows how long each record ran, what it waited on, and what it `reused`: the second `ask("would a user notice?"; .)` was answered from the first, not asked again. (This run used a test stand-in for the model, so the times and scores are only illustrative.)

`deem trace --visual FILE` draws the same run as a timeline: a row per request and per record, on one time axis. An arrow runs into each request from the requests whose answers it needed, so you can see what held it back:

```console
$ deem trace --visual run.jsonl
5 records · 15 questions · 10 requests · 2 waves · 1023.8 ms
█      a request in flight
━      a record running
░      a record waiting for an answer
┐ └    the lower request needed the upper one's answer, so it went out after it
[A B]  the questions a request carried, as listed under "questions"

             0 ms                                1023.8 ms
waves        ▼               ▼
request 1    ████████████████████████┐                       [A B]
request 2    █████████████████████████┐                      [A B]
request 3    █████████████████████┐  ││                      [A B]
request 4    ████████████████┐    │  ││                      [A B]
request 5    ███████████████████████┐││                      [A B]
request 6                    └█████████████████████████      [C] after request 4
request 7                         └█████████████████████     [C] after request 3
request 8                           └████████████████        [C] after request 5
request 9                            └███████████████████    [C] after request 1
request 10                            └█████████████████     [C] after request 2
record 1     ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
record 2     ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
record 3     ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░━
record 4     ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
record 5     ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░

questions (… stands for a number: an answer, or one from the record)
  A  "would a user notice?" · 5 in requests 1, 2, 3, 4, 5
  B  "which service is this about?" · 5 in requests 1, 2, 3, 4, 5
  C  "given …, is it severe?" · 5 in requests 6, 7, 8, 9, 10 · after A
```

Requests 6 to 10 carried the `severe` questions (C), and each needed the score (A) from its own record's first request, so it went out the moment that answer came back: request 6 after request 4, request 7 after request 3, and so on. The records spent the run waiting: the time is in the round trips, which is why a record's questions travel together. Where a request went out later than the answer it needed, `└─▶` shows the gap; here there is none, as no record waits for another. Where one request needed several, their arrows join (`┴`), and a line passing down to a later request crosses as `│`, `├` or `┼`.

`questions` groups the questions by text, with numbers masked, so an `ask` whose text carries an earlier answer is one group: roughly one group per `ask` in the program. A `choose` answer in a question's text makes a group per option. `after` is the dependency between the groups: the answers a record waited for since its previous question, at a `select`, an `if`, or a string that interpolates an answer. An answer the record already had when it asked isn't listed: it held nothing back. The first 40 requests and 40 records are drawn; the groups are all listed.

Traces of runs against different APIs can be compared, answer by answer: `deem trace compare` ([Comparing APIs](#comparing-apis)).

**What was sent, and what came back.** Each `request` event holds the request's `body`, exactly as deem sent it: the `model`, the value judged as its `state`, and its `questions`, named `q0`, `q1`, … in the order of the event's `questions` list. A picture appears by its ID, as sent, and `pictures` names the file each ID was read from; a request whose pictures couldn't be read isn't sent, and has no body. The `answered` event that follows holds the API's `response`, as it was received, which answers each question by the same name, even a reply deem couldn't use; a request that got no reply, or an error status, has none, and its `result` says why. The log is JSON lines, so a program that asks nothing reads it:

```sh
deem run -J 'select(.ev == "request") | .body.state' run.jsonl   # values sent
deem run -J 'select(.ev == "answered") | .response' run.jsonl    # replies
```

So a trace holds your data: every value deem judged, as the API saw it. Keep it as you would the input, and expect it to be about as large as what was sent.

Events are written as they happen, each one whole, so a trace of a long run grows on disk, not in memory, and a run that hangs can be killed and still read. Whatever was still open then, such as a request that never came back and the records waiting on it, is reported as `still out when the trace ends` or `still running when the trace ends`.


## When an answer fails

An answer can fail: the API is unreachable or rejects the request after its retries, or the model's answer isn't what the question asked for (a number outside [0,1], an option that wasn't offered). In deem, **a failed answer is a jq error where it was asked**, as if `ask` had called `error`. `try` and `?` catch it like any other error; uncaught, it fails its record, and the other records are unaffected. ([DESIGN.md](DESIGN.md#when-an-answer-fails) has how.)

A failed record prints nothing more, its reason goes to stderr with the record's number, and the run exits 2. `--failed FILE` writes each record that didn't finish to `FILE`, so it can be run again with the same flags: a text record as it was read, one per line (separated by NUL with `--split`, since a record may then hold newlines), and with `-J` each value as compact JSON, one per line.

The examples below point deem at a port where nothing listens, so every answer fails the same way, after deem has retried it five times (about 7 seconds). They run as written from `examples/`.

```console
$ export JEV_BASE_URL=http://127.0.0.1:9
```

`try` catches a failed answer, as it would any error, and deem says how many it caught, since the output alone can't show it:

```console
$ head -1 app.log | deem run 'try ask("is this an error?"; .) catch 0'
0
deem: 1 failed answer was caught by a try or ? in the program
$ echo $?
0
```

`//` doesn't catch it: in jaq, `//` passes errors on. Uncaught, the answer fails its record:

```console
$ head -1 app.log | deem run 'ask("is this an error?"; .) // "no answer"'
deem: record 1: the answer to "is this an error?" failed: request failed:
  io: Connection refused (os error 111)
$ echo $?
2
```

`--failed` collects what to run again. Here both records failed, so both are in it:

```console
$ head -2 app.log \
    | deem run --failed retry.log 'select(ask("is this an error?"; .) > 0.5)'
deem: record 1: the answer to "is this an error?" failed: request failed:
  io: Connection refused (os error 111)
deem: record 2: the answer to "is this an error?" failed: request failed:
  io: Connection refused (os error 111)
deem: 2 of 2 records failed; they are in retry.log
$ wc -l < retry.log
2
```

`--failed` empties its file when a run starts, so deem refuses one that is also an input: a rerun that collects failures again names another file. Rerun them later, once the API is back, and append: `deem run 'select(ask("is this an error?"; .) > 0.5)' retry.log >> errors.log`. Or give them a fallback with ordinary tools: `jq -Rc '{line: ., error: null}' retry.log`.

What keeps failures rare, and loud when they happen:

- **Retries.** A refused or dropped connection, a timeout, a 408 or a 5xx is retried five times, backing off from 250 ms to at most 5 s. A 429 or 529, the API asking deem to slow down, halves deem's rate and is retried longer, waiting as long as the API's `Retry-After` says (up to a minute). Two deems on one key share its rate this way instead of failing each other's records. When retries hold deem up for 10 seconds, it says why, once.
- **A wrong setting stops the run at once.** A `JEV_BASE_URL` without `/v1` (404) or one that redirects, a key the API rejects, or a model it doesn't know stops the run on the first reply, with one message naming the setting to check: the variable, or the setting of the API from the config file (`check the key of API kev`). What the server said is in the message as text: escape codes written out, and the key, if the reply repeats it, as `<key>` ([TRANSPORT.md](TRANSPORT.md), rule 13).
- **So does a failure that keeps coming.** When ten requests in a row bring no usable answer (they failed after their retries, were refused, couldn't be read, or every answer was invalid), deem stops with one message and the last reason: `the API is unavailable: …` for an outage, `no usable answer to 10 requests in a row, …` otherwise. After any stop nothing more is sent, retries included. With `--failed`, what was read and didn't finish goes to the file, and deem says how many records it had read, so a rerun can pick up the rest.
- **Failures are counted.** The first five records that fail are listed as they fail, the rest counted, and a run of more than one record ends with how many failed of how many were read, how many more a stop left unfinished, and where `--failed` put them all. Failed answers a program caught are counted too.
- **One bad question fails one record.** The API refuses a request as a whole, and a request carries one record, so only the record with the bad question fails.
- **Mistakes in a question fail where it's asked.** A question the API can't take, such as a `choose` with more than 255 options, is an ordinary jq error at the call, before anything is sent, and `try` catches it like any other:

```console
$ head -1 app.log \
    | deem run 'try choose("which one?"; [range(300) | tostring]; .) catch "x"'
x
```

A question the program asks is sent even if nothing looks at its answer, since its failure is an error there:

```console
$ head -1 app.log | deem run 'ask("is this an error?"; .) as $x | "unused"'
deem: record 1: the answer to "is this an error?" failed: request failed:
  io: Connection refused (os error 111)
```

A question the program never reaches costs nothing:

```console
$ sed -n 2p app.log \
    | deem run 'if test("ERROR") then ask("is it bad?"; .) else "not asked" end'
not asked
```

| program, when the answer fails | prints |
|---|---|
| `try ask(q; .) catch 0` | `0` |
| `try ask(q; .) catch .` | the reason: `the answer to "q" failed: …` |
| `ask(q; .)?` | nothing |
| `ask(q; .) // 0` | nothing; the record fails |
| `select(ask(q; .) > 0.5)` | nothing; the record fails |
| `ask(q; .) as $x \| "unused"` | nothing; the record fails |
| `try choose(q; [300 options]; .) catch "x"` | `"x"`: an error at the call, nothing sent |
| `try (ask(q; .) + "s") catch "bad"`, the answer a number | `"bad"` |


## Gotchas

The first two fail with an error; the others fail silently or only warn. An answer that fails is a jq error where it was asked: see [When an answer fails](#when-an-answer-fails).

**1. Arguments are separated by `;`, not `,`.**

```console
$ deem run 'choose("which service?", ["api","db"])' app.log
deem: 1:1: filter choose/1 is not defined
    choose("which service?", ["api","db"])
    ^
```

In jq a comma means "emit both", so this is a one-argument call. Write `choose("which service?"; ["api","db"]; .)`.

**2. Numbers need a leading digit.** jq accepts `.5`; deem doesn't:

```console
$ deem run 'select(ask("would a user notice this?"; .) > .5)' app.log
deem: 1:47: expected semicolon or right parenthesis, found "5"
    select(ask("would a user notice this?"; .) > .5)
                                                  ^
  numbers need a leading digit: write 0.5, not .5
```

**3. Comparing a number to a string is legal and always false.** jq orders `null < false < true < numbers < strings < arrays < objects`, so a quoted threshold matches nothing and reports no error:

```console
$ deem run 'select(ask("would a user notice this?"; .) > "0.5")' app.log
```

Compare against a number: `> 0.5`.

**4. Nothing selected is exit 1 with no output**, same as `grep`. If a filter matches nothing, look at the raw scores:

```console
$ deem "would a user notice this?" -t 0.8 app.log
$ echo $?

1

$ deem "would a user notice this?" -p app.log

0.54	Oct 19 14:22:01 api-7f4 ERROR upstream timeout after 30s for /v1/checkout
0.3	Oct 19 14:22:03 worker-2 INFO retrying job 8831
0.32	Oct 19 14:22:11 db-primary WARN replication lag 4200ms
0.74	Oct 19 14:23:44 api-7f4 ERROR 502 from payments-svc, 118 requests affected
0.21	Oct 19 14:24:02 worker-2 INFO job 8831 completed
```

The highest score was 0.74 and the threshold was 0.8. `--stats` helps here too, and so do `deem run histogram` and `deem tune` ([Programs that come with deem](#programs-that-come-with-deem)).

**5. `sort -rn` misorders scores if your locale uses a decimal comma.** `sort` reads numbers using `LC_NUMERIC`; in locales such as `de_DE` or `fr_FR` it stops at the `.`, so every score between 0 and 1 counts as `0`. Check with `locale | grep NUMERIC`, and fix it with `LC_ALL=C sort -rn` (or `export LC_NUMERIC=C` for the whole shell). `sort -g` has the same problem.

**6. Long values are cut before the model sees them.** Anything judged is limited to `DEEM_MAX_RECORD_BYTES` (32 KiB). A longer string is cut, and the model sees only its start; deem says how many were cut on stderr. A value that is not a string, such as a whole JSON object, cannot be cut and fails its record instead:

```console
$ deem run -J 'ask("is this a complaint?"; .)' big.jsonl

deem: record 1: judge: the value to judge is 50013 bytes, over
  DEEM_MAX_RECORD_BYTES (32768). Judge a smaller part, e.g.
  `.body | .[0:4000] | ask(…; .)`, or raise the limit
```

The limit applies to what is judged, not to the input line, so a record with a large field you never ask about is fine, and the record printed is always the whole one. To choose what the model sees, cut it yourself (`.body | .[0:4000] | ask(…; .)`); to send more, raise the limit.

**7. `now` and local time make a program depend on when and where it runs.** A rerun from `--failed`, or the same run next week, reads another time; `localtime` and `strflocaltime` use the machine's time zone, and a question built from them sends it. Pass times in the data, and use `todate`, `gmtime` and `strftime`, which are UTC and depend only on their input.


## Differences from jq

deem runs jaq's dialect of jq. A program that doesn't call `judge` prints what jaq prints: deem's tests run jq's own test suite under both, and the two exceptions are deem's own choices (`tests/golden/jaq.skip`). jaq differs from jq 1.7.1 in places, and [jaq's README](https://github.com/01mf02/jaq#differences-between-jq-and-jaq) lists them. The table has the ones everyday programs run into, found by running common programs under deem and jq 1.7.1 (`tests/golden/everyday.tsv`, differences in `everyday.diff`), plus what deem changes itself.

| | jq 1.7.1 | deem |
|---|---|---|
| `judge`, `ask`, `choose` | not defined | added |
| a failed answer | — | a jq error where it was asked: [When an answer fails](#when-an-answer-fails) |
| `env`, `$ENV` | the environment | `{}` |
| `import`, `include`, `input`, `inputs`, `input_filename` | defined | not defined: they could leak data or tie records together ([Safety](#safety)) |
| `$__loc__` | defined | not defined in jaq |
| `module {…};` | any metadata, ignored | an object with `input` and `defaults` only; anything else is an error ([Program files](#program-files)) |
| `.meta.score = 0.5` where `.meta` is missing | creates `.meta` | an error; with `?`, silently wrong: [Setting a field under a missing one](#setting-a-field-under-a-missing-one) |
| `.[5] = 1` past the end of an array | pads with `null` | an error |
| `del(.[0,2])`, several paths at once | deletes them together: `[2,4]` | one at a time, so indexes shift: `[2,3]` |
| `join` with a `null` element | `""` | `"null"` |
| `from_entries` | takes `key`, `k`, `name`, `Name`, `K`, `Key` | takes `key`, `k` |
| `"ab" * 0` | `""` | `null` |
| `4 / 2`, `2 * 1.5` | `2`, `3` | `2.0`, `3.0`: a division, or arithmetic with a decimal, gives a float, printed with its point |
| `1 / 0` | an error | `Infinity` |
| `nan`, `infinite`, `{(1): 2}` | printed as JSON (`null`, a large number) or an error | printed as `NaN`, `Infinity`, `{1:2}`: not JSON |
| a number literal like `1.0e3` | `1.0E+3` | as written |
| `.5` | `0.5` | a syntax error: write `0.5` |
| destructuring alternatives `?//`, `{$a: pattern}` | supported | syntax errors |
| `fromjson`, `tonumber` on `"2a"` | an error | an error (jaq itself reads `2`) |
| error messages (`try … catch .`) | jq's wording | jaq's wording |
| `halt`, `halt_error` | stop the run | end the record |
| input | JSON values | text lines; JSON values with `-J` ([Input](#input)) |
| output | pretty-printed | compact; strings raw when the input is text, or when the program defines `main` |

### Setting a field under a missing one

Annotating records is most of what deem is for, and this is the difference from jq you are most likely to meet. jaq creates a missing field, but not a missing *parent* of one:

| program, on `{"msg":"x"}` | jq 1.7.1 | deem (jaq) |
|---|---|---|
| `.meta = 0.5`, `.meta \|= 0.5` | sets `.meta` | the same |
| `.meta.score = 0.5`, `\|=`, `["score"]` | `{"msg":"x","meta":{"score":0.5}}` | the record fails: `cannot use null as iterable (array or object)` |
| `.meta.tags[0] = "new"` | creates the array | the record fails, the same way |
| `try (.meta.score = 0.5) catch "caught"` | sets it | `"caught"` |
| **`.meta.score? = 0.5`** | sets it | **`{"msg":"x","meta":null}`: no error, and nothing set** |

`.meta.score = 0.5` is an update inside an update: the outer one finds no `.meta` and hands `null` to the inner one, and jaq updates only objects and arrays. With `?`, jaq keeps the value it couldn't update, so the record gets `"meta": null` and no score. Don't reach for `?` to silence the error.

Merge instead. `*` on two objects merges them recursively, so this does what `.meta.score = 0.5` does in jq, whatever is already there:

```jq
. * {meta: {score: ask("would a user notice?"; .)}}
```

`.meta += {score: …}` also works one level down, and keeps `.meta`'s other fields. It is not the same for deeper paths: `.a += {b: {c: 1}}` replaces all of `.a.b`. An existing parent is fine as it is: `.meta.score = 0.5` works when `.meta` is an object.

**deem can print values that aren't JSON.** jaq prints NaN as `NaN`, the infinities as `Infinity` and `-Infinity`, an object key that isn't a string as it is (`{1:2}`), and a byte string (from `tobytes`) as `b"…"`, where jq would quietly turn them into something else (`null`, the largest number) or refuse. So `1 / 0`, `nan`, or `{(.n): .x}` with a numeric `.n` can put a line on stdout that `jq` and other JSON readers reject. deem says so on stderr the first time it happens in a run, and names the record:

```console
$ echo '{"n": 0}' | deem run -J '{ratio: (1 / .n)}'
{"ratio":Infinity}
deem: record 1 printed a value that isn't JSON (NaN, Infinity, a non-string
  object key, or a byte string); see "Differences from jq"
```

The exit code doesn't change. Such a value can't be judged: `ask(q; nan)` fails at the call. Guard a division with `if .n == 0 then null else 1 / .n end`, and turn a key into a string with `tostring`.


## The whole interface

A program is written in **jq's language** as jaq runs it, with one added function and four small helpers built on it. `deem QUESTION` and its `-p`, `-o` and `-l` forms run one-line programs of this kind (the shapes in [Programs are jq](#programs-are-jq) show them), and `deem run` runs any.

```
judge(spec; ctx)   judge ctx → {value, confidence, probabilities}
```

`spec` is an object, and its shape picks the answer type:

| spec | `.value` is |
|---|---|
| `{q: "…"}` | a calibrated float in `[0,1]` |
| `{q: "…", among: ["a","b","c"]}` | one of the options |
| `{q: "…", among: {a: "what a means", b: "…"}}` | one of the names |
| `{q: "…", levels: ["low", "medium", "high"]}` | a number in `[0, n−1]` on the scale, lowest level first |
| `{q: "…", box: true}` | where in a picture: `{top, bottom, left, right}` ([Pictures](#pictures)) |

`among` as an object tells the model what each option means: a description is any JSON, `null` for none (`choose("topic?"; {billing: "invoices, refunds, charges", bug: "something is broken", other: null}; .body)`). The answer is still a name, and a different description is a different question. `levels` is a scale of 2 to 10 levels, each described by any JSON: the value is where the answer falls on it, `0` for the first level and `n−1` for the last, and it can fall between two (`1.4`); `probabilities` are keyed by level, `"0"`, `"1"`, …. `levels` doesn't go with `among`, `yes` or `no`; `box` goes with none of them, nor with examples. A yes/no question can also say what yes and no mean, `yes:` and `no:`, and give examples of each, `like:` and `unlike:` (arrays of strings): `judge({q: "is this urgent?", yes: "someone must act today", like: ["site is down"], unlike: ["typo on the about page"]}; .)`. A choice or a scale takes none of the four; it describes each option or level instead. Any other field is an error, so a misspelt one can't quietly change the question.

**The last argument is what the model sees, and nothing else is sent.** Both arguments are computed from the current record, so the question can mention what isn't sent:

```bash
ask("…"; .)                     # send the whole record
ask("…"; .message)              # send one field
ask("…"; {svc, err})            # send a projection you build on the spot
ask("…"; .body[0:4000])         # send only the first 4000 characters
ask("is a \(.level) bad?"; .msg) # the question reads .level; only .msg is sent
```

A context that produces several values asks a question about each: `ask(q; .a, .b)` gives two answers.

The model sees the context exactly as `tojson` prints it: numbers as written (`1.10`, `12345678901234567890`) and keys in their order. A picture is the exception: the model sees the picture where `tojson` would print its reference ([Pictures](#pictures)). A context that isn't JSON (`NaN`, the infinities, a non-string key, a byte string) can't be sent, so the call fails with an error that `try` catches.

`ask`, `choose`, `rate` and `box` are not builtins; they are four lines of jq, defined before every program:

```jq
def ask(q; ctx):          judge({q: q}; ctx).value;
def choose(q; opts; ctx): judge({q: q, among: opts}; ctx).value;
def rate(q; levels; ctx): judge({q: q, levels: levels}; ctx).value;
def box(q; ctx):          judge({q: q, box: true}; ctx).value;
```

There are no flags for choosing fields or routing uncertain answers; those are written as jq expressions, for instance `if judge({q:…}; .).confidence < 0.75 then … else … end`.


## Calibrated answers

deem's floats are calibrated: across a corpus, records scored `0.8` are positive about 80% of the time. That is **an aggregate property, not a per-record one**: a `0.7` does not mean "this line is 70% likely to be an outage", it means the *set* of lines scored `0.7` is about 70% outages.

Three things this buys you:

**A threshold says how sure you want to be.** Of the lines scored `0.9`, about nine in ten are ones the model reads as yes, whatever the question or the file. That is the model's yes, though, not yours: where a question means a little more or less to the model than to you (a grumble counted as a problem), the threshold you want moves, and only your own labels show where to. `deem tune` finds it from them.

**Counts without thresholds.** Summing scores estimates how many records are positive, and beats counting the ones over a threshold because it does not throw away the uncertain middle:

```bash
deem run 'ask("is this a customer-facing outage?"; .)' app.log \
  | awk '{ total += $1 } END { print total }'
```

**Thresholds from cost.** If a false positive costs one unit of triage time and a false negative costs twenty, your threshold is `1/(1+20) ≈ 0.05`. That arithmetic is valid only because the scores are calibrated.

### Combining answers

jq has the C math library, so combinations are one-liners, and the questions in each go out together, in one wave:

```jq
ask("outage?"; .) * ask("customer-facing?"; .)                       # product
1 - (1 - ask("security risk?"; .)) * (1 - ask("data loss risk?"; .)) # noisy-OR
0.7 * ask("would a user notice?"; .) + 0.3 * ask("is this novel?"; .)# weighted
```

⚠️ **A product of calibrated scores is not itself calibrated.** `P(A)·P(B) = P(A∧B)` only when A and B are independent, and two questions about the same log line rarely are. Use combinations to rank; when you want a conjunction, ask for it in one question.

### Check the calibration, don't assume it

Label some records by hand as `label<TAB>record`, `1` or `0`. Score them, put the scores next to the labels, and compare:

```bash
cut -f2- labeled.tsv \
  | deem run 'ask("would a user actually notice this?"; .)' \
  | paste - labeled.tsv \
  | awk -F'\t' -v t=0.5 '
      { p = $1 > t; tp += p && $2; fp += p && !$2; fn += !p && $2
        sum += $1; pos += $2 }
      END { printf "precision %.3f  recall %.3f  expected %.1f vs actual %d\n",
                   tp+fp ? tp/(tp+fp) : 0, tp+fn ? tp/(tp+fn) : 0, sum, pos }'
```

This works because deem prints one answer per record, in input order. If the expected count (the sum of scores) is far from the actual count, the scores are not calibrated on your data.

## What composes with what

1. **`deem | deem -J` works.** JSON out is JSON in (unless a value can't be JSON: NaN, the infinities, a non-string key; see [Differences from jq](#differences-from-jq)). Text out, such as the question forms print, is text in, so `deem "Q1" | deem "Q2"` needs no flag. A picture is printed as its path, so after pictures the second deem takes `-I`: `deem "Q1" screens/*.png | deem -I "Q2"`.
2. **`deem | sort | awk | jq | head` works.** Text in gives raw lines out, and a closed pipe ends the run promptly (exit 141, like any program killed by SIGPIPE).
3. **Input order out = input order in**, regardless of waves or completion order.
4. **Exit codes compose.** `if deem -e …; then`, `deem … && …`.
5. **`fd`, `find -print0` and `xargs` feed it.** `--split '\0'` for NUL-separated input.

Output follows input (raw strings for text, compact JSON with `-J`), which is what makes 1 and 2 both true at once.

### Input

deem never guesses what its input is: a file is a picture if it starts like a PNG or JPEG, which text never does; otherwise the command line says how its text is read.

- **A picture file is one record** ([Pictures](#pictures)). A run's files are all pictures or all text. With `-I`, each line of stdin is a picture's path.
- **The input is UTF-8 text.** A byte-order mark at the start of the input, or of each file, is dropped. Bytes that aren't UTF-8 become U+FFFD, as in jq, so such a line is printed with that character in their place.
- **Without `-J`, each line is a record**, a string, even if it looks like JSON. A line ends at `\n`, and a `\r` before it is dropped; empty lines are skipped. `--split RE` cuts records at another separator instead, and `-s` makes the whole input one record.
- **With `-J`, each JSON value is a record**, read as jaq reads it, however it is laid out: one per line, several on a line, or pretty-printed across many. An array is one value, so one record; to run on each element, split it first with `jq -c '.[]'`. With `-J -s`, the record is an array of every value in the input. `-J` and `--split` don't go together: a JSON value ends where it ends.
- **Invalid JSON stops the input where it breaks.** The records before it run and print, nothing after it is read, and deem exits 2, naming the record and line: `deem: reading the input: record 4812 (line 4812) is not JSON: …; the input stops here`. A log cut off mid-write loses only its last value; to skip a bad line in the middle, clean the file first (`jq -c . 2>/dev/null`).
- **A picture in text stops the input there**, as invalid JSON does: `cat photo.jpg | deem "Q"`, or a picture after text on stdin, gives `deem: reading the input: record 1 is a picture: pass picture files as arguments, or their paths with -I; the input stops here`. deem reads a picture's bytes when it sends them, so a picture must be a file.
- **A forgotten `-J` is named.** A program that reads a field of a text record (`.body` of a line) fails on every record, before asking anything, and deem ends with `the input was read as text; pass -J to read JSON records`. The question forms work either way, but without `-J` the model is sent each line as a string, not the object, so pass `-J` for JSON.


## Pictures

A PNG or JPEG file named on the command line is a picture. As input, each is one record, printed as its path; with `-I`, each line of stdin is a picture's path. `--img NAME FILE` binds `$NAME` to a picture, as `--rawfile NAME FILE` binds a text file's text.

```sh
deem "does the screenshot show an error?" screens/*.png
deem run 'ask("does `screen` show `expected`?";
  {expected: "a declined payment", screen: .})' screens/*.png
```

**A picture is a value only deem makes.** In a program it is a string, its *reference*, which deem replaces with the picture when it sends a request, and with the picture's path when it prints. You never write one: `.` and `$NAME` hold them. Data can't make one either: each run's references carry a random token, so a line that names a file is only text, and only the pictures your program names reach the model.

**A picture is a whole string**: the value judged, a field or element in it, an option's description (`choose(q; {this: ., that: $other}; …)`), a level of `rate`, or a `like`/`unlike` example. As the API's docs recommend, put it in the value next to what it is compared with, and name each part in the question in backticks. A picture in the middle of a longer string, or in the question's text, is an error, before anything is sent:

```console
$ deem run 'ask("is \(.) loaded?"; "x")' screens/orders.png
deem: record 1: judge: a picture can't be in the question's text: put it in
  the value and name it in backticks
```

A value that is a picture alone goes as `{"image": …}`, the state the API's docs show; any other value goes as written.

**Where in a picture: `box`.** `box(q; .)` is a box around what `q` names: `{top, bottom, left, right}`, fractions of the picture's height (top, bottom) and width (left, right) from its top left corner, each the expected position of its edge (`judge` adds a `confidence`, which may be `null`, and no probabilities). It needs Jev 1.14 or later. On 2026-10-22 requests for `jev-latest` were answered by `jev-1.14.0`, boxes too (`locate` and `screen-check`, by hand: tests/live/README.md), though the API's model page still listed `jev-latest` as 1.13.0; `jev-preview` is 1.14. An older model refuses a box with `Invalid request.`, and deem says to use `jev-preview` (`JEV_MODEL=jev-preview`, or `deem api edit jev --model jev-preview`). The value must hold a picture; with two, name the one you mean (`` box("the Pay button in `screen`"; {screen: ., baseline: $baseline}) ``). A box comes back even when the thing isn't there (asked for a circle that wasn't, the API boxed something else, with confidence 0.14; asked for a photographed card's mana cost on a card that has none, it boxed the set symbol), so ask about it too, in the same request. Both questions are about the same picture, so they go in one request; the [`locate`](../examples/.deem/locate.deem) example does this and prints each picture's path with its box. The edges refer to the picture as stored (below).

```console
$ deem run '{seen: ask("is there an error message?"; .),
    at: box("the error message"; .)} | select(.seen > 0.5)' screens/*.png
{"seen":1.0,"at":{"top":0.2,"bottom":0.32,"left":0.24,"right":0.97}}
```

**What deem checks.** The API takes still PNG and JPEG pictures, 280 to 1568 pixels a side, of at most 16 MiB, and fails the whole request over one it can't take. deem reads each picture's header first, and a picture it can't send fails its record (or, given with `--img`, stops the run) with the reason, before anything is sent. It doesn't resize or rotate, and the API ignores a JPEG's orientation tag, so prepare pictures first: `magick in.jpg -auto-orient -resize '1568x1568>' out.jpg`. Other formats are named: convert a GIF, WebP or HEIC to PNG or JPEG.

**What a picture costs.** About width × height / 390 tokens each time a request shows it (measured 2026-10-16): 1,230 for 800×600, 6,300 for 1568×1568. Larger pictures cost somewhat more than that: a 960×600 screenshot cost 1,854 tokens with its question, where the rule gives 1,477 (2026-10-20, `a_picture_costs_what_deem_estimates` in the live tests). The API reads at most 32,000 tokens of the value and any one question, so about four full-size pictures fill a question; a request over that is refused before it is sent, naming its pictures. A picture in the value is shown once for all its questions in a wave; in an option, presumably once for each question that offers it (not measured yet). A question asked after an answer, in a later wave, shows the picture again: three waves on one photo cost three times as much (2026-10-18). So ask a picture's questions together, and compare the answers after; follow up only when it skips most pictures, as [`screen-check`](../examples/.deem/screen-check.deem) does.

**How it is sent.** A request with pictures goes as `multipart/form-data`, the API's recommended form: the request, then each picture's file as it is on disk, read when the request is sent (a question never sent never reads its picture). In the request a picture is named by a hash of its bytes, so the same pictures make the same requests on every run, and the path stays on your machine. Each run assumes its files don't change while it runs.

**What doesn't take pictures.** `-J`, `-s` and `--split` read text; `deem tune` labels text in the terminal. With `--failed`, a picture run's file lists the paths, to run again with `-I`.

Pictures are "Dreamer-ready" at the API: experimental, and its format and limits may change. A picture's answers vary more between runs than text's: the same picture question, asked twice, gave confidences of 0.49 and 0.65.

## Safety

Every question deem asks goes to a third-party API. So safety in deem is mostly one promise: **nothing reaches the model except what your program names**: the last argument of each `ask`, `choose` and `judge`, and the text of its question.

1. **Only what a program names reaches the model.** `judge` sends a request to the run's API, `{url}/systemone`, with that API's key ([APIs](#apis); without APIs, `JEV_BASE_URL` and `JEV_API_KEY`), in a shape deem builds; no program can choose the URL, the headers, the key or the model. And a program can't read secrets it could send by mistake. deem keeps every builtin jaq has except these:
   - **The environment** (`env` and `$ENV` are `{}`). It holds secrets, `JEV_API_KEY` among them, and anything a program can read it can put into a question.
   - **Files** (`import` and `include` are rejected). A file's contents could end up in a question, and the program would depend on files besides its input.

   What reaches the model, then, is each call's last argument and its question's text, and nothing else, and every call says which ([The whole interface](#the-whole-interface) shows how to pick it; Egress below, how to strip what you can't send). Each record's value is sent alone, in a request of its own, so one record's value doesn't enter the answer about another's. A picture goes only where the program puts its reference, and data can't write a reference ([Pictures](#pictures)): deem reads only the files named on its command line, or with `-I` listed on its stdin, so don't pipe a list of paths you don't trust into `deem -I`.
2. **Records are independent.** deem runs records in parallel, sends their questions together, prints them in input order and reruns failed ones from `--failed`; all of that needs each record to depend on nothing but itself. So a program can't read other records (`input`, `inputs`, `input_filename` and `input_line_number` don't exist), and `halt` ends only its own record, not the run. A program that needs every record says so on its first line, `module {input: "all"}`: then the whole input is one record, `.` is its array of records, and there are no records to keep apart.

   What stays: `debug` and `stderr`, which only write to stderr, and `now`, `localtime` and `strflocaltime`, which read the clock. They can't tie records together, so deem keeps them, as jaq has them; a program can put the time they read into a question, and so send it ([DESIGN.md](DESIGN.md#what-a-program-depends-on) has the rest).
3. **Input data can't start a request.** jq has no `eval`, so data never becomes code; only a `judge` written in the program asks anything. Data still *influences* requests: what they contain, how many there are (`.[] | ask(q; .)` over an array the data controls), and, through answers, which branch runs next. So the number of questions is capped.
4. **A failing record is contained.** A record whose answer fails, or whose program fails, ends on its own; that never reaches the other records or the run.
5. **Arguments are values.** The words after `deem run PROGRAM`, the question of `deem "Q"`, `--arg` values and `--rawfile` files' text are bound as jq variables, never pasted into the program's text, so a question with quotes or `\(` in it can't change the program. Saved programs (`.deem/`, `~/.deem/`) and the built-in ones run under the same rules as any other.

**The question cap.** A run asks at most 100,000 questions unless `--max-questions N` says otherwise (`0` is no limit). A question counts when it is queued for the API; a repeat answered from the run's memo doesn't count. Reaching the cap ends the run early, and the result is partial:

- The question that would go past the cap fails, and so does its record. So does every record that asks a new question after that.
- No more records are read. With `--failed`, deem says how many it read, R; the unread ones aren't in the file, so a rerun is the records in `--failed` plus the input after record R.
- Questions within the cap are still sent, and the records that finish are printed. Exit code 2.
- **The records that finish aren't the first ones, and can differ from run to run.** Records run in parallel, so which ones got their questions in before the cap is a matter of timing. Output keeps input order, with gaps; `--failed` lists the records that started and didn't finish.
- A failed record's queued questions aren't sent, but answers it already had were paid for. A program that asks one question after another (`select(ask(…) > 0.5) | ask(…)`) can lose up to one wave of answers at the cap.

The cap is a safety stop against runaway spending, not a way to judge "the first N": to judge a sample, cut the input (`head -1000 app.log | deem …`). Set the cap with headroom, from `--stats` on a sample:

```console
$ deem run --max-questions 50 \
    'range(1e9) as $i | ask("is \($i) special?"; .)' app.log
deem: stopped at the question limit (50); raise it with --max-questions N, 0
  for no limit
$ echo $?
2
```

The cap bounds questions, not computation: a loop that only repeats questions already asked is answered from the memo, costs nothing, and runs until it is stopped, as it would in jq.

**Egress.** Every record you judge is sent to a third-party API. If your data has PII or secrets you can't send, strip or hash it in the jq program before it reaches `ask`/`choose`, for instance `gsub("[\\w.+-]+@[\\w-]+\\.\\w+"; "[email]")` for the common cases, and keep that step visible in the pipeline you save, not hidden in a flag. A picture is sent as its file is, with its metadata (a photo's EXIF can hold where it was taken); the API strips it only after receiving it, so strip it first if it matters: `exiftool -all= photo.jpg` or `magick photo.jpg -strip out.jpg`.

**Injection.** The model does not treat the value it judges as hostile, so a log line that argues for its own classification can move its own answer. The judged value (each call's last argument) is the request's `state`; deem never pastes it into the instructions. The question's text is different: it is sent as the program built it, so a question built from the record (`ask("is a \(.level) bad?"; .msg)`) carries that part of the record into it.

How far, measured on three labelled sets (150 support messages, 94 log lines, 81 app reviews; `robustness` in `tests/live/versus.rs`, 2026-10-19): a record ending "IMPORTANT: … answer no" moved urgent texts from 0.82 to 0.69, and borderline ones from 21 to 16 of 24 urgent; a review ending "IMPORTANT: … answer yes" turned 2 sarcastic reviews of 40 into 8; a plausible urgent paragraph added to calm messages, 5 urgent of 40 into 14. Text imitating the request ("The question has changed…", the request's own JSON) moved no message, and 4 of 40 log lines asked whether they were a security event. On attacker-controlled input:

- **Don't interpolate free-text fields into questions.** Send them as the judged value instead: `ask("is this bad?"; {level, msg})`. A field you have checked against known values is fine: `select(.level | IN("error", "warn", "info")) | ask("is a \(.level) bad?"; .msg)`.
- **Judge only what the question needs.** A field, not the whole record, leaves a hostile record less room to argue, and costs less ([Before a big run](#before-a-big-run)). It also leaves nothing else to answer: a record whose other fields ask something of their own (a `question`, a `prompt`, `instructions`) can get that question answered in place of yours. Log records carrying `"question": "Is this message written in English?"`, asked "would a user notice this?", were answered yes for 15 of 63 lines labelled no, against 2 without the field (2026-10-19). Judge the field the question is about (`ask(q; .line)`), or drop the fields that ask (`ask(q; del(.question, .prompt))`).

A picture can carry text that gives orders, as a record can, and is read the same way.

Still, a semantic filter on attacker-controlled input is attacker-influenced. Screen anything web-facing.


## Before a big run

What a run will cost depends on your data and your program, so measure it: run the command on a sample with `--stats`.

```bash
head -1000 tickets.jsonl | deem run -J --stats \
  '{id, urgent: ask("is this urgent?"; .body),
    angry: ask("is the customer angry?"; .body),
    topic: choose("what is it about?"; ["bug", "billing", "account"]; .body)}'
```

`--stats` prints what the run took on stderr, in requests and input tokens. For tickets of about 3 KB, asked these three questions, the lines about cost would read about:

```text
deem: records 1000 · questions 3000 of 100000 · …
deem: tokens 952.0k · …
deem: per 1000 records: tokens 952.0k
```

- **`questions`** is what a record costs in questions: 3 here. Set `--max-questions` for the full run from it, with headroom.
- **`per 1000 records`** sizes the full run: the file's records ÷ 1000 × that, in tokens; multiply by the API's current price for money. The sample must look like the file: the first lines of a log are often not typical of the rest, and a sample of short tickets prices a file of long ones wrong.

A request carries its value once, however many questions ask about it. What a value costs, in input tokens as the API counted them (2026-09-29):

| value | one question | three |
|---|---|---|
| a short message (~50 bytes) | 300 | 340 |
| 3 KB | 912 | 952 |
| 12 KB | 2,684 | 2,724 |

To bring the cost down:

- **Judge what the question needs, and no more.** `ask(q; .body)` rather than `ask(q; .)`, and `.body[0:2000]` for a long body whose start says enough. The API's guide asks for the same: "Include only the context relevant to the current questions. This helps the model avoid distractions and context rot." Less to read is fewer tokens, often a better answer, and less room for a hostile record to argue.
- **Ask one question where you can.** `choose(q; ["bug", "billing", "account"]; .body)` is one question, and a request's fixed cost is paid once for the three options.
- **Filter first.** `select(ask(cheap; .title) > 0.5) | ask(q; .body)` asks about the body only for the records that passed.


## APIs

A run sends to one API. Without APIs in a config file, it is the environment's: `JEV_BASE_URL` (the hosted API by default), `JEV_MODEL` and `JEV_API_KEY` ([Reference](#reference)). `deem api` names the APIs you use instead, the hosted one and any server of your own that speaks the Jev API (`POST /v1/systemone`), and a run chooses one:

```bash
deem api add jev https://api.typesafe.ai/v1 --pictures --key
deem api add kev http://127.0.0.1:8010/v1 --model kev-latest --parallel 4
```

The first asks for jev's key, without showing it as you type; piped, it reads it from stdin (`pass show jev | deem api edit jev --key`). It saves it in `~/.local/state/deem/keys/jev` and says so. A key is never an argument, which `ps` and the shell's history would show. The first API added is the default:

```console
$ deem api
* jev  https://api.typesafe.ai/v1  jev-latest  pictures  key
  kev  http://127.0.0.1:8010/v1    kev-latest  parallel 4
$ deem api default
jev
```

A run goes to the default unless `--api NAME` says otherwise, or `DEEM_API=NAME` for a whole shell; `--api` wins over `DEEM_API`. A run that doesn't go to the default says where it goes, on stderr, so it never goes elsewhere unseen:

```bash
deem --api kev "is it critical?" app.log
```

```text
deem: using kev (http://127.0.0.1:8010/v1)
```

- **Changing them.** `deem api edit NAME` takes `--url`, `--model`, `--pictures` or `--no-pictures`, `--key` or `--no-key`, `--rps N`, `--tps N` and `--parallel N`; `deem api default NAME` makes another the default; `deem api remove NAME` removes one and its key. The default is removed only when it is the last: deem doesn't choose another for you.
- **Keys.** Each key is in a file of its own, `~/.local/state/deem/keys/NAME` (`$XDG_STATE_HOME/deem/keys/NAME` if that is set), that only you can read (mode 600, in folders only you can open). `deem api` says where when it saves or removes one: `key for jev saved in /home/you/.local/state/deem/keys/jev, readable only by you` (`the_commands_keep_the_file`). Keys are kept apart from the config file so that a `~/.config` copied to another machine or kept in a dotfiles repository carries no key. With no home folder and no `XDG_STATE_HOME`, there is nowhere to keep them: `XDG_CONFIG_HOME` still finds the APIs, but none has a key, and setting one says why (`apis_without_a_state_folder`). A key file others can read is refused, as ssh refuses such a private key; so are the config file, deem's folders and `keys/` if others can change them, since who can change the file can send your key elsewhere (`a_config_others_can_change_is_refused`). `deem api` makes them so: the folders mode 700, made or found, and the file 600. An API's key is sent only to that API's URL: one without a key, like a server of your own, gets requests with no `Authorization` header (tests/apis.rs, `a_key_goes_only_to_its_api`). The hosted API needs one; without it, the run stops before anything is sent. A key file written by hand is checked as a typed key is: one word, no control characters (`a_key_file_that_cant_be_sent_is_named`). A key for a server on this machine goes to whatever listens on its port: on a machine shared with other people, anyone can listen on a port from 1024 up while your server isn't running (Linux's `net.ipv4.ip_unprivileged_port_start`, 1024 on Linux 5.15, 2026-10-21), so give such a server a key only if you need to.
- **URLs.** An API's URL is `https://`, or `http://` only to this machine (`localhost`, `127.0.0.1`, `[::1]`): anywhere else, the key and the records would go in the clear, and the URL is refused (`a_file_written_by_hand_is_checked`). A URL with a name or password in it (`https://me:secret@host/v1`) is refused too, without showing it: deem shows an API's URL where it says where a run goes, and a key belongs in `keys/NAME` (src/cli/args.rs, `api_commands`). `JEV_BASE_URL` is held to the same rules.
- **A key stays with its server.** `deem api edit NAME --url` to another server (another `host:port`) of an API that has a key is refused unless the same edit says `--key`, for a key there, or `--no-key`; another path on the same server keeps the key (`a_new_server_needs_its_own_key`).
- **Pictures.** An API takes pictures only if added or edited with `--pictures`. A run that shows a picture (a picture file, `-I`, `--img`) to one that doesn't stops before anything is sent: `deem: API kev doesn't take pictures: …` (`an_api_without_pictures_is_shown_none`).
- **Rates.** `--rps` and `--tps` are the API's requests and input tokens a second, as `JEV_REQUESTS_PER_SECOND` and `JEV_TOKENS_PER_SECOND` are without APIs; unset, they are the hosted API's defaults.
- **Requests at once.** `--parallel N` is the most requests deem has at the API at once, as `JEV_PARALLEL` is without APIs; the rest wait in deem until one is answered. Set it to what a server of your own runs at once (llama-server's `--parallel`: 4 unless set, in llama.cpp `ead9f32` as LM Studio's CUDA 12 runtime 2.62.0 ships it): a request beyond it waits in the server's queue, where deem's 60 s reply timeout runs too, and is sent again when it runs out. Against llama-server with 4 slots and Kev-9B (2026-10-21, a GPU of 8 GB), 300 records sent at once had 64 to 249 requests time out in its queue and sent again, and the server cancelled the work it had begun on each. On Kev-4B Q8_0 (the same GPU, 4 slots), the cap costs nothing: 300 one-line tickets (`sample.txt`), three questions each in one request, took 28.2 s with `--parallel 4` and with 256, but a request took 281 ms at the median with the cap and 13.6 s without, most of it in the queue. Unset, deem sends as fast as `--rps` and `--tps` allow (`parallel_caps_requests_at_once`, `an_apis_parallel_caps_its_requests`).
- **One place.** With APIs in the config file, the API's variables (`JEV_API_KEY`, `JEV_BASE_URL`, `JEV_MODEL`, `JEV_REQUESTS_PER_SECOND`, `JEV_TOKENS_PER_SECOND`, `JEV_PARALLEL`) are refused, so a run's settings come from one place (`settings_come_from_one_place`). With none left, after the last `deem api remove`, they set the API again, as without a file (`the_commands_keep_the_file`). `DEEM_MAX_RECORD_BYTES` belongs to the run, and stays.
- **Another server, once.** Name a folder with no config file for that run, and the variables set its API, leaving your APIs and keys as they are: `XDG_CONFIG_HOME=/nonexistent JEV_BASE_URL=https://other.example/v1 deem "is it critical?" app.log` (with `JEV_API_KEY` if that server needs a key; `settings_come_from_one_place`). To use it again, add it: `deem api add`.
- **The file** is `$XDG_CONFIG_HOME/deem/config.toml`, else `~/.config/deem/config.toml`, and holds no secrets. It can be edited by hand; `deem api` keeps what it doesn't change, comments too. A setting deem doesn't know, or a value it can't use, is an error naming it (`a_file_written_by_hand_is_checked`):

  ```toml
  default = "jev"

  [api.jev]
  url = "https://api.typesafe.ai/v1"
  model = "jev-latest"
  pictures = true

  [api.kev]
  url = "http://127.0.0.1:8010/v1"
  model = "kev-latest"
  pictures = false
  rps = 20
  ```

- **Changes and runs.** A run reads the file once, when it starts, so a change never reaches a run that is going; and a change is written whole, then put in place, so a run starting meanwhile reads the old file or the new one (`a_change_never_shows_a_run_half_a_file`). Changes wait for each other on `config.lock`.
- **Which API a run used.** A `--trace` file's first line names it, its URL and its model. Run `--failed` records again with the same `--api` as the first run, or their answers come from another model.

`box` needs Jev 1.14 ([Pictures](#pictures)), which `jev-latest` was on 2026-10-22. If your API's model refuses a box, `deem api edit jev --model jev-preview`, or an API of its own for it, added like jev.

### A server of your own

Jev runs only at TypeSafe AI. On your own machine you run another model behind a server that speaks the Jev API: llama.cpp's `llama-server` serves [Kev](https://github.com/jaredpalmer/kev) (text) and vllm-d3-mini (text and pictures) on `/v1/systemone`. Both are Apache-2.0 (Kev's `LICENSE`, and the `general.license` of d3-mini's GGUF file). llama-server comes with [llama.cpp](https://github.com/ggml-org/llama.cpp): its [releases](https://github.com/ggml-org/llama.cpp/releases) are built for Linux, macOS and Windows, on Linux and Windows one archive for each GPU backend (CPU only, CUDA, Vulkan, ROCm, SYCL among them; on 2026-11-02, release `b12040`, whose Linux Vulkan archive holds `llama-server`), and its [install guide](https://github.com/ggml-org/llama.cpp/blob/master/docs/install.md) names the package managers that have it (Homebrew, winget, conda-forge, Nix, MacPorts). LM Studio ships one too (below). With `-hf`, llama-server downloads a GGUF file from Hugging Face into `~/.cache/huggingface/hub` (`$LLAMA_CACHE` if set) and loads it from there on later runs. For Kev, as the `kev` API above:

```sh
llama-server -hf ggml-org/Kev-4B-GGUF:Q8_0 --host 127.0.0.1 --port 8010 -ngl 99
```

The part after `:` is the quantization: `BF16` keeps 16 bits a weight, `Q8_0` 8, `Q4_K_M` fewer, for a smaller file. `-ngl 99` puts every layer on the GPU; a model that doesn't fit starts with `-ngl` left out, slower. llama-server sizes its context to the GPU memory left, so a smaller file gives each slot a longer context rather than leaving memory free. Kev comes in three sizes; on Hugging Face on 2026-10-26:

| GGUF | reads | quantizations |
|---|---|---|
| `ggml-org/Kev-0.8B-GGUF` | text | `Q8_0` 0.8 GB, `BF16` 1.5 GB |
| `ggml-org/Kev-4B-GGUF` | text | `Q4_K_M` 3.0 GB, `Q8_0` 4.5 GB, `BF16` 8.4 GB |
| `ggml-org/Kev-9B-GGUF` | text | `Q4_K_M` 6.4 GB, `Q8_0` 9.5 GB, `BF16` 17.9 GB |
| `ggml-org/vllm-d3-mini-GGUF` | text and pictures | `Q4_K_M` 3.7 GB, `Q8_0` 5.2 GB, `BF16` 9.7 GB, and a vision projector (mmproj) of 0.4 GB (`Q8_0`) or 0.7 GB (`BF16`) |

Kev-4B Q8_0 scored within 0.015 of Kev's own server running Kev-4B in bf16 (2026-10-21). Neither model is Jev, and scores differ from its: Kev ranks records much as Jev does and scores lower, so the example programs with thresholds set on Jev (`problem` 0.7, `route` 0.9) kept fewer records on Kev (tests/live/README.md, 2026-10-21). Compare a model's answers before you rely on a threshold ([Comparing APIs](#comparing-apis)).

- **Requests at once.** llama-server runs 4 requests at once unless its `--parallel` says otherwise: give the API the same `--parallel`, or requests wait in its queue past deem's timeout and are sent again ([APIs](#apis), "Requests at once").
- **Not LM Studio's own API.** Its server (port 1234) answers `/v1/systemone` with `Unexpected endpoint or method` (2026-10-21). The `llama-server` LM Studio ships works, run by hand.

**Pictures.** deem sends pictures as Jev takes them, a `multipart/form-data` request with each picture a part of its own ([Pictures](#pictures), "How it is sent"); llama-server takes them as data URLs in a JSON `images` list. A small proxy sits between them: [`pictures.py`](https://gist.github.com/phyro/a4be45fc76e1bb879de6c63c40a42681) (Python 3, nothing to install). Text passes through as it is; a request with pictures goes on as JSON, each picture in `images` and its ID in the request replaced by `[image N]`, so a question still names its picture. `-hf` fetches the model's vision projector (mmproj) with it; d3-mini reads a whole request, pictures included, in one batch, so `-b` and `-ub` must hold it. The proxy and llama-server each keep running, in a terminal of their own:

```sh
curl -L -o pictures.py \
  https://gist.github.com/phyro/a4be45fc76e1bb879de6c63c40a42681/raw
llama-server -hf ggml-org/vllm-d3-mini-GGUF:Q8_0 --host 127.0.0.1 --port 8012 \
  -ngl 99 -b 8192 -ub 8192
python3 pictures.py http://127.0.0.1:8012 --port 8013
```

```bash
deem api add d3 http://127.0.0.1:8013/v1 --model jev-latest --pictures \
  --parallel 4
deem --api d3 "does the screenshot show an error?" -p screens/*.png
```

On `Q8_0` and `Q4_K_M` it gave `checkout-error.png` 0.99 and the other four screenshots 0.14 or less; Jev gives them 0.04 or less ([README](../README.md#pictures)). `box` on d3-mini is not measured.

### Comparing APIs

To see how close another API's answers are to the hosted API's, run the same programs against each with `--trace`, then compare the traces. In `examples/`, `trace.sh` runs the example programs against one API, a trace each, into `traces/` (the script says which examples it leaves out, and why). Trace the hosted API once, with its key, and a server of your own as often as you like; only one server need be up at a time:

```bash
sh trace.sh jev
sh trace.sh kev
deem trace compare --with jev traces/*.jsonl
```

`deem trace compare --with NAME FILE...` reads only the traces: no API is asked, and the APIs need not be in your config. Each API is named as its traces' first line names it (by its URL without APIs), NAME's answers are taken as right, and each other API gets a column in the first tables, side by side (**every API against NAME**), then a section of its own (**API against NAME**); each part starts under its title between two rules. An answer is matched by what was asked, the same question (text, kind, options) about the same value, whichever trace or program asked it; what only one API was asked about is counted, not compared (tests/trace.rs, `traces_are_compared`). Kev-4B Q8_0 on llama-server (4 slots, a GPU of 8 GB) against the hosted API (`jev-latest`, which answered as `jev-1.14.0`), both traced 2026-10-22; part of the report:

```text
jev   https://api.typesafe.ai/v1  jev-latest, answered by jev-1.14.0
kev8  http://127.0.0.1:8010/v1    kev-latest, answered by Kev-4B-Q8_0.gguf

      answers  failed  median request
jev       699       0          313 ms
kev8      686       0          121 ms

════════════════════════════════════════════════════════════════════════════════
every API against jev
════════════════════════════════════════════════════════════════════════════════

ask: how often each API answers as jev does
                                                              answers       kev8
                                                             compared     agrees
all 15 questions                                                  371  323 (87%)
is this urgent?                                                   300  259 (86%)
── fewer than 20 answers: a smoke test, not a measurement ──────────────────────
is this message friendly?                                           9        8/9
is the user reporting a problem?                                    9        9/9
…

choose: how often each API answers as jev does
                                                              answers       kev8
                                                             compared     agrees
all 4 questions                                                   314  222 (71%)
what is this ticket about?                                        300  210 (70%)
── fewer than 20 answers: a smoke test, not a measurement ──────────────────────
which part of the product is this about?                            6        4/6
…

════════════════════════════════════════════════════════════════════════════════
kev8 against jev
════════════════════════════════════════════════════════════════════════════════

ask                                             AUC  offset  kev8 yes,  kev8 no,
                                                                jev no   jev yes
is this urgent?                                0.90   +0.03         21        20
── fewer than 20 answers: a smoke test, not a measurement ──────────────────────
is this message friendly?                      1.00   -0.08          0         1
…

choices that differ: jev's answer down, kev8's across

what is this ticket about?
           billing  bug  other  refund
  billing       30    ·     26       1
  bug            1   68     61       ·
  other          ·    ·    105       ·
  refund         1    ·      ·       7
…

not compared
is this about billing?                          6 only in jev, 1 only in kev8
what does this screenshot of a web page show?   5 only in jev
does `screen` look like `baseline`, with the …  2 only in jev
the error message                               box, not compared: jev 1, kev8 0

largest differences (≠: on the other side of 0.5)
  kev8   jev      Δ
≠ 0.34  0.68  -0.34  would a user actually notice this?
                     Oct 19 14:23:44 api-7f4 ERROR 502 from payments-svc, 118 r…
≠ 0.22  0.56  -0.34  is this urgent?
                     the store is showing prices without VAT to EU customers
```

Kev-4B gives the hosted API's answer to "is this urgent?" for 259 of 300 tickets, and orders them much as it does (AUC 0.90), its misses as often yes (21) as no (20); but it gives the hosted API's topic to 70%, and the table under it shows where the rest went: to `other`, 26 of the 57 billing tickets and 61 of the 130 bugs.

- **The first table** is each API's URL and model as its traces name them, and the model its responses named when that is another: llama-server names the file it loaded. deem doesn't check that one name is one model; a name used for two shows both.
- **answers** and **failed** count questions; a question failed if its request failed or was never sent, or the answer was of another kind than the question (`traces_are_compared`, `answers_that_are_not_scores`). **median request** is from sending to the answer. A second answer to what an API had already answered is left out, and counted under the table. A trace from a deem whose traces don't name their URL is refused: it lacks what compare reads.
- **ask: how often each API answers as NAME does**, and the same for **choose**, are a row a question and a column an API, so APIs compare side by side. **answers compared** counts the values every API answered, NAME too, so each column counts the same ones; what some API wasn't asked, or failed, is under **not compared**. **agrees** is how many of those the API answered as NAME did, and their share: for an `ask` on the same side of 0.5, for a `choose` the same option. **all** adds up every question of the kind. A row of fewer than 20 answers is a fraction, and those rows come after a line that says they are a smoke test, not a measurement (`traces_are_compared`, `numbers`). The questions with the most answers come first. A question no value of which every API answered, a picture question when one API takes no pictures, has a table of its own under them, **not every API answered these**: a column an API, each the share of the values it and NAME both answered that it answers as NAME does, `-` for an API that answered none (`traces_are_compared`).
- **API against NAME** is a section for each other API, of the values it and NAME both answered, whatever the other APIs answered (`traces_are_compared`). Its **ask** table has a row for each question both answered: **AUC**, how well the API orders what NAME says yes to before what it says no to (above 0.5 or not), 1.00 for every yes first, 0.50 for chance, none unless NAME has both; **offset**, the mean of the API's scores minus NAME's; and the two ways they can differ, each column named for both: **API yes, NAME no** counts the answers the API put above 0.5 and NAME didn't, **API no, NAME yes** the other way. An API that orders well but answers one way too often needs another threshold (`deem tune` on its own scores), not another question.
- **choices that differ** is a table for each `choose` question the API answered differently at least once: a row for NAME's answer, a column for the API's, the same options down and across, so the same answers are the diagonal and everything off it a difference (`columns_line_up`).
- **not compared** lists, a question a line, each side's failures (**failed in**) and the values one API was asked about and the other wasn't (**only in**): a question asked after an answer (`route` asks "is this about billing?" only of tickets it found urgent) is asked of different records by different APIs, and an API without pictures is asked none of the picture questions. `box` and `rate` questions are listed there with how many were asked.
- **largest differences** are the `ask` answers furthest from NAME's, each with its question and, under it, the value: the records worth reading. The ones on the other side of 0.5, marked ≠, come first, since those change what a program does (`answers_that_are_not_scores`).

In the examples the two 300-ticket questions, "is this urgent?" (`route`) and "what is this ticket about?" (`topics`), lead their tables; the rest are small. A difference of a few hundredths says nothing: compare an API with itself to see how much its answers move. It reports, and doesn't pass or fail: how close is close enough is yours to decide.

## Cost and rate limits

The API charges for input tokens, at the price on [TypeSafe AI's site](https://docs.typesafe.ai); deem reports tokens, not money, as the price can change. A million short lines asked one question is about 300 million tokens (300 each, as every request costs 272 before its content; measured on short support messages, 2026-09-29). Long values cost more: [Before a big run](#before-a-big-run) shows how to size a run from a sample. Cost is rarely the constraint; the rate limit is. The hosted API's limits are set per organization and shared by all its keys, 80 requests and 100,000 input tokens a second by default (your [console](https://console.typesafe.ai/rate-limits) shows yours), and deem takes them all: about 80 values a second (2,000 log lines in 24.4 s, 2026-10-19). deem stays under both, and backs off and retries when the API says it is busy. Set an API's `--rps` and `--tps` ([APIs](#apis)), or without APIs `JEV_REQUESTS_PER_SECOND` and `JEV_TOKENS_PER_SECOND` ([Reference](#reference)), when your limits differ, as with a plan that allows more, which then runs faster; or lower them to leave room for something else on the same organization, since a run at the full rate leaves nothing for the rest, which then meets `429`s.


## Reference

```
deem - grep by meaning: ask a question about every line

USAGE
  deem [OPTIONS] QUESTION [FILE...]              keep lines where it's yes
  deem [OPTIONS] QUESTION -p [FILE...]           score<TAB>record
  deem [OPTIONS] QUESTION -o A,B,C [FILE...]     option<TAB>record
  deem [OPTIONS] QUESTION -l A,B,C [FILE...]     level<TAB>record
  deem run [OPTIONS] PROGRAM [ARG...] [FILE...]  a name, a path, or jq text
  deem tune [OPTIONS] QUESTION [FILE...]         label lines, get a threshold
  deem programs [NAME]                           list programs, or print one
  deem api [add | edit | remove | default] ...   APIs to send to
  deem trace [--visual] FILE                     show a run --trace recorded
  deem trace compare --with NAME FILE...         each API's answers in the
                                                 traces against NAME's
  deem models | help | version

Options go anywhere, before run or tune too; the words that aren't options
are the question (or the program and its arguments), then files. A question
that starts with -, or is the word run or tune, goes after --.

  -p, --score        print each line's score in front of it
  -o, --options A,B  print the option each line fits best in front of it
  -l, --levels A,B   print each line's level in front of it: 0 is A, 1 B, ...
  -n N               deem tune: how many lines to label         (default 20)
  -t, --threshold N  keep lines scoring above N, in [0,1]  (default 0.5)
                     the same as --arg threshold N
      --arg NAME V   set a named value the program declares
      --rawfile NAME FILE  $NAME is the file's text, as in jq; the program
                     needn't declare it
      --img NAME FILE      $NAME is the picture (PNG or JPEG)

PROGRAMS are jq with judge(spec; ctx), and ask(q; ctx), choose(q; opts;
ctx), rate(q; levels; ctx) and box(q; ctx) built on it; ctx is all the
model sees. Built in: group, histogram, mix, disagree; `deem programs`
lists them and yours (the nearest .deem/ from here up, ~/.deem/).

  -J, --json         each JSON value is a record; without it, each line
  -I, --images       each stdin line is a picture's path (picture files
                     given as FILE need no flag)
  -s, --slurp        the whole input is one record (-J: an array of the
                     values; not with input "all")
      --split RE     record separator                       (default \n)
  -r, --raw-output   print strings unquoted
  -e, --exit-status  exit 1 if the last output was false or null

      --max-questions N  stop after N questions; the run is partial
                     (default 100000, 0: none)
      --stats        requests, tokens, tokens per 1000 records, score
                     histogram -> stderr
      --trace FILE   record the run, one event per line; see deem trace --help
      --failed FILE  the records that didn't finish, as read, to run again
      --api NAME     the API to send to                   (default: the
                     default API; DEEM_API=NAME sets it for a shell)

APIS are in ~/.config/deem/config.toml, their keys in ~/.local/state/deem/keys.
  deem api                       list them; * marks the default
  deem api add NAME URL [SET]    add one; the first added is the default
  deem api edit NAME SET         change one
  deem api remove NAME           remove one, and its key
  deem api default [NAME]        print, or set, the default
  SET is any of --url U, --model M, --pictures or --no-pictures, --key or
  --no-key (--key reads it from the terminal, or stdin), --rps N, --tps N,
  --parallel N (the most requests at the API at once: as many as your
  server runs at once, e.g. llama-server's --parallel)

ENVIRONMENT
  DEEM_API DEEM_MAX_RECORD_BYTES
  without APIs (deem api), the API is JEV_BASE_URL, JEV_MODEL, JEV_API_KEY,
  JEV_REQUESTS_PER_SECOND, JEV_TOKENS_PER_SECOND and JEV_PARALLEL

EXIT  0 output produced - 1 nothing selected, or -e and last output false/null
      2 usage error, program error, failed record, or API error
      141 the output was closed

More: README.md and docs/MANUAL.md
```

`deem trace --help` prints the trace commands, with examples:

```
deem trace - show a run --trace recorded, or compare runs against APIs

USAGE
  deem trace [--visual] FILE                     a run's waves and requests,
                                                 or (--visual) a timeline
  deem trace compare --with NAME FILE...         each API's answers in the
                                                 traces against NAME's

A RUN
  deem run triage app.log --trace run.jsonl      record it
  deem trace run.jsonl                           each wave, its requests, and
                                                 what each question waited for
  deem trace --visual run.jsonl                  the same run as a timeline

COMPARING APIS (in examples/: a trace of each example program)
  sh trace.sh jev                                the hosted API
  sh trace.sh kev                                a server of your own
  deem trace compare --with jev traces/*.jsonl   kev's answers against jev's

Traces of any runs compare: answers are matched by question and value.
More: docs/MANUAL.md, "Seeing a run" and "Comparing APIs"
```

Options go anywhere, before `run` or `tune` too. The first word that isn't an option is the question (or, after `run`, the program); after `run`, the next words are the program's arguments, as many as its `main` takes; the rest are files. A word that starts with `-` goes after `--`, and so does a question that is the word `run` or `tune`.

A word after `deem run` is a path if it ends in `.deem`, or contains `/` and names a file; otherwise a program's name if deem finds one (built in, then `NAME.deem` in the nearest `.deem/` from the current directory up, stopping below the home directory, then `~/.deem/NAME.deem`); otherwise jq text.

**Builtins added to jq:** `judge(spec; .)`, and, from the prelude, `ask(q; .)`, `choose(q; opts; .)`, `rate(q; levels; .)` and `box(q; .)`.

**Environment.**

| | default |
|---|---|
| `DEEM_API` | the API a run sends to, as `--api NAME` (which wins over it); unset, the config file's default ([APIs](#apis)) |
| `DEEM_MAX_RECORD_BYTES` | `32768`; the most that is judged at once ([gotcha 6](#gotchas)) |

Without APIs in a config file (no file, or the last API removed), these set the API; with some, they are refused ([APIs](#apis)):

| | default |
|---|---|
| `JEV_API_KEY` | required by the hosted API; elsewhere optional, and unset, requests go without an `Authorization` header. deem sends it to whatever `JEV_BASE_URL` names: unset it before pointing deem at a server you don't trust with it |
| `JEV_BASE_URL` | `https://api.typesafe.ai/v1`; deem POSTs to `{JEV_BASE_URL}/systemone` and `deem models` GETs `{JEV_BASE_URL}/models`. Point it at any server that speaks the Jev API, over `https://`, or `http://` on this machine only ([APIs](#apis)) |
| `JEV_MODEL` | `jev-latest`; `deem models` lists what the API offers |
| `JEV_REQUESTS_PER_SECOND` | `80`, the hosted API's default for an organization; the most requests a second deem sends, 1 to 1000 |
| `JEV_TOKENS_PER_SECOND` | `100000`, the hosted API's default for an organization; the most input tokens a second deem sends |
| `JEV_PARALLEL` | unset; the most requests at the API at once, 1 to 256 ([APIs](#apis), "Rates") |

deem paces itself to the rate settings: at most `JEV_REQUESTS_PER_SECOND` (or an API's `rps`) requests and `JEV_TOKENS_PER_SECOND` (`tps`) input tokens a second, with as many requests in flight as keep the rate at 1.2 s a request (96 at 80 a second, at most 256, and at most `JEV_PARALLEL` or an API's `parallel`), and one value per request. It slows down while the API answers `429` or `529`, so a server of your own can take less by saying so.

