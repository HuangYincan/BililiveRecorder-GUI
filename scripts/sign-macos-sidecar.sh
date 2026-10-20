#!/usr/bin/env bash
# Sign fresh build inputs before Tauri seals and packages the enclosing app.
set -euo pipefail
if [ "$#" -ne 1 ]; then
  echo 'Usage: sign-macos-sidecar.sh <prepared-macOS-target-dir>' >&2
  exit 2
fi
root=$1
case "$root" in
  src-tauri/sidecar/aarch64-apple-darwin|src-tauri/sidecar/x86_64-apple-darwin) ;;
  *) echo 'Expected a prepared macOS sidecar directory' >&2; exit 2 ;;
esac
test -f "$root/BililiveRecorder.Cli"
manifest=$(mktemp)
trap 'rm -f "$manifest"' EXIT
find "$root" -type f -print0 > "$manifest"
seen=0
cli_signed=0
while IFS= read -r -d '' native; do
  kind=$(file -b "$native")
  case "$kind" in
    Mach-O\ *)
      # Free ad-hoc signing does not enable hardened runtime. The self-contained
      # .NET runtime can JIT without granting debugging or memory exceptions.
      codesign --force --sign - --timestamp=none "$native"
      codesign --verify --strict "$native"
      seen=$((seen + 1))
      if [ "$native" = "$root/BililiveRecorder.Cli" ]; then cli_signed=1; fi
      ;;
  esac
done < "$manifest"
test "$seen" -ge 2
test "$cli_signed" -eq 1
echo "Ad-hoc signed $seen native sidecar files."
