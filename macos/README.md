# macOS preview (Linux remains primary)

This additive overlay prepares a Mac build from the shared Neutron source.
It leaves every existing source, manifest, lockfile, Linux integration and
Linux CI workflow unchanged in the checkout. All substitutions happen in a
fresh staging directory outside it. Exact-match guards fail if upstream code
changes need review. There is no second app repository or maintained copy of
the mail engine.

The shared UI, mail engine, encrypted cache, notes/tasks and email invitation
replies are reused. The Mac build selects Apple Keychain, native notifications
and Dock badges. Recipient suggestions use mail history only. It removes the
Calendar rail and calendar-write checkbox; it never reads Apple Contacts or
Apple Calendar and requests neither permission. Existing GNOME source stays
untouched. Its unused Rust types still compile through the shared library;
the Mac command implementations do not connect to EDS.

## Build

Use a Mac with Xcode command-line tools, Node 22+, stable Rust, both
`aarch64-apple-darwin` and `x86_64-apple-darwin` targets, and Tauri CLI 2.
Python 3.9+ prepares the staging directory:

```sh
python3 macos/prepare.py /tmp/neutron-mail-macos-build
```

Run `npm ci`, generate `src-tauri/icons/icon.icns` using macOS `sips`/`iconutil`,
then `cargo tauri build --target universal-apple-darwin --bundles app,dmg`
inside that staging directory. The complete reproducible procedure is in
`.github/workflows/macos-preview.yml`. The existing Cargo lockfile seeds
resolution; Mac-specific crypto/notification dependencies are added only to
the staging lockfile, which is shipped with the artifact for traceability.
SQLCipher/OpenSSL is bundled for both architectures so the colleague does
not need Homebrew or Rust.

The workflow builds a universal macOS 13+ app, validates Mach-O architectures,
checks the ad-hoc signature and disk image, runs frontend/Rust tests and a
Keychain roundtrip on a temporary CI keychain, and launches the packaged app.
It uploads a DMG, ZIP containing the app, installation notes, hashes, source
manifest and resolved Mac lockfile. No mail credentials are used in CI.
It also runs when shared source changes, so later Linux development is checked
for Mac compatibility without changing the Linux workflow.

## Distribution and limitations

This preview uses Tauri's ad-hoc signing identity `-`. It has no Developer ID
certificate and is not notarized; the colleague may need Apple's per-app
Privacy & Security approval. Do not describe this artifact as notarized or
fully macOS-certified. See `INSTALL.txt` for handoff steps and test limits.
No default-mail-client registration, App Store distribution or auto-updater
is included. New mail features continue to be developed in the shared Linux
source; only platform-specific differences belong in this directory.

References:
- https://v2.tauri.app/distribute/macos-application-bundle/
- https://v2.tauri.app/distribute/sign/macos/#ad-hoc-signing
- https://v2.tauri.app/plugin/notification/
- https://docs.rs/keyring/3.6.3/keyring/
