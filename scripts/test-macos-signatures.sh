#!/usr/bin/env bash
# Real codesign regression test using tiny native fixtures; no app is launched.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
temp=$(mktemp -d)
trap 'rm -rf "$temp"' EXIT
case "$(uname -m)" in
  arm64) target=aarch64-apple-darwin ;;
  x86_64) target=x86_64-apple-darwin ;;
  *) exit 2 ;;
esac
cd "$temp"
prepared="src-tauri/sidecar/$target"
app="$temp/Fixture.app"
mkdir -p "$prepared" "$app/Contents/MacOS" "$app/Contents/Resources/sidecar"
printf 'int main(void) { return 0; }\n' > main.c
printf 'int fixture(void) { return 42; }\n' > library.c
clang main.c -o "$prepared/BililiveRecorder.Cli"
clang -dynamiclib library.c -o "$prepared/libfixture.dylib"
bash "$repo/scripts/sign-macos-sidecar.sh" "$prepared"
cp -R "$prepared" "$app/Contents/Resources/sidecar/"
cp "$prepared/BililiveRecorder.Cli" "$app/Contents/MacOS/bililive-recorder-gui"
python3 - "$app/Contents/Info.plist" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'wb') as output:
    plistlib.dump({
        'CFBundleIdentifier': 'org.danmuji.bililiverecorder-gui',
        'CFBundleExecutable': 'bililive-recorder-gui',
        'CFBundlePackageType': 'APPL',
    }, output)
PY
codesign --force --sign - --timestamp=none "$app"
bash "$repo/scripts/verify-macos-signatures.sh" "$app" "$target"
printf 'tampered\n' > "$app/Contents/Resources/extra.txt"
if bash "$repo/scripts/verify-macos-signatures.sh" "$app" "$target"; then
  echo 'Verifier accepted a broken resource seal' >&2; exit 1
fi
rm "$app/Contents/Resources/extra.txt"
codesign --remove-signature "$app/Contents/Resources/sidecar/$target/libfixture.dylib"
codesign --force --sign - --timestamp=none "$app"
if bash "$repo/scripts/verify-macos-signatures.sh" "$app" "$target"; then
  echo 'Verifier accepted an unsigned native library' >&2; exit 1
fi
echo 'Real signature checks passed: valid bundle accepted, altered resources and unsigned dylib rejected.'
