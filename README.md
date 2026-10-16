# Neutron Mail

A free, open-source desktop mail client inspired by modern privacy-focused webmail layouts, built for **your own IMAP/SMTP server** and **Fedora 44 GNOME**.

- **Core:** Rust (tokio, async-imap, lettre, mail-parser, ammonia, rustls, SQLite FTS5, keyring)
- **Shell:** Tauri v2 (WebKitGTK 4.1)
- **UI:** Svelte 5 + TypeScript + Vite (original design)
- **GNOME integration:** Contacts & Calendar via evolution-data-server, GNOME Online Accounts, `mailto:` handler, notifications

> **Not affiliated with Proton AG.** Neutron Mail uses **no Proton code, name, logos, or assets**. "Proton" is a trademark of Proton AG; it is mentioned in the docs only to describe design inspiration and prior-art research.

## Status

**Working prototype (v0.2.0).** Verified in the real GUI against a local Dovecot + SMTP test server:
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
- **Fast incremental sync:** UIDNEXT/EXISTS bookkeeping and CONDSTORE (`CHANGEDSINCE`) where the server has it,
  with a full-diff fallback; expunges handled; no cap on the number of messages (paged download and list);
  every IMAP command has a timeout. 5,000 messages: first sync ≈1.3 s, a no-change resync ≈10 ms (local Dovecot, debug build).
- **Unread badge** (window title, and the dock icon where the dock supports the Unity LauncherEntry API) and
  **desktop notifications** for new mail in the inbox while the window is in the background.
- **Provider tiles** for 263.net, Alimail (enterprise and personal), Google (Gmail) and Outlook (Outlook.com /
  Microsoft 365), plus Other/manual: picking one fills in and locks the server settings (unlock with "Advanced"),
  asks only for the address and the right kind of password, with provider-specific hints and links. The provider
  is detected from the address; unknown domains are looked up via Mozilla autoconfig (HTTPS only).
  **Sign in with Google / Microsoft** (OAuth2 + PKCE, XOAUTH2) when the build has OAuth client IDs (see "OAuth setup").
- **Contact autocomplete** in To/Cc/Bcc from your GNOME address books (evolution-data-server over D-Bus), merged with addresses from your mail; works without EDS too.

- **GNOME Calendar** (evolution-data-server over D-Bus): a read-only **agenda strip** with the next 7 days from all
  enabled calendars (recurring events expanded), and an **invitation card** for `text/calendar` invites with
  **Accept / Maybe / Decline**: sends an iMIP `REPLY` to the organizer (copy saved to Sent) and, optionally, adds or
  updates the event in your Personal calendar.

- **Folders:** create, rename and delete folders (delete asks first and moves the messages to Trash or
  deletes them), nested subfolders using the server's hierarchy delimiter, drag a folder onto another to
  reparent it (or onto "top level"), subscribe/unsubscribe with a "show unsubscribed" toggle. Special-use folders
  (RFC 6154: Inbox, Sent, Drafts, Trash, Junk, Archive) are protected. Names in any script work (modified UTF-7,
  or `UTF8=ACCEPT` where the server offers it). Right-click a folder for the menu.
- **Moving and copying mail:** drag one or many messages onto a folder (the target is highlighted; hold **Ctrl**
  to copy), Ctrl/Shift-click for multi-select, **Move to… / Copy to…** pickers (**V** / **Shift+V**), and **Undo**
  (toast button or **Ctrl+Z**). Uses IMAP `MOVE`, falling back to `COPY` + `\Deleted` + `UID EXPUNGE` (requires UIDPLUS; unsupported servers are rejected safely); the local cache is
  updated at once.
