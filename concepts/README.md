# Neutron Mail — UI concepts

> These mockups use the placeholder name **"Aurora Mail"** from before the name **Neutron Mail** was chosen.

Original design inspired by modern three-pane mail UIs. No Proton name, logos, code or assets are used; Neutron Mail is not affiliated with Proton AG.

Stack context: Rust + Tauri v2, React or Svelte front-end, own IMAP/SMTP server, Fedora 44 GNOME.

## Design rationale

- **Three-pane layout** (folders · message list · viewer) matches desktop mail habits and maps cleanly to a virtualized list + sandboxed HTML viewer.
- **Purple accent on dark/light surfaces** gives a distinctive look without copying any brand marks.
- **GNOME-friendly restraint**: rounded corners, clear hierarchy, settings that surface Contacts, Calendar and `mailto:` handler options.
- These are direction-setting concept images, not pixel-perfect production UI.

## Files

| File | View |
|------|------|
| `01-inbox-dark.png` | Main inbox, dark theme |
| `02-inbox-light.png` | Main inbox, light theme |
| `03-compose.png` | Compose: recipient chips, rich text, attachments, signature |
| `04-search.png` | Full-text search with keyword highlighting |
| `05-settings.png` | Accounts, IMAP/SMTP, signature, GNOME integration toggles |
| `06-notes-dark.png` | Notes: folders, tags, rich-text editor with checklist, linked email |
| `08-mail-to-task-light.png` | Mail view with "Add to To-do" popover and the Today/Notes side panel |

The To-do concept (`07`) was lost; the To-do view was built from its written description instead.
