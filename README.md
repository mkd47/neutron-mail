# Neutron Mail

A free, open-source desktop mail client inspired by modern privacy-focused webmail layouts, built for **your own IMAP/SMTP server** and **Fedora 44 GNOME**.

- **Core:** Rust (tokio, async-imap, lettre, mail-parser/mail-builder, SQLite)
- **Shell:** Tauri v2 (WebKitGTK 4.1)
- **UI:** React or Svelte + TypeScript (original design)
- **GNOME integration:** Contacts & Calendar via evolution-data-server, GNOME Online Accounts, `mailto:` handler, notifications

> **Not affiliated with Proton AG.** Neutron Mail uses **no Proton code, name, logos, or assets**. "Proton" is a trademark of Proton AG; it is mentioned in the docs only to describe design inspiration and prior-art research.

## Status

Research and design stage — no application code yet.

- 📄 [Research report](docs/research-report.md): architecture, crates, GNOME integration, licensing, prototype plan
- 🎨 [UI concepts](concepts/README.md)

## Concepts

| Inbox (dark) | Inbox (light) |
|---|---|
| ![Inbox dark](concepts/01-inbox-dark.png) | ![Inbox light](concepts/02-inbox-light.png) |

| Compose | Search | Settings |
|---|---|---|
| ![Compose](concepts/03-compose.png) | ![Search](concepts/04-search.png) | ![Settings](concepts/05-settings.png) |

The mockups show the placeholder name "Aurora Mail", which was used before the name Neutron Mail was chosen.

## License

**To be decided.** No license has been chosen yet, so all rights are reserved for now. (If any GPLv3 code is ever incorporated, the project must be GPL-3.0-or-later. See the report.)
