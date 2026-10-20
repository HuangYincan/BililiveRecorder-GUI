# macOS free ad-hoc signing

Desktop 2.20.4 uses `bundle.macOS.signingIdentity: "-"` with hardened runtime
disabled. No Apple account, certificate or notarization credentials are needed.
This repairs missing signatures and broken resource seals; it does not grant
Apple trust. Gatekeeper may still block a downloaded app at first launch.
Tauri updater `.sig` files authenticate update packages independently.

`prepare-sidecar.mjs` ad-hoc signs every native Mach-O in the fresh sidecar
directory, including the .NET apphost, runtime libraries and native helpers.
Tauri then signs the main executable and seals the complete app before producing
the DMG and updater archive. Files must not be changed after that final seal.
The .NET apphost does not use hardened runtime, so it needs no extra JIT or
debugging entitlements for this signing mode.

CI verifies the app's strict resource seal, every native file's signature and
architecture, and starts the CLI from the DMG. Real `codesign` fixtures ensure
that changed resources and unsigned native libraries are rejected. The existing
installed-app lifecycle tests also run on disposable hosted runners.
Release jobs download both the draft DMG and updater archive, compare each to
GitHub's SHA-256 asset digest, verify the packaged signatures, start each
packaged CLI, and test the downloaded updater app's backend lifecycle before
the draft can become public. These are integrity and runtime checks, not
Gatekeeper acceptance or notarization checks.

## Review of PR #12

Standards: no blocking documented-standard violations were found.

Spec: PR #12 targets paid Developer ID signing and notarization. Its global
Apple-credentials preflight would stop all platform releases with the current
repository secrets, and its verifier intentionally rejects ad-hoc signatures.
Its positive Apple signing path remains unverified. It cannot be merged as the
free self-signing fix. This implementation reuses the review principles of
enumerating native files and checking the exact downloaded artifacts, without
requiring paid credentials or claiming notarization.

The pinned Tauri CLI 2.11.5 copies resource files separately from its explicit
framework and executable signing targets. Therefore the resource sidecar must
be signed before Tauri seals the enclosing app. Source:
`tauri-apps/tauri`, tag `tauri-cli-v2.11.5`,
`crates/tauri-bundler/src/bundle/macos/app.rs` and `sign.rs`.