- **Rules** (filter icon in the sidebar): ordered rules with conditions on From/To/Cc/Subject/Body/any header/size/
  attachments/account (contains, equals, starts with, regex; all or any) and actions move, copy, mark read, star,
  label (IMAP keyword), delete, forward (confirmed in the UI) and stop. They run on new Inbox mail during sync,
  or on any folder with **Run now**; **Test against message** shows which conditions match and what would happen.
  Rules marked **Run on server** are exported as a **Sieve** script (preview shown) and uploaded and activated over
  **ManageSieve** (RFC 5804), so the server files mail even when the app is closed; anything Sieve can't express stays local.
  Guards: no forwarding of automated/forwarded mail or your own mail, `Auto-Submitted` on forwards, 20 forwards/hour,
  each message processed once, linear-time regex with size limits. Rules are stored in the encrypted cache.

- **Notes and To-do** (app rail on the left: Mail · Calendar · Notes · To-do). **Notes**: folders, tags (with colors),
  search, pinning, and a rich-text editor (headings, bold/italic/underline/strike, lists, **checklists**, quotes, links);
  everything is sanitized on paste and again in the Rust core. **Save to Notes** turns an email into a note linked to it
  (**Open email** jumps back). **To-do**: smart lists Today (count badge), Upcoming, Flagged emails (starred mail and tasks
  created from email) and Completed; your own lists ("+ New list"); quick add with **Ctrl+Enter** (`!high` / `!med` / `!low`);
  due dates, priority dots, subtasks, notes, and **reminders** as desktop notifications. **Add to To-do** in the reading
  pane pre-fills the task from the subject and keeps a link to the email. A collapsible **Today / Notes panel** sits beside
  your mail (panel icon at the bottom of the sidebar). Stored locally in the encrypted cache, with UIDs and sync columns
  ready for future IMAP-notes / CalDAV VTODO sync (not implemented yet).

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

| Invitation card (iMIP) | Accepted: reply sent, added to GNOME Calendar | Agenda strip (EDS) |
|---|---|---|
| ![](docs/screenshots/invite-card.png) | ![](docs/screenshots/invite-accepted.png) | ![](docs/screenshots/agenda-strip.png) |

| Dark mode: invitation + agenda | Dark mode: 2,800-message folder | New-mail notification + unread count in the title |
|---|---|---|
| ![](docs/screenshots/dark-invite-agenda.png) | ![](docs/screenshots/dark-large-folder.png) | ![](docs/screenshots/notification-and-badge.png) |

| Provider tiles: 263.net (authorization-code hint) | Gmail detected from the address (app password) | Outlook: OAuth only, consumer SMTP host |
|---|---|---|
| ![](docs/screenshots/provider-tile-263.png) | ![](docs/screenshots/provider-detect-gmail.png) | ![](docs/screenshots/provider-detect-hotmail.png) |

| Alimail enterprise | Unknown domain via autoconfig | Other/manual against the local test server |
|---|---|---|
| ![](docs/screenshots/provider-tile-alimail.png) | ![](docs/screenshots/provider-autoconfig-fastmail.png) | ![](docs/screenshots/provider-other-filled.png) |

| New folder (any script) | Nested folders | Drag & drop with drop-target highlight |
|---|---|---|
| ![](docs/screenshots/folder-create.png) | ![](docs/screenshots/folders-nested.png) | ![](docs/screenshots/drag-drop-target.png) |

| Multi-select (Shift-click) | Ctrl+drag copies | Moved, with Undo |
|---|---|---|
| ![](docs/screenshots/multi-select.png) | ![](docs/screenshots/drag-copy-ctrl.png) | ![](docs/screenshots/move-undo.png) |

| Rules editor + test against a message | Sieve preview | Server rule applied to incoming mail (starred, filed by Sieve) |
|---|---|---|
| ![](docs/screenshots/rules-editor-test.png) | ![](docs/screenshots/rules-sieve-preview.png) | ![](docs/screenshots/rule-applied-incoming.png) |

| Notes: checklist, tags, linked email | To-do: Today, priorities, linked email | Add to To-do from an email (light) |
|---|---|---|
| ![](docs/screenshots/notes.png) | ![](docs/screenshots/todo.png) | ![](docs/screenshots/mail-to-task-light.png) |

