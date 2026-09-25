## macOS downloaded-release signing and Gatekeeper gate

**Do not describe Tauri updater `.sig` as Apple code signing.** Updater minisign keys
are independent of Developer ID signing and Apple notarization.

Published `app-v2.20.3` is **not a Gatekeeper-accepted macOS download**. On
September 25, 2026, both public DMGs were downloaded afresh; SHA-256 matched
GitHub release asset digests, and `hdiutil verify` reported valid disk images:

| Release asset | SHA-256 | Published bundle assessment |
| --- | --- | --- |
| `MikufansRecorder_2.20.3_darwin_aarch64.dmg` | `d004c41af7a4669a2c5470eb572e767a0ab0cf1ba2721b5de69f876dd5bb5df7` | App linker/ad-hoc signed; no resource seal; `codesign --verify --deep --strict` reports `code has no resources but signature indicates they must be present`; Gatekeeper rejects. |
| `MikufansRecorder_2.20.3_darwin_x64.dmg` | `a812e9e7cc270c7e82ff168f20bbb7d6293cc66232836ec5631a288612843ac2` | App unsigned (`code object is not signed at all`); Gatekeeper rejects. |

Both embedded CLI executables are **ad-hoc signed** and internally verify, but
`spctl --assess --type execute` rejects them. The .NET sidecar contains additional
native Mach-O dylibs and `createdump`, so signing only the CLI or app entrypoint
is insufficient. Neither DMG contains a stapled notarization ticket; the ARM
app likewise has no stapled ticket. This is an actual published-artifact
signature/notarization defect, not evidence of a truncated download. The exact
Finder dialog from the user's machine is not yet available, so do not claim a
one-to-one reproduction of its localized text. No user installation was
modified, launched, re-signed or stripped of quarantine.

### Release gate and credential requirements

`release.yml` now refuses to start a new draft without the following repository
**Actions secrets** set through GitHub's supported secret UI (never issue
comments):

- `APPLE_CERTIFICATE`: base64 PKCS#12 containing a valid **Developer ID
  Application** certificate and private key; `APPLE_CERTIFICATE_PASSWORD`.
- `APPLE_SIGNING_IDENTITY`: the exact `Developer ID Application: … (TEAMID)`
  identity; `APPLE_TEAM_ID`: the corresponding 10-character Apple Team ID.
- `APPLE_ID` and `APPLE_PASSWORD`: account email and an **app-specific**
  notarization password for the same team. Tauri supports alternate App Store
  Connect API credentials but this workflow intentionally uses the Apple ID
  route to avoid mixing incomplete credential sets.

The pinned official import action makes the certificate available in an
**ephemeral macOS runner**. Before the Tauri build, the prepared (not installed)
sidecar's native Mach-O files are Developer ID-signed. Tauri must then sign and
notarize the app using the configured credentials; both are subject to an
additional independent check. On each macOS release matrix leg, the exact DMG
is downloaded back from its *draft*, its SHA-256 is matched against GitHub's
asset digest, mounted read-only and `scripts/verify-macos-release-dmg.sh`
requires a sealed Developer ID bundle, matching team on all native sidecar
binaries, Gatekeeper acceptance and a stapled app ticket **before publish**.
A failed test stops the public release (a draft may remain; do not delete it
without an explicit recovery decision). The DMG's own notarization/ticket and
Gatekeeper-open verdict should be recorded independently; the app acceptance
checks do not claim a DMG is itself Developer ID signed.

Local diagnostic command (use a fresh downloaded DMG and **independent copy**,
not a user's installed app):

```sh
APPLE_TEAM_ID=<team-id> bash scripts/verify-macos-release-dmg.sh \
  <downloaded.dmg> aarch64-apple-darwin
```

The current release intentionally makes this command fail for **both** macOS
architectures. There is no green end-to-end result until the owner provisions
real Apple credentials and an isolated candidate run succeeds; do not treat a
passing PR build or Tauri updater signature as a Gatekeeper acceptance. A
future corrected release needs a new shell version, independent review, all
platform checks and explicit release authorization. Do not overwrite the
existing public `2.20.3` tag/assets or ask users to bypass Gatekeeper/SIP.

### Hardened-runtime CLI capability and startup proof

The official .NET 8 signing fixture lists JIT among multiple broad debugging
exceptions. This release deliberately gives **only**
`com.apple.security.cs.allow-jit` to the managed CLI **apphost**, not
`get-task-allow`, debugger, unsigned executable memory, arbitrary library
validation bypass or DYLD environment access. All other embedded native dylibs
and `createdump` are signed with the same team but no extra entitlements.
The signing script and downloaded-DMG gate both parse the **embedded** CLI
entitlements and reject any missing or additional keys. The release macOS
matrix launches the signed CLI (`--version`) and the downloaded, copied app
using the existing bounded normal-exit artifact test **only on the isolated
hosted runner**. This is a proposed positive path, not an already completed
Developer ID notarized run: no Apple certificate is currently available. If
signed .NET actually needs another capability, measure that specific failure
first and amend the entitlement set only after security review; never grant
blanket debugging rights to make a test green.
