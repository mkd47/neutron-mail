# Neutron Mail – Security & Dependency Audit (prototype v0.1.0 + v0.2 features)

*Date: 2026-10-14/15 (Asia/Shanghai); updated 2026-10-15 for links, rich HTML, reply/forward/drafts, attachments and contacts. Scope: `crates/neutron-core`, `src-tauri`, `src/` (Svelte UI), `testserver/`.
Performed on the build box (Debian 13, rustc 1.99, Node 20.19). Author: Kaan Durmus (with Grok Bot).*

Severity scale: **High** (exploitable by a remote sender/server with real impact) · **Medium** · **Low** · **Info**.

## 1. Tool results

| Tool | Result |
|---|---|
| `cargo audit` (RustSec DB, 1329 advisories, 587 crates) | **0 vulnerabilities**, 2 warnings (both transitive via Tauri's Linux GTK3 stack): RUSTSEC-2024-0370 `proc-macro-error` unmaintained (build-time proc macro); RUSTSEC-2024-0429 `glib 0.18.5` unsound `VariantStrIter` (API not used by Neutron; fix needs gtk-rs ≥ 0.20, which Tauri 2/wry don't use yet). The new crates (reqwest 0.12 + rustls/ring, zbus 5, mail-builder 1, url, tauri-plugin-opener 2, tauri-plugin-dialog 2, webkit2gtk 2.0.2) add no advisories. |
| `cargo deny check` (`deny.toml`) | **advisories ok, bans ok, licenses ok, sources ok** (3 informational "license not encountered" notes for allow-list entries no crate uses). The new crates needed no license exceptions. Only crates.io; no git deps; no wildcards. |
| `npm audit` | **0 vulnerabilities**. Runtime npm deps: `@tauri-apps/api` and `@tauri-apps/plugin-opener`. The dialog plugin has **no** JS package: dialogs are opened from Rust only (see 2.8). |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean (`unsafe_code = "forbid"` workspace-wide) · `cargo fmt --check` clean |
| `svelte-check --fail-on-warnings` | 0 errors, 0 warnings · `vite build` OK |
| Tests | **55 pass** with the live local server (`NEUTRON_IT=1`) and real evolution-data-server (`testserver/eds-test.sh`): 17 unit (attachments, links, remote images, vCard/sexp), 6 IMAP sync, 4 SMTP, 11 sanitizer, 9 store/search/TLS, 6 feature integration tests (remote-image fetch + SSRF, reply-all headers + Bcc envelope, forward attachments byte-for-byte, drafts save/resume/delete, hostile attachment names, address history), 2 EDS (live add + any-field search; graceful failure without a session bus). Without the server/EDS the network tests return early (counted as "passed" but *skipped*). |

## 2. Review by area

### 2.1 HTML rendering (remote sender → viewer) — defence in depth
1. **ammonia allow-list** (`sanitize.rs`): scripts, forms/inputs, iframes, objects, event handlers and `javascript:` URLs removed; link schemes limited to http/https/mailto; relative URLs denied; `rel="noopener noreferrer nofollow"` added. `<img src>` kept **only** for raster `data:image/{png,jpeg,gif,webp}` (no SVG, no `srcset`). Remote images are removed and counted ("N remote images blocked") unless the user loads them (2.7).
   **CSS (v0.2):** `<style>` blocks and `style`/`class`/`bgcolor`/`align`/… attributes are kept so newsletters render properly, but every declaration goes through `sanitize_css`: `expression()`, `-moz-binding`, `behavior`, `javascript:`, `@import` and `</style` breakouts are dropped; `url()` keeps only inline raster `data:` images, remote URLs become `none` (or the fetched data URL when the user loaded images). The iframe CSP (point 2) is the backstop if a CSS trick slipped through: no network fetch is possible from the message document.
   **cid: images** are resolved from the message's own MIME parts and inlined as `data:` URLs, raster types only (png/jpeg/gif/webp, no SVG).
2. **Per-message CSP** injected into the iframe document: `default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src 'none'; form-action 'none'; base-uri 'none'`, so even a sanitizer bypass could not fetch remote resources (tracking) or load fonts.
3. **Sandboxed iframe** `sandbox=""` (no scripts, opaque origin, no forms/popups/top navigation) with `srcdoc` and `referrerpolicy="no-referrer"`; it cannot reach the Tauri IPC bridge.
4. Search snippets use U+0002/U+0003 markers rendered as Svelte text nodes, never `{@html}` (the UI contains no `{@html}` at all).

