#!/usr/bin/env bash
# The app .deb on a headless GNOME desktop, in an Ubuntu container (as root):
# installs the released receiver-only .deb and pairs a (fake) phone with it,
# upgrades to the app .deb, checks the files, transcribes public test
# recordings with the packaged model, then runs the installed app on a GNOME
# session (app_test.py). Prints RESULT lines.
# Usage: scripts/gnome/test.sh <folder with both build artifacts>
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
dist=$(cd "$1" && pwd)
export LOGS=/tmp/gnome-logs DEBIAN_FRONTEND=noninteractive
mkdir -p "$LOGS" && chmod 777 "$LOGS"
failed=()
result() { # name PASS|FAIL|INFO detail
  echo "RESULT $1: $2${3:+ ($3)}"
  [ "$2" != FAIL ] || failed+=("$1")
}
as_tester() { runuser -u tester -- env HOME=/home/tester XDG_RUNTIME_DIR=/run/user/1500 PATH=/usr/bin:/bin "$@"; }

"$here/setup.sh"
apt-get install -y -qq --no-install-recommends ca-certificates curl >/dev/null

echo "== the receiver-only package as released (0.3.0)"
curl -sSfL -o /tmp/caw-0.3.0.deb https://github.com/birdbrainlabs/caw-desktop/releases/download/v0.3.0/caw-desktop-linux-x86_64.deb
apt-get install -y -qq --no-install-recommends /tmp/caw-0.3.0.deb >/dev/null
dpkg-query -W caw-desktop
mkdir -p /tmp/receiver && tar -xzf "$dist/caw-desktop-linux-x86_64.tar.gz" -C /tmp/receiver && chmod -R a+rX /tmp/receiver

echo "== pair a fake phone with the old receiver (started as at login)"
as_tester bash -ec '
  setsid /usr/bin/caw-desktop serve >"$0/old-serve.log" 2>&1 </dev/null &
  sleep 3
  caw-desktop pair >"$0/old-pair.log" 2>&1 &
  for _ in $(seq 20); do
    code=$(grep -o "[0-9A-Z]\{4\}-[0-9A-Z]\{4\}-[0-9A-Z]\{4\}-[0-9A-Z]\{4\}" "$0/old-pair.log" | tr -d - || true)
    [ -n "$code" ] && break
    sleep 0.5
  done
  [ -n "$code" ] || { echo "no pairing code"; cat "$0/old-pair.log" "$0/old-serve.log"; exit 1; }
  XDG_CONFIG_HOME=$HOME/phone /tmp/receiver/caw-fake-phone pair "caw://pair?h=127.0.0.1&p=47820&c=$code&n=test"
  caw-desktop devices
' "$LOGS"
before=$(as_tester caw-desktop devices)
grep -q "Fake phone" <<<"$before" && result paired-with-old-receiver PASS "$before" || result paired-with-old-receiver FAIL "$before"

echo "== upgrade to the app package"
apt-get install -y --no-install-recommends "$dist/caw-desktop-app-linux-x86_64.deb" 2>&1 | tail -n 25
version=$(dpkg-query -W -f '${Version}' caw-desktop)
caw-desktop --version
result upgraded INFO "caw-desktop $version, $(du -k "$dist/caw-desktop-app-linux-x86_64.deb" | cut -f1) KB .deb"
check_file() { if [ -e "$2" ]; then result "$1" PASS "$2"; else result "$1" FAIL "$2 missing"; fi; }
check_file app-launcher /usr/share/applications/dev.birdbrainlabs.CawDesktop.desktop
check_file app-autostart /etc/xdg/autostart/dev.birdbrainlabs.CawDesktop.desktop
if [ -e /etc/xdg/autostart/caw-desktop.desktop ]; then result old-autostart-removed FAIL "still there"; else result old-autostart-removed PASS; fi
check_file packaged-model /usr/share/caw-desktop/models/moonshine-base-en/encode.int8.onnx
# Ubuntu's container images leave /usr/share/doc out of every package, so look in the package's file list.
for f in /usr/share/caw-desktop/models/moonshine-base-en/LICENSE /usr/share/doc/caw-desktop/licenses/MOONSHINE-LICENSE /usr/share/doc/caw-desktop/licenses/SHERPA-ONNX-LICENSE /usr/share/doc/caw-desktop/licenses/ONNXRUNTIME-LICENSE; do
  dpkg -L caw-desktop | grep -qx "$f" && result "license-in-package $(basename "$f")" PASS "$f" || result "license-in-package $(basename "$f")" FAIL "$f not in the package"
done
check_file tray-icon /usr/share/icons/hicolor/scalable/status/caw-waveform-listening-symbolic.svg
ls -la /usr/share/caw-desktop/models/moonshine-base-en /usr/share/doc/caw-desktop/licenses
after=$(as_tester caw-desktop devices)
grep -q "Fake phone" <<<"$after" && result pairing-kept-in-config PASS "$after" || result pairing-kept-in-config FAIL "$after"

echo "== dictate --file with the packaged model (public LibriSpeech-style test recordings from the model's release)"
wavs=$dist/target/model/test_wavs
chmod -R a+rX "$dist"
for w in 0.wav 1.wav; do
  heard=$(as_tester caw-desktop dictate --file "$wavs/$w" 2>"$LOGS/dictate-$w.log" || true)
  expected=$(grep "^$w " "$wavs/trans.txt" | cut -d' ' -f2-)
  score=$(python3 - "$heard" "$expected" <<'EOF'
import re, sys
h = set(re.findall(r"[a-z']+", sys.argv[1].lower()))
e = re.findall(r"[a-z']+", sys.argv[2].lower())
print(round(100 * sum(w in h for w in e) / max(1, len(e))))
EOF
)
  echo "$w heard: $heard"
  [ "$score" -ge 80 ] && result "dictate-file-$w" PASS "$score% of the words" || { result "dictate-file-$w" FAIL "$score% of the words"; cat "$LOGS/dictate-$w.log"; }
done

echo "== the app on GNOME"
chmod -R a+rX "$here"
set +e
as_tester dbus-run-session -- "$here/session.sh" python3 "$here/app_test.py" "$dist" 2>&1 | tee "$LOGS/app_test.out"
status=${PIPESTATUS[0]}
set -e
[ "$status" = 0 ] || failed+=(app_test)

echo "== remove"
apt-get purge -y -qq caw-desktop >/dev/null
if [ -e /usr/bin/caw-desktop ] || [ -e /usr/share/caw-desktop ]; then result purge FAIL "files left"; else result purge PASS; fi
if pgrep -f '^(/usr/bin/)?caw-desktop ' >/dev/null; then result stopped-on-remove FAIL "$(pgrep -af caw-desktop)"; else result stopped-on-remove PASS; fi

echo "== summary"
grep -h '^RESULT' "$LOGS/app_test.out" || true
if [ ${#failed[@]} -gt 0 ]; then
  echo "FAILED: ${failed[*]}"
  exit 1
fi
