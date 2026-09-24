#!/usr/bin/env bash
# Only invoke in a disposable macOS runner. All key material is ephemeral.
set -euo pipefail
umask 077
fixture="$(mktemp -d "$RUNNER_TEMP/brand-updater-XXXXXX")"
old_source="$fixture/old-source"
cleanup() {
  if test -d "$old_source"; then git worktree remove --force "$old_source" >/dev/null 2>&1 || true; fi
  rm -rf "$fixture"
}
trap cleanup EXIT

# Actual old-name app source; the fixture-only changes are applied to the
# throwaway worktree by verify-macos-updater-fixture.py, never to the PR tree.
git fetch --quiet --no-tags --depth=1 origin aa39e6cf6b77ed34ac32747f4d6f78064d4e12a0
git worktree add --quiet --detach "$old_source" aa39e6cf6b77ed34ac32747f4d6f78064d4e12a0
npx tauri signer generate --ci --password '' --write-keys "$fixture/updater.key" >/dev/null

# Rebuild current, unchanged production config in the ephemeral runner.
npm run sidecar:prepare > "$fixture/new-build.log" 2>&1
npm run tauri -- build --target "$TAURI_TARGET_TRIPLE" --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}' >> "$fixture/new-build.log" 2>&1
new_app="src-tauri/target/$TAURI_TARGET_TRIPLE/release/bundle/macos/Mikufans录播姬.app"
archive="$fixture/Mikufans录播姬.app.tar.gz"
COPYFILE_DISABLE=1 tar -czf "$archive" -C "$(dirname "$new_app")" "$(basename "$new_app")"
npx tauri signer sign --password '' --private-key-path "$fixture/updater.key" --app-version "$(node -p "require('./package.json').version")" "$archive" > /dev/null

python3 scripts/verify-macos-updater-fixture.py "$new_app" "$archive" "$archive.sig" "$old_source" "$fixture" "$fixture/updater.key.pub" "$(node -p "require('./package.json').version")" "${TAURI_TARGET_TRIPLE%%-apple-darwin}"
