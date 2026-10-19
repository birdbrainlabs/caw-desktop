#!/usr/bin/env bash
# Tests `caw-desktop serve --headless`, the mode Caw for Mac runs as its
# helper: JSON-line commands on stdin, JSON-line events on stdout. Dry run
# (nothing is typed), its own port, config and temporary folder.
# Runs on macOS and Linux. Usage: scripts/headless-test.sh [target/release]
set -euo pipefail
bin=$(cd "${1:-target/release}" && pwd)
work=$(mktemp -d)
export CAW_DRY_RUN=1 CAW_PORT=47831 CAW_NO_MDNS=1
export XDG_CONFIG_HOME=$work/config XDG_RUNTIME_DIR=$work/run
mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
if [ "$(uname)" = Darwin ]; then
  export HOME=$work/home TMPDIR=$work/run
  mkdir -p "$HOME"
fi
phone="$bin/caw-fake-phone"
pids=()
cleanup() {
  for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
  echo "── events"; cat "$work/events.log" 2>/dev/null || true
  echo "── log"; cat "$work/daemon.log" 2>/dev/null || true
}
trap cleanup EXIT

# Waits up to 10 s for a line matching $2 in file $1.
wait_for() {
  for _ in $(seq 100); do
    grep -q -- "$2" "$1" 2>/dev/null && return 0
    sleep 0.1
  done
  echo "timed out waiting for $2 in $1"; exit 1
}
count() { grep -c -- "$2" "$1" 2>/dev/null || true; }
send_cmd() { echo "$1" >&3; }

mkfifo "$work/in"
"$bin/caw-desktop" serve --headless <"$work/in" >"$work/events.log" 2>"$work/daemon.log" &
helper=$!
pids+=("$helper")
exec 3>"$work/in"

wait_for "$work/events.log" '"event":"listening","port":47831'
wait_for "$work/events.log" '"event":"devices","devices":\[\]'

# Pair the fake phone with the code from a pair command.
send_cmd '{"cmd":"pair"}'
wait_for "$work/events.log" '"event":"pairing"'
uri=$(grep '"event":"pairing"' "$work/events.log" | tail -1 | sed -nE 's/.*"uri":"([^"]*)".*/\1/p' | sed -E 's/h=[^&]*/h=127.0.0.1/')
code=$(grep '"event":"pairing"' "$work/events.log" | tail -1 | sed -nE 's/.*"code":"([^"]*)".*/\1/p')
grep '"event":"pairing"' "$work/events.log" | grep -q '"expires_in":120'
"$phone" pair "$uri"
wait_for "$work/events.log" "\"event\":\"pair_ended\",\"code\":\"$code\",\"result\":\"paired\""
wait_for "$work/events.log" '"event":"devices","devices":\[{"id":"[0-9a-f]*","name":"Fake phone"'
wait_for "$work/events.log" '"event":"connected","id":"[0-9a-f]*","name":"Fake phone"'
id=$(grep '"event":"connected"' "$work/events.log" | head -1 | sed -nE 's/.*"id":"([^"]*)".*/\1/p')
wait_for "$work/events.log" "\"event\":\"disconnected\",\"id\":\"$id\""

"$phone" send 'Hello from the headless test. '
wait_for "$work/daemon.log" 'type "Hello from the headless test. "'

# Only events on stdout.
if grep -v '^{"event":' "$work/events.log"; then echo "stdout carries more than events"; exit 1; fi

# A new code replaces the old one; cancelling closes it.
send_cmd '{"cmd":"pair"}'
send_cmd '{"cmd":"pair"}'
wait_for "$work/events.log" '"result":"replaced"'
send_cmd '{"cmd":"cancel_pair"}'
wait_for "$work/events.log" '"result":"cancelled"'

# Unknown commands are refused, not obeyed.
send_cmd '{"cmd":"format_disk"}'
wait_for "$work/events.log" '"event":"error","message":"unknown command'

# Removing a connected phone drops its connection at once.
"$phone" stay 20 >"$work/stay.log" 2>&1 &
stay=$!
pids+=("$stay")
for _ in $(seq 100); do
  [ "$(count "$work/events.log" '"event":"connected"')" -ge 3 ] && break
  sleep 0.1
done
[ "$(count "$work/events.log" '"event":"connected"')" -ge 3 ] || { echo "the staying phone never connected"; exit 1; }
send_cmd "{\"cmd\":\"remove\",\"id\":\"$id\"}"
if wait "$stay"; then echo "the removed phone stayed connected"; exit 1; fi
grep -q not_paired "$work/stay.log"
wait_for "$work/events.log" '"event":"devices","devices":\[\]}$'
send_cmd '{"cmd":"remove","id":"nope"}'
wait_for "$work/events.log" '"message":"no paired phone with id nope"'

# A second receiver on the same port says the port is taken and exits 3.
set +e
"$bin/caw-desktop" serve --headless </dev/null >"$work/second.log" 2>/dev/null
status=$?
set -e
[ "$status" = 3 ] || { echo "second receiver exited $status, not 3"; cat "$work/second.log"; exit 1; }
grep -q '"event":"stopped","reason":"port_in_use"' "$work/second.log"

# Closing stdin stops the receiver, cleanly.
exec 3>&-
for _ in $(seq 50); do kill -0 "$helper" 2>/dev/null || break; sleep 0.1; done
if kill -0 "$helper" 2>/dev/null; then echo "the receiver kept running after stdin closed"; exit 1; fi
wait "$helper"

# With a standalone receiver running (it answers on the control socket), the
# headless one says so and exits 3 instead of fighting over the port.
CAW_PORT=47832 "$bin/caw-desktop" serve >>"$work/daemon.log" 2>&1 &
pids+=("$!")
wait_for "$work/daemon.log" 'listening on .*:47832'
sleep 0.5
set +e
"$bin/caw-desktop" serve --headless </dev/null >"$work/third.log" 2>/dev/null
status=$?
set -e
[ "$status" = 3 ] || { echo "receiver next to a standalone one exited $status, not 3"; cat "$work/third.log"; exit 1; }
grep -q '"event":"stopped","reason":"already_running"' "$work/third.log"

echo "headless test passed"
