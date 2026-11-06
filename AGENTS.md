# deem

[`README.md`](README.md) is the user's view, [`docs/MANUAL.md`](docs/MANUAL.md)
the reference, [`docs/DESIGN.md`](docs/DESIGN.md) how it works and what
a program depends on, [`docs/TRANSPORT.md`](docs/TRANSPORT.md) the rules the transport keeps,
[`ISSUES.md`](ISSUES.md) what is known and not done, and
[`CHANGELOG.md`](CHANGELOG.md) what changed in each release.
Finished plans and reviews are not kept in the tree.

- The reference for the language and output is **jaq** (the pinned
  jaq-core/std/json), plus deem's additions; not jq. Differences from jq
  1.7.1 are documented, not fixed (decided 2026-09-24; see docs/MANUAL.md,
  "Differences from jq").
- Keep the code minimal and modular: dependencies point one way
  (`programs` → `engine`; `run` → `engine`, `transport`, `config`, `trace`;
  `config` → `transport` → `trace` → `clock`; `engine` uses only
  `transport::api`'s types; `tests/layers.rs` checks it), the binary
  (`src/cli/`, including `deem tune`) uses only the library's public API,
  and nothing outside `src/engine/` sees a deferred value.
- Batching tests assert wave and request counts, not just outputs.
- A change to `src/transport/` keeps TRANSPORT.md's rules; each rule names
  the test that checks it. Change a rule and its test together.
- Every command in the README, the manual and the deem-programs skill
  runs in `tests/readme.rs` (the manual's "When an answer fails" in
  `tests/errors.rs`); change a doc example and its test together. The outputs the docs show
  come from live runs on `examples/` (scores vary a little; an interactive
  one like `deem tune` is built from real scores), never made up.
- A change to the language, the CLI or the examples updates
  [`.claude/skills/deem-programs/SKILL.md`](.claude/skills/deem-programs/SKILL.md),
  its `description` too: that line decides when the skill is used.
- No test but `tests/live` may reach the hosted API. Start deem in a test
  with `common::bin`, or with `OFFLINE` in its environment
  (`tests/layers.rs` checks it); `just test` and CI also run without a
  route to it. A live check made by hand is written down in
  `tests/live/README.md`, "Measured by hand".
- Run tests under a memory cap (`just test`, or `systemd-run --user --scope
  -p MemoryMax=… timeout …`). After a change, run `cargo test`,
  `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`.
- A claim in the docs about what deem does names the test that checks it,
  or the date it was measured and how; a claim about another tool (jq,
  jaq, the API) says which version or what was run. A claim that is
  neither is checked before it is written, or left out.
- A new open problem goes in ISSUES.md; a fixed one comes out of it.
- A change a user would notice gets a line under "Unreleased" in
  CHANGELOG.md; nothing else in the tree describes past behaviour.
