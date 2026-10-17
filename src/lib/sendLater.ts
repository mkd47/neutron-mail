// "Send later" presets, in local time. Pure functions so they can be tested.

export interface LaterChoice { label: string; at: Date }

const at = (base: Date, days: number, hour: number) => {
  const d = new Date(base.getFullYear(), base.getMonth(), base.getDate() + days, hour, 0, 0, 0);
  return d;
};

/** Tomorrow morning/afternoon and next Monday morning (if that's not tomorrow). */
export function laterChoices(now = new Date()): LaterChoice[] {
  const fmt = (d: Date) => d.toLocaleString([], { weekday: "short", hour: "2-digit", minute: "2-digit" });
  const out: LaterChoice[] = [];
  if (now.getHours() < 16) {
    const evening = at(now, 0, 18);
    out.push({ label: `This evening · ${fmt(evening)}`, at: evening });
  }
  const morning = at(now, 1, 8), afternoon = at(now, 1, 13);
  out.push({ label: `Tomorrow morning · ${fmt(morning)}`, at: morning });
  out.push({ label: `Tomorrow afternoon · ${fmt(afternoon)}`, at: afternoon });
  const toMonday = ((8 - now.getDay()) % 7) || 7;
  if (toMonday > 1) {
    const monday = at(now, toMonday, 8);
    out.push({ label: `Monday morning · ${fmt(monday)}`, at: monday });
  }
  return out;
}

/** Value for <input type="datetime-local"> in local time. */
export function toLocalInput(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** Parse a datetime-local value (local time); null if invalid. */
export function fromLocalInput(s: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(s);
  if (!m) return null;
  const d = new Date(+m[1], +m[2] - 1, +m[3], +m[4], +m[5]);
  return isNaN(d.getTime()) ? null : d;
}

/** "Tue 17 Oct, 08:00" (or "Today, 18:00" / "Tomorrow, 08:00"). */
export function whenLabel(d: Date, now = new Date()): string {
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((day(d) - day(now)) / 86_400_000);
  if (diff === 0) return `Today, ${time}`;
  if (diff === 1) return `Tomorrow, ${time}`;
  return `${d.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" })}, ${time}`;
}
