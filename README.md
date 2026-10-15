# Neutron Mail

A free, open-source desktop mail client inspired by modern privacy-focused webmail layouts, built for **your own IMAP/SMTP server** and **Fedora 44 GNOME**.

- **Core:** Rust (tokio, async-imap, lettre, mail-parser, ammonia, rustls, SQLite FTS5, keyring)
- **Shell:** Tauri v2 (WebKitGTK 4.1)
- **UI:** Svelte 5 + TypeScript + Vite (original design)
- **GNOME integration:** Contacts & Calendar via evolution-data-server, GNOME Online Accounts, `mailto:` handler, notifications

> **Not affiliated with Proton AG.** Neutron Mail uses **no Proton code, name, logos, or assets**. "Proton" is a trademark of Proton AG; it is mentioned in the docs only to describe design inspiration and prior-art research.

## Status

**Working prototype (v0.1.0 + v0.2 features).** Verified in the real GUI against a local Dovecot + SMTP test server:
add an account (IMAP/SMTP over TLS or STARTTLS, certificate always verified), folder list,
message list from a local SQLite cache, full-text search, IMAP IDLE push with a polling fallback, and:

- **Rich HTML mail:** safe CSS kept, inline `cid:` images, remote images blocked by default with
  "Load remote images" per message or "Always load from this sender" (fetched by the Rust core with an SSRF guard), and **View source**.
- **Links that work, safely:** clicks in the sandboxed message are intercepted; a dialog shows the real host and
  the cleaned URL (LinkedIn/redirect tracking removed) before opening it in your browser. Hovering shows the real target. `mailto:` opens the composer.
- **Rich-text (HTML) composing** for new mail, replies and forwards: bold/italic/lists/quotes/links, the original
  quoted with its formatting and inline images, sent as multipart/alternative (text generated from the HTML);
  everything pasted or quoted is sanitized, and you can switch any message to plain text.
- **Reply / Reply all / Forward inline** in the reading pane (original stays visible), plus a standalone Compose window:
  To/Cc/Bcc chips, correct `In-Reply-To`/`References`, quoted text, forwarded attachments, **drafts** (save, resume, send), and a visible warning if the copy to Sent fails.
- **Attachments:** attach via the native file picker; save (native save dialog) or open received files with the default app;
  hostile names neutralized, executables/scripts need confirmation, 25 MB limits.
- **Account settings** (gear icon in the sidebar): edit server/login settings (saved only after a successful
  login; leave the password empty to keep it), a rich-text **signature** (added above the quote in new mail,
  replies and forwards with the standard `-- ` delimiter, not re-added when resuming a draft), a **sync window**
  (all mail / 30 / 90 days / 1 / 2 years: older mail is neither downloaded nor kept in the local cache, and is
  never deleted on the server), the **trusted remote-image senders** list with Remove, and **Remove account**
  (deletes its keyring password and cached mail, with an in-app confirmation).
- **Contact autocomplete** in To/Cc/Bcc from your GNOME address books (evolution-data-server over D-Bus), merged with addresses from your mail; works without EDS too.

GNOME Calendar integration is still a stub.

- 🔐 [Security audit](docs/AUDIT.md): findings, severities, fixes and known gaps
- 📄 [Research report](docs/research-report.md): architecture, crates, GNOME integration, licensing
- 🎨 [UI concepts](concepts/README.md) · 📸 [Prototype screenshots](docs/screenshots/)

| Inline reply all + contact autocomplete | Forward with attachments | Link confirmation (LinkedIn tracking removed) |
|---|---|---|
| ![](docs/screenshots/autocomplete-inline.png) | ![](docs/screenshots/inline-forward-attachment.png) | ![](docs/screenshots/linkedin-link-confirm.png) |

| Rich-text reply (quoted HTML) | Received rich reply | Encrypted cache (sidebar) |
|---|---|---|
| ![](docs/screenshots/html-compose-reply.png) | ![](docs/screenshots/html-compose-received.png) | ![](docs/screenshots/cache-encrypted.png) |

| HTML + remote images loaded | View source | Dangerous attachment warning |
|---|---|---|
| ![](docs/screenshots/html-rendering-images.png) | ![](docs/screenshots/view-source.png) | ![](docs/screenshots/attachment-risky-warning.png) |

| Account settings (login re-verified) | Signature, sync window, trusted senders | Signature in a reply |
|---|---|---|
| ![](docs/screenshots/settings-account.png) | ![](docs/screenshots/settings-signature-images-sync.png) | ![](docs/screenshots/settings-signature-reply.png) |

