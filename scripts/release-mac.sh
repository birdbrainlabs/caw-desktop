#!/bin/bash
#
# Builds Caw Desktop.app for macOS: universal (Apple silicon and Intel),
# signed with a Developer ID certificate, hardened runtime, notarized by Apple
# and stapled. Maintainer only: it needs a "Developer ID Application"
# certificate in the login keychain and notarytool credentials saved under the
# profile name below (xcrun notarytool store-credentials caw-notary ...).
#
#   scripts/release-mac.sh                 build, sign, notarize
#   scripts/release-mac.sh --no-notarize   build and sign only (to test; still
#                                          Developer ID signed, so the
#                                          Accessibility permission carries over)
#
# Output: target/macos/caw-desktop-macos.zip (the app inside) and a .sha256 file.

set -euo pipefail
cd "$(dirname "$0")/.."

NOTARY_PROFILE=caw-notary
BUNDLE_ID=com.birdbrainlabs.caw-desktop
notarize=1
[[ "${1:-}" == "--no-notarize" ]] && notarize=0

# The certificate is found by its hash, so no name is written anywhere.
identity=$(security find-identity -v -p codesigning | grep -m1 '"Developer ID Application:' | awk '{print $2}' || true)
if [[ -z "$identity" ]]; then
    echo "No Developer ID Application certificate in the keychain." >&2
    exit 1
fi

if (( notarize )) && ! xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1; then
    echo "No notarytool credentials saved as \"$NOTARY_PROFILE\"." >&2
    exit 1
fi

version=$(sed -nE 's/^version = "([^"]+)"/\1/p' Cargo.toml | head -1)
plist_version=$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' packaging/macos/Info.plist)
if [[ "$version" != "$plist_version" ]]; then
    echo "Cargo.toml says $version but packaging/macos/Info.plist says $plist_version." >&2
    exit 1
fi

out=target/macos
build=$out/build
app="$out/Caw Desktop.app"
zip=$out/caw-desktop-macos.zip
rm -rf "$app" "$zip" "$zip.sha256"
mkdir -p "$out"

echo "── Building $version"
# Paths from this Mac (the home folder, where the source is) are rewritten so
# none end up in the binary; symbols and debug info are left out.
export RUSTFLAGS="--remap-path-prefix=$HOME=/build --remap-path-prefix=$PWD=."
export CARGO_PROFILE_RELEASE_STRIP=symbols CARGO_PROFILE_RELEASE_DEBUG=0
export MACOSX_DEPLOYMENT_TARGET=13.0
for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --locked --bin caw-desktop --target "$target" --target-dir "$build"
done

echo "── Assembling the app"
mkdir -p "$app/Contents/MacOS"
lipo -create -output "$app/Contents/MacOS/caw-desktop" \
    "$build/aarch64-apple-darwin/release/caw-desktop" "$build/x86_64-apple-darwin/release/caw-desktop"
cp packaging/macos/Info.plist "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

# No build machine paths (home folder, user name) left in the app.
if LC_ALL=C grep -r -a -q "$HOME" "$app"; then
    echo "The app contains a path from this Mac ($HOME)." >&2
    exit 1
fi

echo "── Signing"
# Hardened runtime and no entitlements: typing needs only the user's
# Accessibility permission, which macOS ties to this signature.
codesign --force --timestamp --options runtime --identifier "$BUNDLE_ID" --sign "$identity" "$app"
codesign --verify --deep --strict "$app"
# (Captured first: grep -q stopping early would fail the pipe under pipefail.)
details=$(codesign -d --verbose=2 "$app" 2>&1)
grep -q 'flags=0x10000(runtime)' <<<"$details" || { echo "Hardened runtime is not on." >&2; exit 1; }
entitlements=$(codesign -d --entitlements - --xml "$app" 2>/dev/null || true)
if grep -q get-task-allow <<<"$entitlements"; then
    echo "The app carries get-task-allow; Apple would refuse it." >&2
    exit 1
fi
lipo -archs "$app/Contents/MacOS/caw-desktop"

pack() {
    rm -f "$zip"
    ditto -c -k --keepParent "$app" "$zip"
}
pack

if (( notarize )); then
    echo "── Notarizing (Apple usually answers within a few minutes)"
    xcrun notarytool submit "$zip" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$app"
    spctl --assess --type execute -vv "$app"
    pack
fi

(cd "$out" && shasum -a 256 "$(basename "$zip")" > "$(basename "$zip").sha256")
echo "$zip"
