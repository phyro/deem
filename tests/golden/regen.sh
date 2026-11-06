#!/bin/sh
# Regenerate tests/golden from jq 1.7.1: fetch jq's test suite (a corpus
# of programs for the jaq parity test), then record jq's output on the
# everyday corpus, which the manual's differences from jq come from.
# The only script, with tests/live/, that touches the network.
set -e
cd "$(dirname "$0")/../.."
release=https://github.com/jqlang/jq/releases/download/jq-1.7.1
if [ "$(target/jq --version 2>/dev/null)" != jq-1.7.1 ]; then
	mkdir -p target
	curl -fsSL -o target/jq "$release/jq-linux-amd64"
	chmod +x target/jq
fi
curl -fsSL -o tests/golden/jq.test https://raw.githubusercontent.com/jqlang/jq/jq-1.7.1/tests/jq.test
DEEM_REGEN=1 cargo test --test jq_diff
