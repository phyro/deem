# Open issues

What is known and not done, highest priority first in each part.

## Problems

Where deem does something wrong, or less than it should.

- **Ctrl-C keeps nothing for `--failed`.** deem doesn't catch SIGINT, so
  an interrupted run ends at once: what it printed is on stdout, and the
  `--failed` file is empty, not the records that didn't finish (SIGINT
  after 3 s against a server that never answers, 2026-11-06). Keeping
  them needs a signal handler (a crate such as `ctrlc`, or code for Unix
  and Windows each), and a test that sends the signal.
- **A record that fails partway** still prints what it printed before the
  failure, so rerunning `--failed` can repeat those lines.
- **In a run where some requests show no picture** (a program that also
  judges a text field), a failure that every picture request shares, such
  as the API changing how it takes pictures, is reset by the text answers:
  picture records fail one by one, each reported, and the run doesn't
  abort (TRANSPORT.md, rule 6).
- **A screenshot against itself is only a likely yes on Jev 1.14.**
  `same-page`'s question, "does `screen` look like `baseline`, with the
  same layout and styling?", scored `orders.png` against itself 0.57 to
  0.86 over 14 runs on 2026-11-06 (tests/live/README.md), while the
  unstyled page stays at 0.01. The default threshold of 0.5 keeps the
  same page, with little room; a wording that scores it higher would also
  change the manual's `trace compare` report, which needs Kev to measure
  again.
- **Permissions are checked and set on Unix only.** On Windows a key file
  is used whoever can read it, and the config file and folders whoever can
  change them; `deem api` writes them with the folder's inherited access,
  which in the user's profile is usually theirs alone (not checked). A
  check would read the files' ACLs, which std doesn't expose.

## Open decisions

Measured, or sketched, and not yet decided.

- **The default is moved more by "answer yes" appended to a review**: 8–10
  of 40 non-sarcastic reviews turned sarcastic, against 3–4 with the
  layout before the bare state (tests/live/versus.rs, 2026-10-16, three
  runs; 8 again on 2026-10-19, `robustness`). Orders on messages and
  logs, and persuasion, moved it no more or less. Decide whether to wrap
  the value again for that, at the cost of the other gains.
- **Records that name their photo** (a batch of claims, each with its
  own picture) can't be written: data can't choose files. The jq-shaped
  way is photos as records, metadata from `--slurpfile`, joined on
  `input_filename`; adding it changes MANUAL "Safety" 2 and DESIGN.md's
  "What a program depends on". `--img NAME DIR`, binding a folder as
  `{basename: picture}`, would help too.

## Unverified

- **The live tests' labels** (`tests/live/labels.tsv`, `logs.tsv`,
  `reviews.tsv`) were written by Claude. A person decided the ones Claude
  flagged as debatable (2026-09-30), and on 2026-10-19 Claude reread all
  of them and flagged seven, and all seven were changed; no person has
  read every label. A wrong label misjudges every shape alike, but the
  accuracy numbers are only as good as the labels.
- **`--stats` after an abort.** `Batcher::close` waits for requests being
  handed to the senders, not for attempts already on the wire, which still
  get their reply (TRANSPORT.md, rule 5). If deem reports before such an
  attempt ends, `--stats` may count fewer attempts than the API received.
  Not seen in a test; `attempts_are_bounded` would show it as fewer
  requests counted than the stub saw.
- **Option descriptions in `choose`** (`{name: description}`): that they
  help, or do no harm, against names alone. Two signs they help, both on
  pictures (tests/live/README.md, "Measured by hand"); not measured on text.
- **Pictures, live**: the example screenshots have live tests
  (tests/live/pictures.rs: answers, `--img`, `choose` with descriptions,
  `box`, and the cost against deem's estimate); a photo, three
  photographed cards and the orientation tag were checked by hand
  (tests/live/README.md, "Measured by hand"). Still to check: whether the
  cost is per pixel or per tile. deem estimates width × height / 390, and
  a 640×427 photo cost 8% more than that (2026-10-16), a 960×600
  screenshot 25% more (1,854 input tokens against 1,477, the question
  included; 2026-10-20), so larger pictures look undercounted; a few more
  sizes would show the rule. Also unchecked: that a picture in an option
  is charged per question that offers it; `{"image": ref}` against the
  bare reference as the state; that no answer field but Score's `legend`
  echoes a reference.
- **Windows**: deem builds for `x86_64-pc-windows-gnu` (`just build-win`,
  with MinGW on Linux, builds it and runs clippy on that target; clean on
  2026-10-21), but it has never run on Windows, and CI doesn't check that
  it builds: the tests and CI are Linux only.
  The Windows-only code is `open_tty` in `src/cli/mod.rs` (`deem tune`'s
  console, `CONIN$`/`CONOUT$`); saved programs fall back to `USERPROFILE`
  when `HOME` isn't set (`run_programs`). Unchecked: `deem tune` on a
  Windows console, paths in `--img`, `--rawfile` and `deem programs`, and
  the docs' examples, which use a Unix shell. Also unchecked: that a
  wildcard in a file name (`screens/*.png`, `examples/README.md`) is
  expanded, by the `wild` crate, in cmd.exe and PowerShell, and one in
  double quotes isn't.
- **Threads under a container limit** (up to 8 workers, plus a sender
  each for up to 256 requests in flight): what a user sees when thread
  creation fails, and that it says what to do.
