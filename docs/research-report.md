# Neutron Mail — Research Report

*Compiled 2026-10-14. Items marked ⚠ are unverified or uncertain.*

Neutron Mail is a free/open-source desktop mail client with a modern three-pane UI inspired by privacy-focused webmail, talking to **your own IMAP/SMTP server**, built with **Rust + Tauri v2** and a **React or Svelte** front-end, targeting **Fedora 44 GNOME**. Neutron Mail is not affiliated with Proton AG and uses no Proton code, name or assets.

## Summary

- Proton's web client is GPLv3 open source, but it is welded to Proton's API, SRP auth and OpenPGP key model. **Don't fork it**; build an original UI.
- Core: Rust on tokio. **async-imap** (IDLE, CONDSTORE), **lettre** (SMTP), **mail-parser/mail-builder**, **SQLite** cache with FTS5, **keyring** for secrets.
- GNOME: Contacts/Calendar via **evolution-data-server** (libebook/libecal; D-Bus is private), accounts via **GNOME Online Accounts**, `mailto:` via the Tauri deep-link + single-instance plugins.
- Biggest risks: IMAP sync correctness (no QRESYNC in async-imap), HTML email safety, WebKitGTK GPU quirks, the GNOME tray, and running tokio and glib together.

---

## 1. Proton WebClients (prior art, not reused)

