# Mikufans录播姬 application branding

![Official upstream logo on an actual opaque white icon canvas](../src-tauri/icons/icon.png)

## Original art and reproducible icon generation

- The **unchanged** source `src-tauri/branding/upstream-logo.svg` is the exact user-selected [official SVG](https://github.com/BililiveRecorder/BililiveRecorder/blob/2744d605b93fec44d00a7957de93ec3148e412fc/.github/assets/logo.svg) at upstream commit `2744d605b93fec44d00a7957de93ec3148e412fc` (Git blob `67a37810cbaf6dd3a9c4bbb709a5bdaa1a2c38d8`). Source SHA-256: `d3d03458c3daecab6444f1f0654543ec2dd343db3448bdfa4f57ebe4c3c1dce0`.
- `npm ci && npm run brand:icons` pins Tauri CLI to `2.11.5` and runs `scripts/prepare-brand-svg.mjs`: it inserts **only** one full 1024×1024 opaque white rectangle **behind** the original four paths. The original SVG, shapes and original blue `#1c9bcd` / off-white `#f7f8f8` path fills remain unchanged. Tauri generates the committed PNG, ICO and ICNS sizes (and unused mobile icon variants) from `icon-white.svg`. Tauri writes ICNS chunks in nondeterministic order; `scripts/canonicalize-icns.mjs` sorts the unchanged chunk bytes so repeated runs produce the same file.
- `npm run brand:verify` checks the official file digest, exact transformation, all desktop PNG pixels' alpha, white corners, visible blue artwork, 6 PNG-based ICO sizes and 8 PNG-based ICNS sizes. White canvas is pixels, not a viewer/checkerboard background. The preview above is the generated `icon.png`, not an artist re-creation.

## Names and old updater/installer identity

The visible Tauri `productName`, window/dialog title, app bundle/installers and auto-generated shortcuts use **Mikufans录播姬**. This branding PR does **not** bump the desktop version or dispatch a release. Keep these existing identifiers for the previously installed 2.20.1 clients:

| Identity | Value / action |
| --- | --- |
| GitHub repo, Cargo/npm package, executable | Keep `HuangYincan/BililiveRecorder-GUI`, `bililive-recorder-gui`, and its binary filename; no repo or package rename. |
| Tauri bundle identifier, app log/config/data directory | Keep `org.danmuji.bililiverecorder-gui`; do not move/erase the existing `BililiveRecorder` recordings folder. |
| Updater endpoint and public signing key | Keep existing values byte-for-byte; a new **higher** shell version and correctly signed updater will be required for a later release. |
| Windows MSI WiX UpgradeCode | Pin prior release's default `3a5e8872-ebd5-524f-9fe9-825771b0859b`; Tauri's default would silently change to `0eb775cd-5730-5440-8a50-c462a1a1244f` after renaming `productName`. |
| Windows NSIS uninstall/install-location registry keys | Pin previously published `BililiveRecorder GUI` keys in the [v2.11.5 Tauri NSIS template](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi). The vendored template retains new visible product labels, detects old/new MSI names when migrating, and renames *only* old shortcuts whose target is this installation's unchanged binary. Preserve a user's choice of no shortcut and never delete unrelated links. |
| macOS app display name | New `.app` filename/CFBundleName, but same CFBundleIdentifier and updater signing key. This proves metadata continuity, **not** that a previously installed `.app` has successfully undergone an actual updater-driven in-place rename on a disposable VM. |
| Linux package vs desktop Name | Keep package/binary name; use new visible `.desktop` Name. Linux graphical installation/updating remains a separate unverified item. |

Debian package identifiers cannot contain Chinese characters; CI builds DEB/RPM with the legacy ASCII `productName` solely for packaging and a pinned `.desktop` template overrides their *visible* Name to `Mikufans录播姬`. AppImage is built separately with the exact new product name. WiX uses `zh-CN` so its database code page supports the requested product name rather than silently replacing it.

The checks in `build.yml` and `release.yml` validate the produced macOS Info.plist, Linux DEB metadata/desktop entry, Windows MSI ProductName/UpgradeCode and NSIS installer version metadata on **isolated CI runners**. The packaging CLI smoke and existing normal/forced lifecycle tests remain separate checks. `scripts/generate-release-manifest.mjs` locates a unique signed updater asset with the *current* product prefix and URL-encodes Unicode filenames; a missing/ambiguous/old-name asset fails closed. `npm run brand:verify` covers its positive and failure fixtures.

**Remaining gate before authorizing a new branded release:** version bump, review and final four-platform signed release, plus isolated old→new updater migration evidence, especially NSIS and macOS's renamed `.app`; signing and installer identity metadata checks alone are not end-to-end update proof. No test here uses the shared user recording instance or its data.

## Disposable macOS updater fixture

For this branding PR, `build.yml` includes an isolated macOS ARM updater job after the four-platform package checks. The fixture builds **old-name app code** pinned to published `main` commit `aa39e6c` at test-only version `2.20.0` and the current **new-name app** at its real shell version `2.20.1` (or higher if later bumped). A fresh, runner-only minisign key and loopback endpoint are injected **only into the throwaway old-source worktree**; the old fixture's consent dialog is replaced with explicit CI consent, but its real updater check, signed download, install and restart are not bypassed. The runner records the old install path, new Info.plist name/version/identifier, exact ICNS bytes, restarted app ready event and a synthetic work-directory sentinel's hash. All fixture files and keys are deleted in the same job; no real user recordings, public tag, asset, endpoint or production key are used.

This is deliberately a **reconstructed old app**, not an end-to-end test of the already published 2.20.1 binary. The disk `.app` filename stays old after an updater install; keeping the old path is the chosen compatibility policy, not evidence of an automatic rename.
