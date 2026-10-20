#!/usr/bin/env bash
# Read-only verification of a built or unpacked application, never re-sign it.
set -euo pipefail
if [ "$#" -ne 2 ]; then
  echo 'Usage: verify-macos-signatures.sh <app-bundle> <target-triple>' >&2
  exit 2
fi
app=$1
target=$2
case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) echo "Unsupported macOS target: $target" >&2; exit 2 ;;
esac
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" = org.danmuji.bililiverecorder-gui
test -s "$app/Contents/_CodeSignature/CodeResources"
codesign --verify --deep --strict --verbose=2 "$app"
manifest=$(mktemp)
trap 'rm -f "$manifest"' EXIT
find "$app" -type f -print0 > "$manifest"
seen=0
cli_checked=0
main_checked=0
while IFS= read -r -d '' native; do
  kind=$(file -b "$native")
  case "$kind" in
    Mach-O\ *)
      codesign --verify --strict "$native"
      info=$(codesign --display --verbose=4 "$native" 2>&1)
      printf '%s\n' "$info" | grep -Fxq 'Signature=adhoc'
      lipo "$native" -verify_arch "$arch"
      seen=$((seen + 1))
      if [ "$native" = "$app/Contents/Resources/sidecar/$target/BililiveRecorder.Cli" ]; then cli_checked=1; fi
      if [ "$native" = "$app/Contents/MacOS/bililive-recorder-gui" ]; then main_checked=1; fi
      ;;
  esac
done < "$manifest"
test "$seen" -ge 3
test "$cli_checked" -eq 1
test "$main_checked" -eq 1
echo "Verified sealed ad-hoc $target app and $seen native files (not Apple notarized)."
