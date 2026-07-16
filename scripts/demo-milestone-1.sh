#!/bin/sh
set -eu

database="${TMPDIR:-/tmp}/autophagy-milestone-1-$$.db"
failure_mutation="mut_d6b7a340eb2fb6f18bee4a20932b9c954adb4975f3ea8136bf0bd264b3ec431c"
trap 'rm -f "$database" "$database-shm" "$database-wal"' EXIT HUP INT TERM

echo "Importing anonymized AEP evidence into $database"
cargo run --quiet -p autophagy-cli -- \
  --database "$database" \
  import evals/fixtures/findings/deterministic.jsonl \
  --instance-key milestone-1-demo

echo
echo "Deterministic patterns"
cargo run --quiet -p autophagy-cli -- --database "$database" patterns

echo
echo "Offline digest"
cargo run --quiet -p autophagy-cli -- --database "$database" digest

echo
echo "Register review-only mutation candidates"
cargo run --quiet -p autophagy-cli -- --database "$database" mutations propose

echo
echo "Persistent mutation registry"
cargo run --quiet -p autophagy-cli -- --database "$database" mutations list

echo
echo "Challenge the command-failure candidate"
cargo run --quiet -p autophagy-cli -- --database "$database" \
  mutations challenge "$failure_mutation" \
  --check coincidence-considered \
  --check sessions-comparable \
  --check trigger-observable \
  --check legitimate-uses-bounded \
  --check equivalent-searched \
  --check counterexamples-reviewed

echo
echo "Deterministic non-executable replay"
cargo run --quiet -p autophagy-cli -- --database "$database" \
  mutations replay "$failure_mutation" \
  --scenarios evals/fixtures/replay/command-preflight-pass.json

echo
echo "Retention preview"
cargo run --quiet -p autophagy-cli -- \
  --database "$database" prune --older-than-days 0 --dry-run