Verified: 11 sanitizer tests (incl. safe CSS kept, dangerous CSS dropped, remote `url()` substitution, `</style>` breakout) (script/iframe/form/on*/javascript:/tracker/srcset/scheme-relative/SVG data) plus the GUI ("Phishing test" and "tracking pixel" fixtures, screenshots 02, 03 and 09).

### 2.2 App CSP and Tauri capabilities (least privilege)
- `tauri.conf.json` CSP: `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; frame-src 'self' http: https: mailto:; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`. `freezePrototype: true`, `withGlobalTauri: false`.
  - **Why `frame-src` lists http/https/mailto (v0.2):** without it, WebKit blocks a link click inside the message iframe *before* the navigation policy runs, so links could never reach the confirm dialog (found in GUI testing). Allowing these schemes in `frame-src` only lets the navigation *request* reach the Rust handler, which **cancels every navigation** that is not the app itself (2.9). Nothing is ever loaded into the frame; `connect-src`/`img-src` still allow no remote origin.
- Commands are declared in an **app manifest** (`build.rs`, 21 commands), so each needs an explicit `allow-*` permission. `capabilities/main.json` grants the `main` window those 21 commands, `core:event:allow-listen`/`allow-unlisten`, and **one plugin permission: `opener:allow-open-url` scoped to `http://*` and `https://*`**. No `core:default`; **no** fs, shell, http, dialog or `opener:allow-open-path` permission for the webview. Opening attachments and showing file dialogs happen in Rust commands (2.8), so a compromised UI cannot open arbitrary paths or show a picker to exfiltrate files.

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

### 2.7 Remote images (opt-in, fetched by the core)
- Blocked by default. "Load remote images" (per message) or "Always load from this sender" (per sender, stored in `image_senders`, revocable with "Stop loading …") makes the **Rust core** fetch them; the webview never touches the network, so the CSP stays `img-src data:`.
- `remote.rs` (reqwest 0.12 + rustls): http/https only; max 3 redirects; 5 s connect / 10 s total timeout; no cookies; generic User-Agent, no Referer.
- **SSRF guard:** a custom DNS resolver refuses loopback, private, link-local, CGNAT, multicast, unspecified, documentation and IPv6 ULA/link-local addresses (and IPv4-mapped forms); IP-literal URLs and every redirect target are checked too. A mail therefore cannot make the client probe the LAN or `localhost` services. (Debug builds honour `NEUTRON_ALLOW_PRIVATE_IMAGE_HOSTS=1` so the local test server can be used; release builds ignore it.)
- Limits: 5 MB per image, 25 MB and 40 images per message, 6 parallel fetches. The body is **magic-byte sniffed** (PNG/JPEG/GIF/WebP only), so an HTML or SVG "image" is rejected (fixture `fake.png`: rejected and reported as "could not be loaded").
- Privacy note (by design): loading remote images tells the sender's server your IP and that you opened the mail. That is why it is opt-in.

### 2.8 Attachments
- **Received:** file names are sanitized (`safe_filename`): basename only (`../../.bashrc` → `bashrc`), control and bidi-override characters removed (RLO trick `report\u202Etxt.exe` → `reporttxt.exe`), leading dots stripped, Windows reserved names prefixed, max 120 chars keeping the extension. `is_risky` flags executables/scripts by extension, MIME type and magic bytes (ELF, MZ, `#!`); risky files are marked in red and **Open** requires a second confirmation.
- **Save** uses the native GTK save dialog driven from Rust (`blocking_save_file`), prefilled with the safe name; the user chooses the location. **Open** writes a private copy (0600 in a fresh 0700 directory under the app cache) and hands it to the default application via the opener plugin from Rust. Copies are deleted on the next app start.
- **Outgoing:** the picker is also Rust-side (`blocking_pick_files`); the UI only receives **opaque tokens** + name/size, never paths, and can't attach a file the user did not pick. Limits: 25 MB per file and per message, 50 attachments; oversize files are rejected with a message. Messages are built with `mail-builder` (multipart/mixed); CR/LF in subject or addresses is rejected (header injection).
- **Forward** carries the original attachments (re-read from the cached raw message by index), byte-for-byte (integration test + GUI test compare bytes).

