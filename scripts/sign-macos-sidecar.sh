#!/usr/bin/env bash
# Only sign a fresh, prepared sidecar on a disposable macOS release runner.
set -euo pipefail
if [ "$#" -ne 3 ] || [ -z "$2" ] || [ -z "$3" ]; then
  echo 'Usage: sign-macos-sidecar.sh <prepared-target-dir> <Developer ID Application identity> <Apple Team ID>' >&2
  exit 2
fi
root=$1
identity=$2
team=$3
test -d "$root"
test -f "$root/BililiveRecorder.Cli"
case "$root" in
  src-tauri/sidecar/aarch64-apple-darwin|src-tauri/sidecar/x86_64-apple-darwin) ;;
  *) echo 'Refusing to sign anything outside prepared macOS release sidecar' >&2; exit 2 ;;
esac
case "$identity" in
  'Developer ID Application: '*) ;;
  *) echo 'Identity must be a Developer ID Application certificate' >&2; exit 2 ;;
esac
seen=0
while IFS= read -r -d '' native; do
  if file -b "$native" | grep -q '^Mach-O '; then
    seen=$((seen + 1))
    codesign --force --options runtime --timestamp --sign "$identity" "$native"
    codesign --verify --strict --verbose=2 "$native"
    info=$(codesign --display --verbose=4 "$native" 2>&1)
    printf '%s\n' "$info" | grep -Fq 'Authority=Developer ID Application:'
    printf '%s\n' "$info" | grep -Fxq "TeamIdentifier=$team"
  fi
done < <(find "$root" -type f -print0)
test "$seen" -ge 2
echo "Signed $seen native prepared sidecar files with Developer ID Application identity."
