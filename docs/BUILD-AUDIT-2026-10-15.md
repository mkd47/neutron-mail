# Linux build audit (2026-10-15)

Scope: how Neutron Mail is compiled and packaged for Linux (Cargo workspace,
Tauri bundle config, frontend build, CI). Static review only: the audit
environment had no access to crates.io, npm or distro mirrors, so nothing was
compiled. CI on this branch is the first real check of the changes below.

## Changes on this branch

| # | Area | Finding | Change |
|---|------|---------|--------|
| B1 | `Cargo.toml` | No `[profile.release]`: packages were built with Cargo defaults (16 codegen units, no LTO, full symbol table). | `codegen-units = 1`, `lto = "fat"`, `strip = "symbols"`. `panic` stays `unwind` so one panicking command or task can't take the app down. Expect a smaller binary and .deb/.rpm and a slower release link; compare the sizes the CI jobs now print against the README's 10.3 MB RPM. |
| B2 | `tauri.conf.json` | Packages declare only WebKitGTK/GTK. SQLCipher (`rusqlite` `bundled-sqlcipher`) links the system **libcrypto** and `keyring`/`tao` link **libdbus-1**; neither was declared (the Tauri bundler does not compute library deps). | `deb.depends`: `libssl3t64 \| libssl3`, `libdbus-1-3`. `rpm.depends`: `openssl-libs`, `dbus-libs`. |
| B3 | CI | `bundle.targets` lists `deb`, but only the RPM was ever built, so the .deb was untested. | New `deb` job: release build, `dpkg-deb --info`, `apt install`, `ldd` check, artifact upload. Runs in parallel with the other jobs and uses its own rust-cache key. |
| B4 | CI | Package size was never reported. | Both packaging jobs print package and binary sizes. |
| B5 | Repo | `testserver/__pycache__/make_images.cpython-313.pyc` was committed. | Removed; `__pycache__/` and `*.pyc` ignored. |

## Recommendations (not changed here)

- **`package.json` has a `tauri` script but no `@tauri-apps/cli` dependency**, so `npm run tauri` fails. Run
  `npm i -D @tauri-apps/cli@^2`: it ships a prebuilt CLI, so both packaging jobs could use `npx tauri build`
  instead of compiling `tauri-cli` from source (≈2.5 min on a cold cache). Not done here because the lockfile
  can't be regenerated without npm registry access.
- **Two `reqwest` versions in `Cargo.lock`** (0.12 from neutron-core, 0.13 from tauri). Check with
  `cargo tree -i reqwest@0.13.5 -e features` whether tauri's copy is actually compiled; if it is, moving
  neutron-core to 0.13 (note its changed TLS feature names; keep `ring`, not aws-lc) removes a duplicate HTTP stack.
  (`zbus` 4 also appears in the lockfile, but only through keyring's weak `secret-service?` feature; it is not built.)
- **Faster debug runs:** the README's sync timings are from debug builds, where html5ever/ammonia/SQLCipher run
  unoptimised. `[profile.dev.package."*"] opt-level = 2` optimises dependencies only (one-time rebuild cost,
  incremental builds of Neutron's own crates stay fast).
- **CI system packages:** `libayatana-appindicator3-dev` (ubuntu) and `libappindicator-gtk3-devel` (Fedora) are
  only needed for Tauri's `tray-icon` feature, which is not enabled.
- **`cargo audit` and `cargo deny check`** both check RustSec advisories; one of them is enough in CI.
- **`webkit2gtk = "=2.0.2"`** pins the exact version used by wry 0.57. Keep it in step when updating Tauri, or
  loosen it to `"2.0"` so a wry patch bump can't produce a resolution conflict.
- **Secret Service at runtime:** passwords need a Secret Service provider (GNOME Keyring / KWallet). Consider a
  Recommends/Suggests entry for `gnome-keyring` in the packages.
