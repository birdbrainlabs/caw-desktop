#!/usr/bin/env bash
# Fetches the speech model the app .deb ships: Moonshine base (English, int8),
# from the official sherpa-onnx release, checked against a known SHA-256. The
# model is MIT licensed (its LICENSE file comes along). Also keeps the
# release's three public test recordings (LibriSpeech) and their transcripts
# for the tests. Usage: scripts/fetch-model.sh [dest, default target/model]
set -euo pipefail
dest=${1:-target/model}
url=https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-base-en-int8.tar.bz2
sha256=21870cecaa2e44e4e2bf63e02d1072bed183ccd10284871353bd9d24dad14e5e
tmp=$(mktemp -d)
curl -sSfL --retry 3 -o "$tmp/model.tar.bz2" "$url"
echo "$sha256  $tmp/model.tar.bz2" | sha256sum -c -
tar -xjf "$tmp/model.tar.bz2" -C "$tmp"
src=$tmp/sherpa-onnx-moonshine-base-en-int8
mkdir -p "$dest/moonshine-base-en" "$dest/test_wavs"
cp "$src"/{preprocess.onnx,encode.int8.onnx,uncached_decode.int8.onnx,cached_decode.int8.onnx,tokens.txt,LICENSE,README.md} "$dest/moonshine-base-en/"
cp "$src"/test_wavs/* "$dest/test_wavs/"
ls -l "$dest/moonshine-base-en"
