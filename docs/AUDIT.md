# Neutron Mail – Security & Dependency Audit (prototype v0.1.0)

*Date: 2026-10-14/15 (Asia/Shanghai). Scope: `crates/neutron-core`, `src-tauri`, `src/` (Svelte UI), `testserver/`.
Performed on the build box (Debian 13, rustc 1.99, Node 20.19). Author: Kaan Durmus (with Grok Bot).*

Severity scale: **High** (exploitable by a remote sender/server with real impact) · **Medium** · **Low** · **Info**.

## 1. Tool results

| Tool | Result |
|---|---|
| `cargo audit` (RustSec DB, 1314 advisories, 546 crates) | **0 vulnerabilities**, 2 warnings (both transitive via Tauri's Linux GTK3 stack): RUSTSEC-2024-0370 `proc-macro-error` unmaintained (build-time proc macro); RUSTSEC-2024-0429 `glib 0.18.5` unsound `VariantStrIter` (API not used by Neutron; fix needs gtk-rs ≥ 0.20, which Tauri 2/wry don't use yet). |
| `cargo deny check` (`deny.toml`) | **advisories ok, bans ok, licenses ok, sources ok.** Licenses limited to permissive/weak-copyleft (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, MPL-2.0, …). Only crates.io; no git deps; no wildcards. RUSTSEC-2024-0370 ignored with documented reason. |
| `npm audit` | **0 vulnerabilities** (all deps and `--omit=dev`). Only one runtime npm dependency: `@tauri-apps/api`. |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean (`unsafe_code = "forbid"` workspace-wide) |
| `svelte-check --fail-on-warnings` | 0 errors, 0 warnings |
| Tests | 28 pass with the live local server (`NEUTRON_IT=1`): 2 unit, 6 IMAP sync, 4 SMTP, 7 sanitizer, 9 store/search/TLS. Without the server the 10 network tests return early (counted as "passed" — they are *skipped*, not run). |

## 2. Review by area

### 2.1 HTML rendering (remote sender → viewer) — defence in depth
1. **ammonia allow-list** (`sanitize.rs`): scripts, `<style>`, forms/inputs, iframes, objects, event handlers and `javascript:` URLs removed; link schemes limited to http/https/mailto; relative URLs denied; `rel="noopener noreferrer nofollow"` added. `<img src>` kept **only** for raster `data:image/{png,jpeg,gif,webp}` (no SVG, no `srcset`). Remote images are removed and counted ("N remote images blocked").
2. **Per-message CSP** injected into the iframe document: `default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src 'none'; form-action 'none'; base-uri 'none'`, so even a sanitizer bypass could not fetch remote resources (tracking) or load fonts.
3. **Sandboxed iframe** `sandbox=""` (no scripts, opaque origin, no forms/popups/top navigation) with `srcdoc` and `referrerpolicy="no-referrer"`; it cannot reach the Tauri IPC bridge.
4. Search snippets use U+0002/U+0003 markers rendered as Svelte text nodes, never `{@html}` (the UI contains no `{@html}` at all).

Verified: 7 sanitizer tests (script/iframe/form/on*/javascript:/tracker/srcset/scheme-relative/SVG data) plus the GUI ("Phishing test" and "tracking pixel" fixtures, screenshots 02, 03 and 09).

### 2.2 App CSP and Tauri capabilities (least privilege)
- `tauri.conf.json` CSP: `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`. No remote origins at all. `freezePrototype: true`, `withGlobalTauri: false`.
- Commands are declared in an **app manifest** (`build.rs`), so each needs an explicit `allow-*` permission. `capabilities/main.json` grants the `main` window only its 11 commands plus `core:event:allow-listen`/`allow-unlisten`. **No `core:default`, no plugins** (no fs, shell, http, opener, dialog).

### 2.3 Credential storage
- Passwords live in the Secret Service (GNOME Keyring) via `keyring` 3 (`sync-secret-service`, `crypto-rust`), service `dev.neutronmail.NeutronMail`, key `account-<id>`. They are never in SQLite or config. Checked: `grep testpass` on the db, WAL and the logs returned 0 matches.
- Fallback when no Secret Service: **memory only** for the session (never plaintext on disk), shown in the UI as "⚠ no keyring".
- An account is persisted only **after** a successful IMAP login. If storing the password fails, the account row is rolled back.
- Verified in the GUI: backend=Keyring, and after an app restart, background sync ran without a credential error.

