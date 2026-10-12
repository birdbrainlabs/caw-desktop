#!/usr/bin/env bash
# End-to-end test: start the receiver (dry run, nothing is typed), pair the
# fake phone through `caw-desktop pair`, send text and keys, check a resend
# is skipped. Runs on Linux and Windows (Git Bash). Usage: scripts/e2e.sh [target/release]
set -euo pipefail
bin=${1:-target/release}
work=$(mktemp -d)
export CAW_DRY_RUN=1 CAW_PORT=47821
# Keep keys and pairings out of the real config (Linux; Windows CI starts clean).
export XDG_CONFIG_HOME=$work/config XDG_RUNTIME_DIR=$work/run
mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"

"$bin/caw-desktop" serve >"$work/daemon.log" 2>&1 &
daemon=$!
trap 'kill $daemon 2>/dev/null || true; cat "$work/daemon.log"' EXIT
sleep 2

"$bin/caw-desktop" pair >"$work/pair.log" 2>&1 &
for _ in $(seq 20); do
  code=$(grep -o '[0-9A-Z]\{4\}-[0-9A-Z]\{4\}-[0-9A-Z]\{4\}-[0-9A-Z]\{4\}' "$work/pair.log" | tr -d - || true)
  [ -n "$code" ] && break
  sleep 0.5
done
[ -n "$code" ] || { echo "no pairing code"; cat "$work/pair.log"; exit 1; }

phone="$bin/caw-fake-phone"
"$phone" pair "caw://pair?h=127.0.0.1&p=47821&c=$code&n=test"
"$phone" send 'Hello from the test. '
"$phone" key enter 2
CAW_RESEND=1 "$phone" send 'Only once. '
"$phone" settings
sleep 1

log=$work/daemon.log
grep -q 'paired with "Fake phone"' "$log"
grep -q 'type "Hello from the test. "' "$log"
grep -q 'key Enter x2' "$log"
[ "$(grep -c 'type "Only once. "' "$log")" = 1 ]
grep -q 'skipped resent message' "$log"
echo "end-to-end test passed"
