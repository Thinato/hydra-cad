#!/usr/bin/env bash
# Builds the macOS release: a universal Hydra.app in a DMG, plus a zip of the universal
# command-line tools (hydra-mcp, hydra-admin). Outputs go to $CARGO_TARGET_DIR/dist
# (default target/package/dist):
#   Hydra-<version>-macos-universal.dmg
#   hydra-tools-<version>-macos-universal.zip
#
# usage: build-dmg.sh [--build-only | --no-build]
#   --build-only  stop after the cargo builds (CI runs this step without any secret)
#   --no-build    skip cargo and package the release binaries already in the target dir
#   (no flag: build and package)
#
# Optional environment:
#   MACOS_SIGNING_IDENTITY  "Developer ID Application: …" in the keychain; unset = ad-hoc signing
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD  notarize with an app-specific password, or
#   NOTARYTOOL_PROFILE      a profile saved with `xcrun notarytool store-credentials`
# Notarization runs only with a real signing identity.
set -euo pipefail

mode=all
case "${1:-}" in
  '') ;;
  --build-only) mode=build ;;
  --no-build) mode=package ;;
  *) echo "usage: $0 [--build-only | --no-build]" >&2; exit 2 ;;
esac

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
version="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' Cargo.toml)"
[[ -n "$version" ]] || { echo "error: no version in [workspace.package] of Cargo.toml" >&2; exit 1; }

# A target dir of its own, so packaging never waits on (or invalidates) a dev build.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target/package}"
# Binaries would run from macOS 11; Info.plist still requires 15.0, the supported floor.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
identity="${MACOS_SIGNING_IDENTITY:-}"
targets=(aarch64-apple-darwin x86_64-apple-darwin)
bins=(hydra hydra-mcp hydra-admin)
dist="$CARGO_TARGET_DIR/dist"
work="$CARGO_TARGET_DIR/macos"

if [[ "$mode" != package ]]; then
  rustup target add "${targets[@]}"
  cargo build --release --locked -p hydra -p hydra-mcp -p hydra-db --bins \
    --target "${targets[0]}" --target "${targets[1]}"
fi
[[ "$mode" != build ]] || exit 0

rm -rf "$work" && mkdir -p "$work/universal" "$dist"
for bin in "${bins[@]}"; do
  lipo -create -output "$work/universal/$bin" \
    "$CARGO_TARGET_DIR/${targets[0]}/release/$bin" "$CARGO_TARGET_DIR/${targets[1]}/release/$bin"
done

# sign PATH [codesign args…]: Developer ID with hardened runtime and a secure timestamp,
# or ad-hoc when no identity is configured (runs locally, rejected by Gatekeeper elsewhere).
sign() {
  if [[ -n "$identity" ]]; then
    codesign --force --options runtime --timestamp --sign "$identity" "$@"
  else
    codesign --force --sign - "$@"
  fi
}

# --- Hydra.app ---
app="$work/Hydra.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$work/universal/hydra" "$app/Contents/MacOS/hydra"
cp "$root/packaging/macos/Hydra.icns" "$app/Contents/Resources/Hydra.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>Hydra</string>
  <key>CFBundleExecutable</key><string>hydra</string>
  <key>CFBundleIconFile</key><string>Hydra</string>
  <key>CFBundleIdentifier</key><string>com.thinato.hydra</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>Hydra</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist"
sign "$app"
codesign --verify --deep --strict "$app"

# --- DMG: the app next to an /Applications link to drag it onto ---
dmg="$dist/Hydra-$version-macos-universal.dmg"
stage="$work/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -volname "Hydra $version" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg"
if [[ -n "$identity" ]]; then sign "$dmg"; fi

# --- Tools zip ---
tools="$work/hydra-tools-$version-macos-universal"
mkdir -p "$tools"
for bin in hydra-mcp hydra-admin; do
  cp "$work/universal/$bin" "$tools/$bin"
  sign "$tools/$bin" --identifier "com.thinato.$bin"
done
zip="$dist/hydra-tools-$version-macos-universal.zip"
rm -f "$zip"
ditto -c -k --norsrc --noextattr --keepParent "$tools" "$zip"

# --- Notarization ---
notary=()
if [[ -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" ]]; then
  notary=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD")
elif [[ -n "${NOTARYTOOL_PROFILE:-}" ]]; then
  notary=(--keychain-profile "$NOTARYTOOL_PROFILE")
fi
if [[ ${#notary[@]} -eq 0 ]]; then
  echo "Notarization skipped: no APPLE_ID/APPLE_TEAM_ID/APPLE_APP_PASSWORD or NOTARYTOOL_PROFILE."
elif [[ -z "$identity" ]]; then
  echo "Notarization skipped: ad-hoc signed builds cannot be notarized (set MACOS_SIGNING_IDENTITY)."
else
  # The DMG ticket covers the app inside it; the stapled ticket lets Gatekeeper check the
  # DMG offline. A zip cannot be stapled: its binaries are checked online on first run.
  notarize() {
    local out id status
    out="$(xcrun notarytool submit "$1" "${notary[@]}" --wait --output-format json)"
    id="$(plutil -extract id raw - <<<"$out")"
    status="$(plutil -extract status raw - <<<"$out")"
    echo "Notarization of $(basename "$1"): $status ($id)"
    if [[ "$status" != Accepted ]]; then
      xcrun notarytool log "$id" "${notary[@]}" || true
      exit 1
    fi
  }
  notarize "$dmg"
  xcrun stapler staple "$dmg"
  notarize "$zip"
fi

echo "Built:"
ls -l "$dmg" "$zip"