- Repo: https://github.com/ProtonMail/WebClients. [LICENSE](https://raw.githubusercontent.com/ProtonMail/WebClients/main/LICENSE): **GPL-3.0-or-later**.
- A pnpm monorepo: React 18, TypeScript, Redux Toolkit, webpack. Apps include Mail (`applications/mail`), Calendar, Drive, Pass, VPN, Wallet, Lumo and Meet, plus 55+ `@proton/*` packages ([structure](https://protonmail-webclients.mintlify.app/project-structure)).
  - UI: `@proton/components`, `@proton/atoms`, `@proton/styles`, `@proton/icons`.
  - API and logic: `@proton/shared`.
  - Crypto: `@proton/crypto` (OpenPGP via CryptoProxy and workers), `@proton/srp`, `@proton/key-transparency`, `@proton/encrypted-search`.
  - Also payments and feature flags. Some packages come from a private registry.
- **Coupling:** Mail depends on Proton sessions, SRP, the API event loop and client-side decryption. There's no self-hostable backend.
- **In a Tauri webview:** possible in theory, but it means rewriting the data layer and stores and stripping crypto, billing and flags from a huge, fast-moving fork. **Not recommended.**
- **Trademark:** GPLv3 §7(e) leaves trademarks out of the license grant. Proton's terms reportedly require permission to use its marks ⚠. Hence the separate name, Neutron Mail, and original assets.
- **Proton Bridge** ([repo](https://github.com/ProtonMail/proton-bridge)) exposes *Proton* mailboxes as local IMAP/SMTP for paid plans ([why](https://proton.me/support/why-you-need-bridge)). It goes the wrong direction for this project.

## 2. Backend options considered

| Option | License | Verdict |
|---|---|---|
| Evolution Camel / libedataserver ([Camel](https://gnome.pages.gitlab.gnome.org/evolution-data-server/camel/)) | LGPL-2.0-only | Mature, has GIR bindings, but it's C/GObject. Good for GNOME data, not the mail engine. |
| Thunderbird comm-central ([docs](https://source-docs.thunderbird.net/en/latest/backend/email_protocols.html)) | MPL-2.0 | Tied to Gecko; not embeddable. |
| mailcore2 ([LICENSE](https://raw.githubusercontent.com/MailCore/mailcore2/master/LICENSE)) / [Mailspring-Sync](https://github.com/Foundry376/Mailspring-Sync) | BSD-3 / GPLv3 | C++; doesn't fit a Rust stack. |
| **Native Rust stack** (below) | MIT/Apache mostly | **Chosen.** |
| JMAP via [Stalwart](https://github.com/stalwartlabs/mail-server) + [jmap-client](https://github.com/stalwartlabs/jmap-client) | AGPL server | Optional, if the server supports JMAP. |

## 3. Existing projects

- **Geary** ([GitLab](https://gitlab.gnome.org/GNOME/geary)): Vala, own SQLite engine, Folks/EDS contacts. Its [Flatpak manifest](https://raw.githubusercontent.com/flathub/org.gnome.Geary/master/org.gnome.Geary.yaml) is a good permissions reference.
- **Mailspring** ([repo](https://github.com/foundry376/mailspring/)): Electron/React plus a C++ sync engine, GPLv3. Closest architecture.
- **Thunderbird:** the Supernova UI ([blog](https://blog.thunderbird.net/2023/07/our-fastest-most-beautiful-release-ever-thunderbird-115-supernova-is-here/)). Thundermail is a hosted service ([blog](https://blog.thunderbird.net/2026/04/thunderbird-pro-april-2026-update/)).
- **Evolution**, **Delta Chat** ([core](https://github.com/chatmail/core), Rust, async-imap), **meli** (melib), **himalaya** (pimalaya), **Postcard** ([repo](https://github.com/gxanshu/postcard)) ⚠.
- **Tangram** ([repo](https://github.com/sonnyp/Tangram)) is a web-app browser, not a mail client.
- No maintained Proton-UI-on-IMAP fork found ⚠.

## 4. Rust crates

| Need | Crate | License | Notes |
|---|---|---|---|
| IMAP | [async-imap](https://docs.rs/async-imap/latest/async_imap/struct.Session.html) 0.12 | MIT/Apache ⚠ | tokio; IDLE; CONDSTORE (`select_condstore`); QUOTA ([changelog](https://github.com/chatmail/async-imap/blob/main/CHANGELOG.md)). **No QRESYNC API.** |
| IMAP (avoid) | [imap](https://crates.io/crates/imap/3.0.0-alpha.15) | MIT/Apache | Blocking; v3 in alpha since 2021; seeking maintainers. |
| SMTP | [lettre](https://github.com/lettre/lettre) | MIT | Async, TLS, XOAUTH2. |
| MIME | [mail-parser](https://github.com/stalwartlabs/mail-parser) / mail-builder | Apache-2.0 ⚠ | Sanitize HTML (e.g. ammonia). |
| All-in-one | [melib](https://crates.io/crates/melib) | EUPL-1.2 OR GPL-3.0+ | IMAP/JMAP/SMTP/maildir/notmuch/SQLite cache. Forces copyleft. |
| All-in-one | [himalaya](https://github.com/pimalaya/himalaya) / email-lib | MIT/Apache | No built-in OAuth in v2; churny API. |
| Cache | rusqlite (+ SQLCipher) / sqlx | MIT/Apache | FTS5 search. |
| Secrets | keyring | MIT/Apache ⚠ | Secret Service / GNOME Keyring. |
| OAuth2 | oauth2 (PKCE) | MIT/Apache | Or tokens from GOA. |
| iCalendar | [icalendar](https://docs.rs/icalendar/latest/icalendar/) | MIT/Apache | RRULE via `rrule`. |
| iTIP | [mailrs-ical](https://crates.io/crates/mailrs-ical) | MIT/Apache | ⚠ maturity unvetted. |
| D-Bus | [zbus](https://docs.rs/zbus/latest/zbus/) (`tokio` feature) | MIT | Plus `zbus_xmlgen`. |

Reference designs: Delta Chat core (JSON-RPC over a Rust core) and aerc (Go; one worker per account).

## 5. Tauri v2 on Fedora 44 GNOME

- **Use v2.** It's on WebKitGTK 4.1 / libsoup3, which matches the GNOME Flatpak runtime ([blog](https://v2.tauri.app/blog/tauri-2-0-0-alpha-3/)).
- **Build packages** ([prerequisites](https://v2.tauri.app/start/prerequisites/)): `webkit2gtk4.1-devel openssl-devel curl wget file libappindicator-gtk3-devel librsvg2-devel libxdo-devel`, plus `c-development`.
- **Look and feel:** GTK3 windows (tao/wry), not libadwaita. Emulate the Adwaita style in CSS.
- **Tray:** needs the AppIndicator GNOME extension. There's a Wayland bug ([tauri#14234](https://github.com/tauri-apps/tauri/issues/14234)). Treat the tray as optional.
- **Graphics** ([Linux graphics](https://v2.tauri.app/develop/debug/linux-graphics/)): try `__NV_DISABLE_EXPLICIT_SYNC=1`, then `WEBKIT_DISABLE_DMABUF_RENDERER=1`, then `WEBKIT_DISABLE_COMPOSITING_MODE=1`.
- **Runtime:** Tauri runs tokio. Use `tauri::async_runtime::spawn`, not a second runtime, and push updates to the UI with events and Channels.

## 6. GNOME integration

### Contacts (evolution-data-server)
- **Public API:** [libebook](https://gnome.pages.gitlab.gnome.org/evolution-data-server/libebook/) / libebook-contacts (LGPL-2.0-only).
  - [`EBookClient`](https://gnome.pages.gitlab.gnome.org/evolution-data-server/libebook/class.BookClient.html): `connect`, `get_contacts`, `get_view`, `add_contact`, `modify_contact`.
  - [`EBookQuery`](https://gnome.pages.gitlab.gnome.org/evolution-data-server/libebook-contacts/struct.BookQuery.html) builds S-expressions (`any_field_contains`, `vcard_field_test`, `and`/`or`).
- **D-Bus** `org.gnome.evolution.dataserver.AddressBook*` / `Sources*` is **private**, and its bus names are versioned (e.g. `AddressBook9`).
- **GOA-synced contacts** (Google, M365, Exchange, CardDAV) show up as EDS sources. GNOME Contacts merges them through libfolks.
- **Rust options:**
  - glib + system GIR on a dedicated MainContext thread (preferred).
  - bindgen FFI (pattern: [alarma-eds](https://tangled.org/juanlu.space/alarma)).
  - zbus proxies (fragile).
  - No maintained `eds`/`libebook` crate found ⚠.

### Calendar
- [libecal 2.0](https://gnome.pages.gitlab.gnome.org/evolution-data-server/libecal/), `ECalClient`.
- Range queries with [`get_object_list_as_comps`](https://gnome.pages.gitlab.gnome.org/evolution-data-server/libecal/method.Client.get_object_list_as_comps.html) using `(occur-in-time-range? (make-time "…") (make-time "…"))`.
- Expand recurrences with [`generate_instances_for_object`](https://gnome.pages.gitlab.gnome.org/evolution-data-server/libecal/method.Client.generate_instances_for_object.html).
- Create events with [`create_object`](https://gnome.pages.gitlab.gnome.org/evolution-data-server/libecal/method.Client.create_object.html).

### Invites
- Generate `.ics` (icalendar or EDS) and attach it as `text/calendar; method=REQUEST` via mail-builder.
- Parse incoming `text/calendar` parts and reply with `METHOD:REPLY`.

### GNOME Online Accounts
- **Public D-Bus** ([overview](https://gnome.pages.gitlab.gnome.org/gnome-online-accounts/overview.html)).
- The [Mail](https://gnome.pages.gitlab.gnome.org/gnome-online-accounts/dbus-org.gnome.OnlineAccounts.Mail.html) interface gives IMAP/SMTP host, TLS and username, plus a `SmtpAuthXoauth2` flag.
- Credentials: call `EnsureCredentials`, then [`GetAccessToken`](https://gnome.pages.gitlab.gnome.org/gnome-online-accounts/dbus-org.gnome.OnlineAccounts.OAuth2Based.html) or [`GetPassword`](https://gnome.pages.gitlab.gnome.org/gnome-online-accounts/dbus-org.gnome.OnlineAccounts.PasswordBased.html) (ids `imap-password` / `smtp-password`).
- Never assume a fixed set of interfaces per account ([services](https://gnome.pages.gitlab.gnome.org/gnome-online-accounts/services.html)).
- Your own server stays a first-class manual account.

### mailto, notifications and extras
- **mailto:** `.desktop` `MimeType=x-scheme-handler/mailto;`, set as default with `xdg-mime default`.
  - [Tauri deep-link](https://v2.tauri.app/plugin/deep-linking/) with `schemes: ["mailto"]`, plus **single-instance (`deep-link` feature)** to forward argv to the running app.
  - `register_all` writes the `.desktop` file and runs xdg-mime ([source](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/deep-link/src/lib.rs)).
- **Notifications:** tauri-plugin-notification or notify-rust; the portal notification API is preferable under Flatpak ⚠.
- **Optional:**
  - [Shell search provider](https://developer.gnome.org/documentation/tutorials/search-provider.html) (`org.gnome.Shell.SearchProvider2`, exported via zbus).
  - Background portal for syncing while the window is closed.

## 7. Architecture

```
React/Svelte + TS UI (original Neutron Mail design)
   │ Tauri commands / events / Channels
Rust core (tokio)
 ├─ mail: AccountManager → per-account worker
 │        async-imap (IDLE on INBOX, CONDSTORE, UIDVALIDITY, polling fallback)
 │        lettre + mail-builder (outbox queue, APPEND to Sent)
 ├─ store: rusqlite (headers, flags, lazy bodies) + FTS5; SQLCipher optional
 ├─ render: mail-parser → sanitized HTML → sandboxed iframe, remote images blocked
 ├─ secrets: keyring (GNOME Keyring)
 ├─ goa: zbus → org.gnome.OnlineAccounts
 └─ eds: dedicated glib MainContext thread (libebook/libecal) ↔ tokio channels
        autocomplete cache in SQLite
```

**Flatpak `finish-args`** (modelled on Geary): `--share=network --socket=wayland --socket=fallback-x11 --device=dri --share=ipc --talk-name=org.freedesktop.secrets --talk-name=org.gnome.OnlineAccounts --talk-name=org.gnome.evolution.dataserver.*`. Use the **host** EDS. Don't bundle it ([why](https://github.com/flathub/org.gnome.Evolution/issues/91)).

## 8. Licensing

- Neutron Mail's own license is **to be decided**.
- Tauri and most crates are MIT/Apache, which can go into anything, including GPLv3.
- **MPL-2.0:** file-level copyleft; compatible with GPL.
- **LGPL** (EDS, GOA libraries): fine with dynamic linking.
- **melib** (EUPL/GPL) or **any Proton code or assets** would force the whole app to GPL-3.0-or-later. Neutron Mail currently uses neither.
- **Apache-2.0** is incompatible with GPLv2-only.
- Proton trademarks are not used.

## 9. Prototype plan

1. **Setup:** Fedora packages; `create-tauri-app` (v2, React or Svelte + TS). Check rendering under Wayland and X11.
2. **IMAP spike:** async-imap login, LIST, fetch headers, IDLE. Run CAPABILITY to check CONDSTORE, QRESYNC, MOVE and UIDPLUS.
3. **Cache:** SQLite schema keyed by UIDVALIDITY/UID; incremental CONDSTORE sync; FTS5 search.
4. **Tauri bridge:** commands `list_threads`, `get_message`, `set_flags`; events for new mail; keyring storage.
5. **UI:** three-pane layout, virtualized list, sandboxed viewer, keyboard shortcuts (see `/concepts`).
6. **Send:** composer → mail-builder → lettre; outbox; drafts via IMAP APPEND.
7. **GOA:** discover Mail accounts; XOAUTH2/password credentials.
8. **Contacts:** EDS address books → EBookQuery autocomplete in the composer; add sender as contact.
9. **Calendar:** agenda strip for date ranges; create events; send `.ics` invites and handle replies.
10. **mailto:** deep-link + single-instance; test `xdg-open 'mailto:…'`.
11. **Desktop:** notifications, Background portal, optional tray and search provider.
12. **Packaging:** RPM via the Tauri bundler (depends on `webkit2gtk4.1`); Flatpak on `org.gnome.Platform` ([guide](https://v2.tauri.app/distribute/flatpak/)); AppImage last ([libwayland issue](https://github.com/tauri-apps/tauri/issues/11988)).
13. **Hardening:** strict Content Security Policy, at-rest encryption (SQLCipher or LUKS), tests for UIDVALIDITY resets, large mailboxes and offline use.

## 10. Risks

| Risk | Mitigation |
|---|---|
| IMAP sync bugs; no QRESYNC in async-imap | Raw command or UID diffing; extensive tests |
| Unsafe HTML email | Sanitizer + sandboxed iframe + CSP; block remote content |
| WebKitGTK GPU and Wayland issues | Documented env-var workarounds; test on NVIDIA |
| No tray on stock GNOME | Rely on notifications and the Background portal |
| Not a native libadwaita look | CSS that follows the Adwaita style |
| tokio and glib event loops together | Dedicated glib thread with channels |
| EDS private D-Bus changes | Use libebook/libecal, not raw D-Bus |
| Flatpak permissions and host EDS | Geary-style `talk-name`s; no bundled EDS |
| GOA provider differences | Check interfaces per account; follow `EnsureCredentials` |
| Licensing / trademark | No Proton code or assets; pick a license before the first release |