### 2.9 Links
- The iframe stays `sandbox=""` (no scripts). wry/WebKitGTK reports subframe navigations to Tauri's `on_navigation`, so a link click in a message is intercepted in Rust **and always cancelled**; only `about:blank`/`about:srcdoc` and the app origin are allowed (the dev server origin only in debug builds). The main window is created in `setup` so the handler is attached before any content loads.
- The link is analysed in Rust (`links.rs`) and sent to the UI: only **http, https and mailto** are accepted (other schemes produce a "Blocked link" notice). The confirm dialog shows the **real host** (IDN as punycode, so look-alike domains are visible), the cleaned URL, and the original URL. Hover shows the actual target in a status pill (WebKit `mouse-target-changed`), not the link text.
- Tracking cleanup: LinkedIn `/comm/` paths and tracking parameters, `linkedin.com/redir/redirect?url=` and similar `?url=`/`?u=` redirectors are unwrapped (the dialog says so); `utm_*`-style parameters are stripped. "Open original" is still available. `lnkd.in` short links can't be resolved offline and are shown as-is.
- mailto opens the in-app composer (to/cc/bcc/subject/body prefilled); nothing leaves the app until the user presses Send.

### 2.10 Sending, replies, drafts
- Reply uses Reply-To/From; Reply all adds To+Cc minus your own address (deduplicated). `In-Reply-To` = parent Message-ID, `References` = parent References + parent Message-ID; Forward sets only References. Verified on the server in the GUI.
- **Bcc** recipients go only into the SMTP envelope; the sent message has no Bcc header (verified on the server: `X-Test-Envelope-To` lists the Bcc address, headers don't). Drafts *do* keep a Bcc header so they can be resumed — they are stored in your own Drafts folder.
- Drafts are APPENDed with `\Draft \Seen`; saving again or sending replaces/deletes the old draft (UID EXPUNGE). A failure to delete is reported.
- **Sent copy:** if the copy to Sent fails, the UI shows "Sent, but the copy could not be saved to Sent: …" (GUI-verified by making the server's Sent folder read-only).

### 2.11 Contacts (evolution-data-server)
- Read via D-Bus (`zbus` 5, pure Rust, no libebook C bindings): `Sources5` to list enabled address books, `AddressBook10.OpenAddressBook` + `GetContactList` with an `x-evolution-any-field` "contains" query. The query string is escaped for EDS's S-expression syntax (`sexp_escape`, tested). Calls have a 4 s timeout; if EDS isn't running the composer silently falls back to addresses from your own mail cache ("Recent", ranked below contacts) and retries EDS after 60 s.
- Contact data is only read (the seeding example in `crates/neutron-core/examples/eds_seed.rs` is for tests). Suggestions are cached in memory for 30 s; nothing about contacts is logged except counts.

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
| O6 | Info | Links in messages could not be opened. | **Done (v0.2)**: intercepted navigation + confirm dialog + opener scoped to http/https (2.9). |
| O7 | Info | cargo-audit warnings RUSTSEC-2024-0370 and RUSTSEC-2024-0429 are transitive via GTK3. | Track Tauri/wry's migration to gtk4/gtk-rs ≥ 0.20. |
| O8 | Info | A failed save to Sent was only logged. | **Fixed (v0.2)**: shown to the user (2.10). A failed server `\Seen` update is still only logged and reconciled on the next sync. |
| F7 | Medium | (v0.2, found in GUI testing) Without `frame-src`, link clicks in messages were silently swallowed by the CSP. | **Fixed**: `frame-src 'self' http: https: mailto:` + Rust-side cancel of every navigation (2.2, 2.9). |
| F8 | Low | (v0.2) Loading remote images fetched each image twice (UI effect re-ran). Doubles the tracking signal. | **Fixed**: one fetch per load (checked in the image server log). |
| F9 | Low | (v0.2) Private copies of opened attachments were never deleted. | **Fixed**: cleared at the next app start (copies are 0600 in 0700 dirs meanwhile). |
| O9 | Low | (v0.2) Remote images, once loaded, reveal the user's IP and open time to the sender (inherent to the feature). No proxy. | Accepted; opt-in only. |
| O10 | Low | (v0.2) CSS is allowed in messages, so layout-spoofing phishing (fake UI drawn by CSS) is easier than with plain text. | Accepted; the real link target is always shown on hover and in the confirm dialog. |
| O11 | Info | (v0.2) Drafts stored on the IMAP server contain the Bcc header (needed to resume them). | Accepted; it is the user's own mailbox. |
| O12 | Info | (v0.2) `lnkd.in` and other opaque short links can't be unwrapped without contacting the shortener; they are shown as-is with their real host. | Accepted. |

## 4. Not covered / limits of this audit
- No fuzzing of mail-parser/ammonia, and no third-party penetration test.
- WebKitGTK sandboxing and the bubblewrap/Flatpak setup were not evaluated.
- The Fedora 44 build was not run (the box is Debian 13).
- CSS-only phishing tricks (layout spoofing) are not prevented beyond what the CSP blocks (see O10).
- The opener/xdg-open handoff was tested with Chrome and MuPDF in an isolated XDG session on the box, not on a real GNOME desktop.
