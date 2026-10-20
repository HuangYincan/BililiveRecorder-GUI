#!/usr/bin/env bash
# Audit exact packaged bytes. This does not remove quarantine or change signatures.
set -euo pipefail
if [ "$#" -ne 2 ]; then
  echo 'Usage: verify-macos-release.sh <dmg-or-app.tar.gz> <target-triple>' >&2
  exit 2
fi
artifact=$1
target=$2
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
temp=$(mktemp -d)
attached=0
cleanup() {
  if [ "$attached" -eq 1 ]; then hdiutil detach "$temp/mount" >/dev/null; fi
  rm -rf "$temp"
}
trap cleanup EXIT
case "$artifact" in
  *.dmg)
    hdiutil verify "$artifact" >/dev/null
    mkdir "$temp/mount"
    hdiutil attach -readonly -nobrowse -noautoopen -mountpoint "$temp/mount" "$artifact" >/dev/null
    attached=1
    app="$temp/mount/Mikufans录播姬.app"
    ;;
  *.app.tar.gz)
    tar -xzf "$artifact" -C "$temp"
    app="$temp/Mikufans录播姬.app"
    ;;
  *) echo "Unsupported macOS artifact: $artifact" >&2; exit 2 ;;
esac
bash "$script_dir/verify-macos-signatures.sh" "$app" "$target"
cli="$app/Contents/Resources/sidecar/$target/BililiveRecorder.Cli"
expected=$(node -p "require('./upstream.json').version.replace(/^v/, '')")
actual=$("$cli" --version)
case "$actual" in
  "$expected"|"$expected"+*) ;;
  *) echo "Packaged CLI version mismatch: $actual" >&2; exit 1 ;;
esac
