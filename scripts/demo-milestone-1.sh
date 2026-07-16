#!/bin/sh
set -eu

database="${TMPDIR:-/tmp}/autophagy-milestone-1-$$.db"
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
echo "Retention preview"
cargo run --quiet -p autophagy-cli -- \
  --database "$database" prune --older-than-days 0 --dry-run
