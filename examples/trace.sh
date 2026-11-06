#!/bin/sh
# Run the example programs against one API, a trace each, for
# `deem trace compare`:
#
#   sh trace.sh jev
#   sh trace.sh kev
#   deem trace compare --with jev traces/*.jsonl
#
# Each program runs on the input this folder's README runs it on. Left out:
#
# - locate: what it finds is a box, and compare doesn't compare boxes; its
#   "is there ...?" only decides whether the box is printed.
# - sample.txt, week38.txt, week39.txt, labeled.tsv, auth.log, big.jsonl:
#   no example program reads them; they are the inputs of the manual's
#   commands (histogram, tune, the calibration check, the 32 KiB limit)
#   and of a test. sample.txt is also the same 300 tickets as
#   tickets.txt, which route and topics ask about.
set -u
api=${1:?usage: sh trace.sh API}
mkdir -p traces
# A program that selects nothing exits 1, and one with failed answers 2;
# the trace is written either way, so carry on. An API without pictures
# refuses the picture programs before sending anything, and writes no
# trace: compare shows their questions as only in the other API.
run() {
  name=$1
  shift
  deem --api "$api" --trace "traces/$api-$name.jsonl" "$@" > /dev/null || true
}

run hello-world run hello-world feedback.txt
run problem run problem feedback.txt
run triage run triage app.log
run route run route billing tickets.txt --arg urgent 0.9
run topics run topics tickets.txt
run failed-requests run -s failed-requests requests.log
run labels run -J labels tickets.jsonl
run mislabeled run -J mislabeled issues.jsonl
run moderation run -J moderation threads.jsonl
run ticket-kinds run -J ticket-kinds tickets.jsonl
run decision-tree run decision-tree alerts.log
run screen-check run screen-check screens/*.png
run same-page run --img baseline screens/orders.png same-page screens/orders*.png