### 2.4 TLS
- rustls 0.23 (ring) with certificates **always** validated. Roots are the Mozilla bundle (`webpki-roots`) plus an optional user-supplied private CA (adds trust, never disables it). There is no "accept invalid certificates" switch anywhere.
- STARTTLS is **mandatory** when selected (IMAP: `STARTTLS` must return OK; SMTP: `Tls::Required`). There is no plaintext fallback, and no plaintext mode exists.
- Tests: an untrusted certificate is rejected for IMAP and SMTP; both STARTTLS and implicit TLS work against the private CA.

### 2.5 Logging
- `tracing` only. Logs contain account ids, folder names, counts and error texts. They never contain passwords, message bodies or addresses. `tracing-log` is not enabled, so `log` records from async-imap/lettre (protocol traces) are never emitted.
- New tests assert that IMAP and SMTP authentication errors do not contain the password (`format!("{err} {err:?}")`).

### 2.6 Local data
- `cache.sqlite3` (+ `-wal`, `-shm`) are mode 0600; the app data dir is set to 0700 at startup.

## 3. Findings

| # | Severity | Finding | Status |
|---|---|---|---|
| F1 | Medium | The test Dovecot listened on `0.0.0.0` (test credentials exposed to the network). | **Fixed**: `listen = 127.0.0.1`. The SMTP test server also binds 127.0.0.1. |
| F2 | Medium | Pasting a CA PEM without a trailing newline (the UI trims input) failed. The error then **echoed the raw input bytes** into the UI. Found during GUI testing. | **Fixed** (`470e4bf`): the PEM is normalized (CRLF, trailing newline) and the error is concise; regression test added. |
| F3 | Low | Remote-image count missed scheme-relative and `srcset`-only images (the user was told fewer were blocked than really were; the images themselves were still blocked). | **Fixed**: counted on the sanitized output. |
| F4 | Low | `APPEND` always set `\Seen`. | **Fixed**: explicit `seen` parameter. |
| F5 | Low | mail-parser synthesized HTML from text parts, so text mail went through the HTML path unnecessarily. | **Fixed**: only real `text/html` parts are used. |
| F6 | Low | The app data dir was 0755 (WebKit storage beside the cache). | **Fixed**: 0700 at startup. |
| O1 | Medium | **Message cache is plaintext at rest** (raw RFC 822 in SQLite, 0600). Anyone with access to the user account or an unencrypted disk can read mail. | Open. Rely on full-disk encryption (the Fedora default with LUKS). Future: SQLCipher with a key in the keyring. |
| O2 | Low | App CSP allows `style-src 'unsafe-inline'`: the srcdoc iframe inherits the parent CSP, and mail HTML needs inline styles. Script execution is still limited to `'self'`. | Accepted for now. Future: serve message documents from a separate custom protocol with their own CSP. |
| O3 | Low | Trust stores differ: IMAP uses webpki-roots (Mozilla), while SMTP (lettre) also loads native certs. A corporate CA in `/etc/pki` works for SMTP but not IMAP, unless pasted as a private CA. | Open. Unify on `rustls-platform-verifier`. |
| O4 | Low | STARTTLS response-injection (data pipelined before the TLS handshake) was **not tested**. async-imap `into_inner()` is expected to drop the buffer, but this is unverified. | Open. Add a malicious-server test. |
| O5 | Low | With the memory fallback, passwords are lost on restart, and the UI has **no re-enter-password prompt** yet (the `set_password` command exists; sync shows a secret error). | Open (UX). |
| O6 | Info | Links in messages cannot be opened (the sandbox blocks navigation). This is a safe default but a functional gap. | Open. Future: opener plugin with a URL-confirm dialog that shows the real target. |
| O7 | Info | cargo-audit warnings RUSTSEC-2024-0370 and RUSTSEC-2024-0429 are transitive via GTK3. | Track Tauri/wry's migration to gtk4/gtk-rs ≥ 0.20. |
| O8 | Info | A failed save to Sent is only logged, not shown to the user. A failed server `\Seen` update is logged and reconciled on the next sync. | Open (UX). |

## 4. Not covered / limits of this audit
- No fuzzing of mail-parser/ammonia, and no third-party penetration test.
- WebKitGTK sandboxing and the bubblewrap/Flatpak setup were not evaluated.
- The Fedora 44 build was not run (the box is Debian 13).
- CSS-only phishing tricks (layout spoofing) are not prevented beyond what the CSP blocks.