| Account settings (login re-verified) | Signature, sync window, trusted senders | Signature in a reply |
|---|---|---|
| ![](docs/screenshots/settings-account.png) | ![](docs/screenshots/settings-signature-images-sync.png) | ![](docs/screenshots/settings-signature-reply.png) |

More: [delete folder confirmation](docs/screenshots/folder-delete-confirm.png), [folder reparented by drag](docs/screenshots/folder-reparent.png),
[Copy to… picker](docs/screenshots/copy-to-picker.png), [forward confirmation](docs/screenshots/rules-forward-confirm.png),
[Sieve script active](docs/screenshots/rules-sieve-active.png), [remove account confirmation](docs/screenshots/settings-remove-confirm.png), [inline reply](docs/screenshots/inline-reply.png), [inline reply all](docs/screenshots/inline-reply-all.png),
[compose autocomplete](docs/screenshots/autocomplete-compose.png), [attachment picker](docs/screenshots/attachment-picker.png),
[save](docs/screenshots/attachment-save.png) / [open](docs/screenshots/attachment-open.png) an attachment,
[link opened in the browser](docs/screenshots/linkedin-link-open.png), [redirect unwrapped](docs/screenshots/linkedin-redirect-unwrapped.png),
[remote images blocked](docs/screenshots/html-rendering-blocked.png), [Sent-copy failure notice](docs/screenshots/sent-copy-failure.png), [Today / Notes panel beside the mail](docs/screenshots/mail-today-panel.png).

### Shared UI package

