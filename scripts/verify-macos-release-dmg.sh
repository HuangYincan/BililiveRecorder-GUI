#!/usr/bin/env bash
# Fail closed on the EXACT downloaded release DMG; never modify an installed app.
set -euo pipefail
if [ "$#" -ne 2 ] || [ -z "${APPLE_TEAM_ID:-}" ]; then
  echo 'Usage: APPLE_TEAM_ID=<10-char team ID> verify-macos-release-dmg.sh <downloaded.dmg> <target-triple>' >&2
  exit 2
fi
dmg=$1
target=$2
case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin) ;;
  *) echo "Unsupported macOS target: $target" >&2; exit 2 ;;
esac
test -f "$dmg"
hdiutil verify "$dmg" >/dev/null
mount=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/gatekeeper-mount-XXXXXX")
attached=0
manifest=$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/bundled-native-files-XXXXXX")
embedded=$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/bundled-entitlements-XXXXXX")
cleanup() {
  rm -f "$manifest" "$embedded"
  if [ "$attached" -eq 1 ]; then hdiutil detach "$mount" >/dev/null; fi
  rmdir "$mount"
}
trap cleanup EXIT
hdiutil attach -readonly -nobrowse -noautoopen -mountpoint "$mount" "$dmg" >/dev/null
attached=1
app="$mount/Mikufans录播姬.app"
sidecar="$app/Contents/Resources/sidecar/$target/BililiveRecorder.Cli"
test -f "$app/Contents/Info.plist"
test -f "$sidecar"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" = org.danmuji.bililiverecorder-gui
codesign --verify --deep --strict --verbose=2 "$app"
# Codesign's success alone is insufficient: ad-hoc signatures can verify locally.
identity=$(codesign --display --verbose=4 "$app" 2>&1)
printf '%s\n' "$identity" | grep -Fq 'Authority=Developer ID Application:'
printf '%s\n' "$identity" | grep -Fxq "TeamIdentifier=$APPLE_TEAM_ID"
# The bundled .NET runtime contains native dylibs beyond the CLI executable.
# Audit every native Mach-O file rather than only the main sidecar.
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# A failed find must fail this gate even if it emitted two valid entries first.
find "$app/Contents/Resources/sidecar/$target" -type f -print0 > "$manifest"
seen=0
cli_checked=0
while IFS= read -r -d '' native; do
  if ! kind=$(file -b "$native"); then
    echo "Cannot inspect native candidate: $native" >&2
    exit 1
  fi
  case "$kind" in
    Mach-O\ *)
      seen=$((seen + 1))
      codesign --verify --strict --verbose=2 "$native"
      info=$(codesign --display --verbose=4 "$native" 2>&1)
      printf '%s\n' "$info" | grep -Fq 'Authority=Developer ID Application:'
      printf '%s\n' "$info" | grep -Fxq "TeamIdentifier=$APPLE_TEAM_ID"
      if [ "$native" = "$sidecar" ]; then
        codesign --display --entitlements - "$native" > "$embedded"
        python3 "$script_dir/verify-macos-cli-entitlements.py" "$embedded"
        cli_checked=1
      fi
      ;;
  esac
done < "$manifest"
test "$seen" -ge 2
test "$cli_checked" -eq 1
spctl --assess --type execute --verbose=4 "$app"
xcrun stapler validate "$app"
echo "Accepted signed, notarized $target app from downloaded DMG; checked $seen native sidecar files."
