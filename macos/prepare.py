#!/usr/bin/env python3
"""Prepare an isolated macOS build; never modify the Linux checkout.

The overlay is deliberately additive in Git. Source edits happen only in a
new directory outside the checkout. Guarded replacements fail when upstream
changes require review rather than silently producing a partial port.
"""

import argparse
import hashlib
import json
import shutil
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OVERLAY = ROOT / "macos"
IGNORED = {".git", "node_modules", "target", "dist", "__pycache__"}


def fingerprints(root):
    result = {}
    for path in root.rglob("*"):
        relative = path.relative_to(root)
        if any(part in IGNORED for part in relative.parts) or path.is_symlink():
            continue
        if path.is_file():
            result[str(relative)] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result


def replace(root, name, old, new):
    path = root / name
    text = path.read_text()
    if text.count(old) != 1:
        raise RuntimeError(f"macOS overlay needs review: {name}: expected one {old!r}")
    path.write_text(text.replace(old, new, 1))


def prepare(destination):
    destination = destination.resolve()
    if destination == ROOT or ROOT in destination.parents or destination in ROOT.parents:
        raise ValueError("destination must be outside and separate from the source checkout")
    if destination.exists():
        raise ValueError("destination must not exist; use a fresh directory")
    before = fingerprints(ROOT)
    shutil.copytree(ROOT, destination, ignore=shutil.ignore_patterns(*IGNORED, "macos"))
    core = "crates/neutron-core/Cargo.toml"
    replace(destination, core, 'features = ["sync-secret-service", "crypto-rust"]',
            'features = ["apple-native"]')
    # Bundle crypto for both architectures; do not depend on Homebrew paths.
    replace(destination, core, 'features = ["bundled-sqlcipher"]',
            'features = ["bundled-sqlcipher-vendored-openssl"]')
    desktop = destination / "src-tauri"
    manifest = desktop / "Cargo.toml"
    manifest.write_text(manifest.read_text() + '\n[target.\'cfg(target_os = "macos")\'.dependencies]\n'
                        'tauri-plugin-notification = "=2.4.0"\n')
    for name in ("contacts.rs", "native.rs"):
        shutil.copyfile(OVERLAY / name, desktop / "src" / name)
    shutil.copyfile(OVERLAY / "keychain.rs",
                    destination / "crates/neutron-core/tests/macos_keychain.rs")
    replace(destination, "src-tauri/src/lib.rs", "mod contacts;", "mod contacts;\nmod native;")
    replace(destination, "src-tauri/src/lib.rs", ".plugin(tauri_plugin_dialog::init())",
            ".plugin(tauri_plugin_dialog::init())\n        .plugin(tauri_plugin_notification::init())")
    replace(destination, "src-tauri/src/watcher.rs",
            "neutron_core::gnome::notify(&summary, &body, *last_id)",
            "crate::native::notify(app, &summary, &body, *last_id)")
    replace(destination, "src-tauri/src/pim.rs",
            'neutron_core::gnome::notify(&format!("Reminder: {}", t.title), &body, 0)',
            'crate::native::notify(&app, &format!("Reminder: {}", t.title), &body, 0)')
    replace(destination, "src-tauri/src/commands.rs",
            'neutron_core::gnome::launcher_badge("Neutron Mail.desktop", count)',
            'crate::native::launcher_badge(&app, count)')

    # Keep email invitation parsing/replies shared; omit all system calendar I/O.
    calendar = desktop / "src/calendar.rs"
    original = calendar.read_text()
    start = original.index("fn load_invite(")
    calendar.write_text('''//! macOS preview: email invitation replies only; no system calendar access.
use neutron_core::gnome::CalendarEvent;
use neutron_core::ical::{self, InviteView};
use neutron_core::{imap, parse, smtp, Error};
use serde::Serialize;
use tauri::State;
use crate::{commands::Res, AppState};

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Default)]
pub struct Calendar {
    _private: (),
}

#[derive(Serialize)]
pub struct Agenda {
    available: bool,
    error: Option<String>,
    events: Vec<CalendarEvent>,
}

#[tauri::command]
pub async fn upcoming_events(state: State<'_, AppState>, days: u32) -> Res<Agenda> {
    let _ = (state, days);
    Ok(Agenda { available: false, error: None, events: vec![] })
}

''' + original[start:])
    text = calendar.read_text()
    block_start = text.index("    let mut in_calendar = view.in_calendar;")
    block_end = text.index("    state\n        .store\n        .set_invite_response", block_start)
    calendar.write_text(text[:block_start] + "    let in_calendar = false;\n    let calendar_error = None;\n" + text[block_end:])
    replace(destination, "src-tauri/src/calendar.rs",
            "    let ps = ical::valid_partstat(&partstat)?;",
            '    if add_to_calendar {\n        return Err(Error::Invalid("System calendar integration is disabled in this Mac preview".into()));\n    }\n    let ps = ical::valid_partstat(&partstat)?;')

    # Remove system-calendar controls only from the generated Mac frontend.
    replace(destination, "src/App.svelte", '  import Calendar from "./lib/Calendar.svelte";\n', "")
    replace(destination, "src/App.svelte", '"mail" | "calendar" | "notes" | "todo"',
            '"mail" | "notes" | "todo"')
    replace(destination, "src/App.svelte", '["calendar", "calendar", "Calendar"], ', "")
    replace(destination, "src/App.svelte", '  {:else if appView === "calendar"}\n    <Calendar />\n', "")
    replace(destination, "src/lib/InviteCard.svelte", "let addToCalendar = $state(true);",
            "const addToCalendar = false;")
    replace(destination, "src/lib/InviteCard.svelte",
            '        <label class="small muted"><input type="checkbox" bind:checked={addToCalendar} /> Add to GNOME Calendar</label>\n', "")
    # The removed checkbox leaves two component-local CSS rules unused.
    replace(destination, "src/lib/InviteCard.svelte", '  label { display: inline-flex; gap: 6px; align-items: center; }\n', "")
    replace(destination, "src/lib/InviteCard.svelte", '  label input { margin: 0; }\n', "")

    config_path = desktop / "tauri.conf.json"
    config = json.loads(config_path.read_text())
    config["bundle"]["targets"] = ["app", "dmg"]
    config["bundle"].pop("linux", None)
    config["bundle"]["icon"] = ["icons/icon.icns", "icons/icon.png"]
    config["bundle"]["macOS"] = {"minimumSystemVersion": "13.0", "signingIdentity": "-"}
    config["bundle"]["shortDescription"] = "Neutron Mail macOS preview"
    config_path.write_text(json.dumps(config, indent=2) + "\n")
    # Intentionally no NSContacts/Calendars usage descriptions or entitlements.
    if before != fingerprints(ROOT):
        raise RuntimeError("source checkout changed while preparing the Mac build")
    (destination / "MACOS-SOURCE-MANIFEST.json").write_text(json.dumps(before, indent=2) + "\n")
    print(f"Prepared macOS staging source at {destination}; source checkout unchanged")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    prepare(parser.parse_args().destination)
