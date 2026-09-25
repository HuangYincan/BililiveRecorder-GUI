#!/usr/bin/env bash
# Control-flow tests only: all signing/notarization commands are mocks.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/macos-gate-test-XXXXXX")
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/runner" "$tmp/src-tauri/sidecar/aarch64-apple-darwin" \
  "$tmp/app/Contents/Resources/sidecar/aarch64-apple-darwin"
for root in "$tmp/src-tauri/sidecar/aarch64-apple-darwin" "$tmp/app/Contents/Resources/sidecar/aarch64-apple-darwin"; do
  touch "$root/BililiveRecorder.Cli" "$root/libfake.dylib" "$root/libfake2.dylib" "$root/third.dll"
done
python3 - "$tmp/app/Contents/Info.plist" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'wb') as file:
    plistlib.dump({'CFBundleIdentifier':'org.danmuji.bililiverecorder-gui'}, file)
PY
touch "$tmp/fixture.dmg"
cat > "$tmp/bin/file" <<'MOCK'
#!/usr/bin/env bash
if [[ "$1" == '-b' && "$2" == */third.dll && "${INSPECTION_FAIL:-0}" == 1 ]]; then
  echo 'injected inspection failure' >&2; exit 17
fi
if [[ "$2" == */BililiveRecorder.Cli && "${CLASSIFY_CLI_AS_DATA:-0}" == 1 ]]; then
  echo 'PE32 managed assembly'; exit 0
fi
case "$2" in
  */BililiveRecorder.Cli|*/libfake.dylib|*/libfake2.dylib) echo 'Mach-O 64-bit executable arm64' ;;
  *) echo 'PE32 managed assembly' ;;
esac
MOCK
cat > "$tmp/bin/find" <<'MOCK'
#!/usr/bin/env bash
if [[ "${ENUMERATION_FAIL:-0}" == 1 ]]; then
  printf '%s\0' "$1/BililiveRecorder.Cli" "$1/libfake.dylib"
  echo 'injected enumeration failure' >&2; exit 18
fi
exec /usr/bin/find "$@"
MOCK
cat > "$tmp/bin/codesign" <<'MOCK'
#!/usr/bin/env bash
if [[ "$1" == '--display' && "$2" == '--entitlements' ]]; then
  if [[ "${ENTITLEMENT_MODE:-normal}" == extra ]]; then
    printf '%s\n' '<?xml version="1.0"?><plist version="1.0"><dict><key>com.apple.security.cs.allow-jit</key><true/><key>com.apple.security.get-task-allow</key><true/></dict></plist>'
  elif [[ "${ENTITLEMENT_MODE:-normal}" == missing ]]; then
    printf '%s\n' '<?xml version="1.0"?><plist version="1.0"><dict></dict></plist>'
  else
    cat "$REPO/src-tauri/branding/macos-cli.entitlements.plist"
  fi
elif [[ "$1" == '--display' ]]; then
  printf 'Authority=Developer ID Application: Fixture\nTeamIdentifier=AAAAAAAAAA\n' >&2
fi
MOCK
cat > "$tmp/bin/hdiutil" <<'MOCK'
#!/usr/bin/env bash
case "$1" in
  verify) exit 0 ;;
  attach)
    while [[ "$#" -gt 0 && "$1" != '-mountpoint' ]]; do shift; done
    cp -R "$FIXTURE_APP" "$2/Mikufans录播姬.app" ;;
  detach)
    python3 - "$2/Mikufans录播姬.app" <<'PY'
import shutil,sys
shutil.rmtree(sys.argv[1])
PY
    ;;
  *) exit 2 ;;
esac
MOCK
for cmd in spctl xcrun; do printf '#!/usr/bin/env bash\nexit 0\n' > "$tmp/bin/$cmd"; done
chmod +x "$tmp/bin"/*
export PATH="$tmp/bin:$PATH" REPO="$repo" FIXTURE_APP="$tmp/app" RUNNER_TEMP="$tmp/runner" APPLE_TEAM_ID=AAAAAAAAAA
cd "$tmp"
signer="$repo/scripts/sign-macos-sidecar.sh"
verifier="$repo/scripts/verify-macos-release-dmg.sh"
run_case() {
  label=$1 expected=$2; shift 2
  set +e
  "$@" > "$tmp/$label.out" 2>&1
  result=$?
  set -e
  if [[ "$expected" == fail && "$result" -eq 0 ]] || [[ "$expected" == pass && "$result" -ne 0 ]]; then
    echo "Unexpected $label result=$result" >&2; cat "$tmp/$label.out" >&2; exit 1
  fi
  if [[ "$label" == *inspect* ]]; then grep -q 'Cannot inspect native candidate' "$tmp/$label.out"; fi
  if [[ "$label" == *enumerate* ]]; then grep -q 'injected enumeration failure' "$tmp/$label.out"; fi
  printf '%s: %s (exit %d)\n' "$label" "$expected" "$result"
}
export INSPECTION_FAIL=0 ENUMERATION_FAIL=0
run_case signer-control pass bash "$signer" src-tauri/sidecar/aarch64-apple-darwin 'Developer ID Application: Fixture' AAAAAAAAAA
run_case verifier-control pass bash "$verifier" "$tmp/fixture.dmg" aarch64-apple-darwin
export INSPECTION_FAIL=1
run_case signer-inspect fail bash "$signer" src-tauri/sidecar/aarch64-apple-darwin 'Developer ID Application: Fixture' AAAAAAAAAA
run_case verifier-inspect fail bash "$verifier" "$tmp/fixture.dmg" aarch64-apple-darwin
export INSPECTION_FAIL=0 ENUMERATION_FAIL=1
run_case signer-enumerate fail bash "$signer" src-tauri/sidecar/aarch64-apple-darwin 'Developer ID Application: Fixture' AAAAAAAAAA
run_case verifier-enumerate fail bash "$verifier" "$tmp/fixture.dmg" aarch64-apple-darwin
export ENUMERATION_FAIL=0 ENTITLEMENT_MODE=missing
run_case signer-missing-jit fail bash "$signer" src-tauri/sidecar/aarch64-apple-darwin 'Developer ID Application: Fixture' AAAAAAAAAA
run_case verifier-missing-jit fail bash "$verifier" "$tmp/fixture.dmg" aarch64-apple-darwin
export ENTITLEMENT_MODE=extra
run_case signer-debug-entitlement fail bash "$signer" src-tauri/sidecar/aarch64-apple-darwin 'Developer ID Application: Fixture' AAAAAAAAAA
run_case verifier-debug-entitlement fail bash "$verifier" "$tmp/fixture.dmg" aarch64-apple-darwin
export ENTITLEMENT_MODE=normal CLASSIFY_CLI_AS_DATA=1
run_case signer-cli-not-native fail bash "$signer" src-tauri/sidecar/aarch64-apple-darwin 'Developer ID Application: Fixture' AAAAAAAAAA
run_case verifier-cli-not-native fail bash "$verifier" "$tmp/fixture.dmg" aarch64-apple-darwin
echo '12/12 mocked command-boundary cases passed; no application launched or actually signed.'
