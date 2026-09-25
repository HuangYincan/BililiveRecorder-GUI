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
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
entitlements="$script_dir/../src-tauri/branding/macos-cli.entitlements.plist"
test -f "$entitlements"
manifest=$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/sidecar-native-files-XXXXXX")
embedded=$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/sidecar-entitlements-XXXXXX")
trap 'rm -f "$manifest" "$embedded"' EXIT
# Redirection belongs to a top-level command: an enumerator that writes a
# partial list and then errors must NOT turn into a successful while loop.
find "$root" -type f -print0 > "$manifest"
seen=0
cli_signed=0
while IFS= read -r -d '' native; do
  if ! kind=$(file -b "$native"); then
    echo "Cannot inspect native candidate: $native" >&2
    exit 1
  fi
  case "$kind" in
    Mach-O\ *)
      seen=$((seen + 1))
      if [ "$native" = "$root/BililiveRecorder.Cli" ]; then
        codesign --force --options runtime --timestamp --entitlements "$entitlements" --sign "$identity" "$native"
      else
        codesign --force --options runtime --timestamp --sign "$identity" "$native"
      fi
      codesign --verify --strict --verbose=2 "$native"
      info=$(codesign --display --verbose=4 "$native" 2>&1)
      printf '%s\n' "$info" | grep -Fq 'Authority=Developer ID Application:'
      printf '%s\n' "$info" | grep -Fxq "TeamIdentifier=$team"
      if [ "$native" = "$root/BililiveRecorder.Cli" ]; then
        codesign --display --entitlements - "$native" > "$embedded"
        python3 "$script_dir/verify-macos-cli-entitlements.py" "$embedded"
        cli_signed=1
      fi
      ;;
  esac
done < "$manifest"
test "$seen" -ge 2
test "$cli_signed" -eq 1
echo "Signed $seen native prepared sidecar files with Developer ID Application identity."