The theme (palette, dark/light, base controls), the left app rail and the icon set come from
[**@neutron/ui**](https://github.com/mkd47/neutron-ui), shared with the other Neutron apps. It is a
GitHub dependency pinned to a commit in `package.json` (`github:mkd47/neutron-ui#<sha>`), and `package-lock.json`
records its integrity hash, so builds are reproducible. `npm ci` fetches it as an HTTPS tarball (no git or SSH
needed; checked in a clean `fedora:44` container). To update it, push to neutron-ui and bump the commit with
`npm install github:mkd47/neutron-ui#<new-sha>`. Mail-only styles stay in `src/app.css`.

### Why Svelte (not React)
Svelte compiles to plain DOM code with no runtime: an 82 KB JS bundle (30 KB gzipped) and two
runtime npm dependencies (`@tauri-apps/api`, `@tauri-apps/plugin-opener`). That keeps the WebView small and the supply-chain
audit surface minimal, which matters for an app that handles mail.

## Provider profiles

Built into `crates/neutron-core/src/providers.rs`; every value is from the provider's own documentation.

| Provider | IMAP | SMTP | Login | Sources |
|---|---|---|---|---|
| **263.net** (企业邮箱) | `imap.263.net:993` SSL | `smtp.263.net:465` SSL | Mailbox password, or a **client authorization code** (客户端授权码) once the company enables them: webmail → Settings → 账号安全 → 客户端授权码 → 新增授权码 (shown once). Overseas hosts `imapw/smtpw.263.net` exist (use Advanced). | [client settings](https://www.263.net/success/mail/client/20160603/970.html), [webmail manual](https://www.263.net/success/mail/pcweb/20231124/1908.html) |
| **Alimail enterprise** (阿里邮箱企业版) | `imap.qiye.aliyun.com:993` SSL | `smtp.qiye.aliyun.com:465` SSL | Third-party clients are **off by default** for new purchases; the admin must allow them ([doc](https://help.aliyun.com/zh/document_detail/606337.html)). If the **三方客户端安全密码** is enabled, generate one under Settings → 账户与安全 → 账户安全 ([doc](https://help.aliyun.com/zh/document_detail/444269.html)). HK hosts `imaphk/smtphk.qiye.aliyun.com`. | [server settings](https://help.aliyun.com/zh/document_detail/36576.html) |
| **Alimail personal** (`@aliyun.com`) | `imap.aliyun.com:993` SSL | `smtp.aliyun.com:465` SSL | Mailbox password | [server settings](https://help.aliyun.com/zh/document_detail/465307.html) |
| **Google (Gmail)** | `imap.gmail.com:993` SSL | `smtp.gmail.com:465` SSL (587 STARTTLS) | **Sign in with Google** (OAuth2, scope `https://mail.google.com/`), or an [app password](https://myaccount.google.com/apppasswords) (needs 2-Step Verification). The account password is not accepted. | [IMAP/SMTP](https://developers.google.com/workspace/gmail/imap/imap-smtp), [XOAUTH2](https://developers.google.com/workspace/gmail/imap/xoauth2-protocol), [client setup](https://support.google.com/mail/answer/7126229) |
| **Outlook** (Outlook.com / Microsoft 365) | `outlook.office365.com:993` SSL | `smtp.office365.com:587` STARTTLS (Outlook.com addresses: `smtp-mail.outlook.com:587`) | **OAuth2 only** (no passwords/app passwords). Outlook.com: turn on Settings → Mail → Forwarding and IMAP first. M365 admins may need to allow IMAP / SMTP AUTH. | [Outlook.com](https://support.microsoft.com/en-us/office/pop-imap-and-smtp-settings-for-outlook-com-d088b986-291d-42b8-9564-9c414e2aa040), [Exchange Online](https://learn.microsoft.com/en-us/exchange/clients-and-mobile-in-exchange-online/pop3-and-imap4/pop3-and-imap4), [OAuth for IMAP/SMTP](https://learn.microsoft.com/en-us/exchange/client-developer/legacy-protocols/how-to-authenticate-an-imap-pop-smtp-application-by-using-oauth) |

Auto-detection by address: `gmail.com`, `googlemail.com` → Google; `outlook.com`, `hotmail.com`, `live.com`, `msn.com`
(+ `.de`) → Outlook; `aliyun.com` → Alimail personal; `263.net` → 263. Company domains hosted at 263/Alimail/Google/M365
can't be told apart by name: pick the tile. For other domains, Neutron Mail asks (HTTPS only, 8 s timeout, 64 KB cap)
`https://autoconfig.<domain>/mail/config-v1.1.xml`, then Mozilla's ISPDB `https://autoconfig.thunderbird.net/v1.1/<domain>`;
only SSL/TLS or STARTTLS servers are accepted, and the result is shown for review. Try it from the shell:
`cargo run -p neutron-core --example autoconfig -- you@example.org`.

### OAuth setup

Neutron Mail ships **no** OAuth client IDs. Register your own apps and pass the IDs at build time or run time
(run-time variables win):

```sh
export NEUTRON_GOOGLE_CLIENT_ID=1234-abc.apps.googleusercontent.com
export NEUTRON_GOOGLE_CLIENT_SECRET=GOCSPX-...      # Google "Desktop app" clients issue one; it is not confidential
export NEUTRON_MICROSOFT_CLIENT_ID=00000000-0000-0000-0000-000000000000
export NEUTRON_MICROSOFT_TENANT=common              # optional: common | consumers | organizations | <tenant id>
cargo tauri build      # baked in via option_env!, or set them when launching the app
```

- **Google:** Google Cloud console → enable the Gmail API → OAuth consent screen (add scope `https://mail.google.com/`,
  add yourself as a test user while unverified) → Credentials → *OAuth client ID* → **Desktop app**. Loopback
  redirects (`http://127.0.0.1:<port>/`) are allowed for desktop clients automatically. Publishing to everyone needs
  Google's restricted-scope verification.
- **Microsoft:** Entra admin center → App registrations → New → "Accounts in any organizational directory and
  personal Microsoft accounts" → Authentication → *Mobile and desktop applications* platform with redirect URI
  `http://127.0.0.1` (the app uses `http://127.0.0.1:<random port>/`; Entra ignores the port for loopback URIs. If
  the portal rejects it, add it in the app's Manifest under `replyUrlsWithType`) and **Allow public client flows** = Yes. API permissions →
  Office 365 Exchange Online / delegated: `IMAP.AccessAsUser.All`, `SMTP.Send` (plus `offline_access`). No secret.

Flow: the app opens the system browser with PKCE (S256) and a random `state`, listens once on `127.0.0.1:<random port>`,
exchanges the code, verifies IMAP with `AUTHENTICATE XOAUTH2`, and stores `{access, refresh, expiry}` as JSON in GNOME
Keyring in place of a password. Access tokens are refreshed 2 minutes before expiry (SMTP uses lettre's XOAUTH2).
The flow is unit-tested against a mock token endpoint; it has **not** been run against real Google/Microsoft accounts.

## Build & run on Fedora 44

> **Checked in a `fedora:44` container** (podman, 2026-10-15, Fedora 44 x86_64): the commands below
> ran as written and `cargo tauri build --bundles rpm` produced `Neutron Mail-0.1.0-1.x86_64.rpm`
> (10.3 MB; requires `libwebkit2gtk-4.1.so.0`, `libgtk-3.so.0`), which installs with `dnf` with no
> missing libraries (`ldd`). Times on the build box: dnf ≈ 2.5–3 min, rustup + `tauri-cli` ≈ 2.5 min,
> `npm ci` + release build ≈ 2 min. Findings: the first `tauri build` failed with
> `invalid category` (`"Email"` is not a Tauri category; fixed: `Productivity` plus a desktop template
> with `Categories=Network;Email;`); on the build box the Cisco openh264 repo
> (`ciscobinary.openh264.org`) timed out, so dnf was run with `--disablerepo=fedora-cisco-openh264`
> (openh264 is only a weak dependency). The GUI was not run on a real Fedora/GNOME desktop.
> Rust ≥ 1.88 is required (async-imap 0.12).

```bash
# 1. System dependencies
sudo dnf install -y webkit2gtk4.1-devel openssl-devel curl wget file \
  libappindicator-gtk3-devel librsvg2-devel libxdo-devel nodejs npm \
  gnome-keyring libsecret evolution-data-server
sudo dnf group install -y c-development

# 2. Rust (>= 1.88) via rustup, plus the Tauri CLI
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo install --locked tauri-cli@^2

# 3. Build and run
npm ci
cargo tauri dev            # development (hot-reload UI)
cargo tauri build --bundles rpm   # release build + .rpm in target/release/bundle/rpm/
sudo dnf install ./target/release/bundle/rpm/*.rpm
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
npm test                                      # frontend async/race regressions
npm run check && npm run build                  # svelte-check (warnings fail) + Vite build

# Integration tests against a local mail server (all bound to 127.0.0.1)
sudo dnf install -y dovecot dovecot-pigeonhole python3-aiosmtpd openssl
# Debian/Ubuntu: dovecot-imapd dovecot-lmtpd dovecot-sieve dovecot-managesieved python3-aiosmtpd
export NM_TEST_BASE=/tmp/neutron-testmail
testserver/start.sh       # private CA + leaf cert, Dovecot on 10993 (TLS)/10143 (STARTTLS), ManageSieve on 10419,
                          # LMTP on 10024 (runs Sieve), SMTP on 10587 (delivers through LMTP)
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

| Notes (dark) | Mail to task (light) |
|---|---|
| ![Notes](concepts/06-notes-dark.png) | ![Mail to task](concepts/08-mail-to-task-light.png) |

The mockups show the placeholder name "Aurora Mail", which was used before the name Neutron Mail was chosen.

## License

Neutron Mail is free software, licensed under the **GNU General Public License v3.0 or later** (GPL-3.0-or-later). See [LICENSE](LICENSE).
