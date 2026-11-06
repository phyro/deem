# deem's everyday tasks. `just` lists them.

# Tests run under a real memory cap and a timeout: a runaway program must not
# eat the machine. `ulimit -v` can't do it: it caps address space, which the
# thread-heavy tests need about 16 GB of, not RAM. Elsewhere than Linux they
# run uncapped.
cap := if os() == "linux" { "systemd-run --user --scope -q -p MemoryMax=6G -p MemorySwapMax=0 timeout 900" } else { "" }

# Tests also run without a network: in a namespace of their own that has
# this machine's loopback and nothing else, so no test can reach the hosted
# API, whatever it does. (The tests keep themselves off it too: tests/common,
# `OFFLINE`.) The live tests, which need the API, are run with `cargo test`
# directly: tests/live/README.md. Where a user may not make a namespace
# (some containers), `unshare` fails and no test runs: use `cargo test` there.
nonet := if os() == "linux" { "unshare -rn sh -c 'ip link set lo up && exec \"$@\"' sh" } else { "" }

default:
    @just --list

# Debug build
build:
    cargo build

# Release build: target/release/deem
release:
    cargo build --release

# Needs `rustup target add x86_64-pc-windows-gnu` and MinGW.
# Release build for Windows, and its lints: target/x86_64-pc-windows-gnu/release/deem.exe
build-win:
    cargo build --release --target x86_64-pc-windows-gnu
    cargo clippy --all-targets --target x86_64-pc-windows-gnu -- -D warnings

# Install (or reinstall) deem into ~/.cargo/bin
install:
    cargo install --locked --path . --force

# Remove the installed deem
uninstall:
    cargo uninstall deem

# All tests, or cargo test arguments (e.g. `just test --test waves`)
test *ARGS:
    cargo fetch --locked --quiet
    {{cap}} {{nonet}} cargo test --offline {{ARGS}}

# Lints, formatting, and the library's docs
check:
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --quiet

# Needs `cargo install cargo-audit --locked`; fetches the advisory database.
# Cargo.lock against RustSec's advisories, as CI's audit job checks it
audit:
    cargo audit

# Format the code
fmt:
    cargo fmt

# Tests, lints, formatting and docs: what every change must pass
ci: test check

# Re-record jq 1.7.1's outputs in tests/golden (downloads jq)
regen:
    {{cap}} tests/golden/regen.sh
