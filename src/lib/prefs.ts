// Small per-device preferences (localStorage; defaults when unavailable).

/** Seconds a sent message waits so it can be undone (0 = send at once). */
export const UNDO_SEND_CHOICES = [0, 5, 10, 20, 30];
export function undoSendSeconds(): number {
  try {
    const v = Number(localStorage.getItem("undoSendSeconds"));
    return localStorage.getItem("undoSendSeconds") !== null && UNDO_SEND_CHOICES.includes(v) ? v : 10;
  } catch { return 10; }
}
export function setUndoSendSeconds(v: number) {
  try { localStorage.setItem("undoSendSeconds", String(v)); } catch { /* ignore */ }
}

/** Text size of the open message (reading pane). */
export const MAIL_ZOOM_STEPS = [0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2];
export function mailZoom(): number {
  try {
    const v = Number(localStorage.getItem("mailZoom"));
    return MAIL_ZOOM_STEPS.includes(v) ? v : 1;
  } catch { return 1; }
}
/** The next step up (+1) or down (-1) from `z`; 0 resets to 100 %. */
export function stepZoom(z: number, dir: -1 | 0 | 1): number {
  if (dir === 0) return 1;
  const i = MAIL_ZOOM_STEPS.indexOf(z);
  const at = i < 0 ? MAIL_ZOOM_STEPS.indexOf(1) : i;
  return MAIL_ZOOM_STEPS[Math.min(MAIL_ZOOM_STEPS.length - 1, Math.max(0, at + dir))];
}
export function setMailZoom(v: number) {
  try { localStorage.setItem("mailZoom", String(v)); } catch { /* ignore */ }
}

/** Message list density. */
export function compactList(): boolean {
  try { return localStorage.getItem("listDensity") === "compact"; } catch { return false; }
}
export function setCompactList(on: boolean) {
  try { localStorage.setItem("listDensity", on ? "compact" : "comfortable"); } catch { /* ignore */ }
}

/** Ask GitHub once a day whether a newer release exists (default on). */
export function updateChecks(): boolean {
  try { return localStorage.getItem("updateChecks") !== "0"; } catch { return true; }
}
export function setUpdateChecks(on: boolean) {
  try { localStorage.setItem("updateChecks", on ? "1" : "0"); } catch { /* ignore */ }
}
/** The one command that downloads, verifies and installs the latest RPM. */
export const INSTALL_COMMAND = "curl -fsSL https://raw.githubusercontent.com/mkd47/neutron-mail/main/scripts/install.sh | bash";
