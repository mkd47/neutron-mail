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