More: [remove account confirmation](docs/screenshots/settings-remove-confirm.png), [inline reply](docs/screenshots/inline-reply.png), [inline reply all](docs/screenshots/inline-reply-all.png),
[compose autocomplete](docs/screenshots/autocomplete-compose.png), [attachment picker](docs/screenshots/attachment-picker.png),
[save](docs/screenshots/attachment-save.png) / [open](docs/screenshots/attachment-open.png) an attachment,
[link opened in the browser](docs/screenshots/linkedin-link-open.png), [redirect unwrapped](docs/screenshots/linkedin-redirect-unwrapped.png),
[remote images blocked](docs/screenshots/html-rendering-blocked.png), [Sent-copy failure notice](docs/screenshots/sent-copy-failure.png).

### Why Svelte (not React)
Svelte compiles to plain DOM code with no runtime: an 82 KB JS bundle (30 KB gzipped) and two
runtime npm dependencies (`@tauri-apps/api`, `@tauri-apps/plugin-opener`). That keeps the WebView small and the supply-chain
audit surface minimal, which matters for an app that handles mail.

## Build & run on Fedora 44

> The prototype was developed and tested on Debian 13 (the build box). The Fedora commands
> below follow Tauri v2's documented Fedora prerequisites but were **not run on Fedora**.

```bash
# 1. System dependencies
sudo dnf install -y webkit2gtk4.1-devel openssl-devel curl wget file \
  libappindicator-gtk3-devel librsvg2-devel libxdo-devel nodejs npm \
  gnome-keyring libsecret evolution-data-server
sudo dnf group install -y c-development

# 2. Rust (>= 1.85) via rustup, plus the Tauri CLI
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo install --locked tauri-cli@^2

# 3. Build and run
npm ci
cargo tauri dev            # development (hot-reload UI)
cargo tauri build          # release build + .rpm / .deb bundles in target/release/bundle/
```

Passwords are stored in the GNOME Keyring (Secret Service). The local mail cache is **encrypted at rest**
with SQLCipher; its random 256-bit key is kept in the keyring too, and a plaintext cache from an older
version is encrypted automatically on first start. Without a keyring, passwords **and** the cache are kept
**in memory only** for the session (nothing is written to disk) and the app shows a warning.
Set `NEUTRON_LOG=debug` for verbose logs (they never contain passwords or message bodies).

## Testing

```bash
cargo test --workspace                          # unit + offline tests (network tests auto-skip)
cargo clippy --workspace --all-targets -- -D warnings
npm run check && npm run build                  # svelte-check (warnings fail) + Vite build

# Integration tests against a local mail server (all bound to 127.0.0.1)
sudo dnf install -y dovecot python3-aiosmtpd openssl   # Debian: dovecot-imapd python3-aiosmtpd
export NM_TEST_BASE=/tmp/neutron-testmail
testserver/start.sh       # private CA + leaf cert, Dovecot on 10993 (TLS)/10143 (STARTTLS), SMTP on 10587
python3 testserver/seed.py   # HTML/CSS newsletter, LinkedIn-style links, reply-all thread, attachments, phishing fixtures
NEUTRON_IT=1 cargo test --workspace
testserver/eds-test.sh    # live evolution-data-server contact tests in a private D-Bus session
testserver/stop.sh
```

`start.sh` also serves test images on `http://127.0.0.1:10880`. Remote-image loading refuses
private/loopback hosts, so to see them in a **debug** build run the app with
`NEUTRON_ALLOW_PRIVATE_IMAGE_HOSTS=1` (ignored in release builds). To get test contacts into
your address book: `cargo run -p neutron-core --example eds_seed -- testserver/contacts.vcf`
(adds four example.test contacts to the local "Personal" book).

The test login is `kaan@localhost` / `testpass`. To use the test server from the GUI, paste
`$NM_TEST_BASE/certs/ca.pem` under **Advanced: trust a private CA**.

### Audit tooling
```bash
cargo install --locked cargo-audit cargo-deny
cargo audit && cargo deny check && npm audit
```

## Concepts

| Inbox (dark) | Inbox (light) |
|---|---|
| ![Inbox dark](concepts/01-inbox-dark.png) | ![Inbox light](concepts/02-inbox-light.png) |

| Compose | Search | Settings |
|---|---|---|
| ![Compose](concepts/03-compose.png) | ![Search](concepts/04-search.png) | ![Settings](concepts/05-settings.png) |

The mockups show the placeholder name "Aurora Mail", which was used before the name Neutron Mail was chosen.

## License

Neutron Mail is free software, licensed under the **GNU General Public License v3.0 or later** (GPL-3.0-or-later). See [LICENSE](LICENSE).
