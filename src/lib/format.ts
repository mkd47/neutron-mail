export function shortDate(ts: number): string {
  if (!ts) return "";
  const d = new Date(ts * 1000);
  const now = new Date();
  if (d.toDateString() === now.toDateString())
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const y = new Date(now); y.setDate(now.getDate() - 1);
  if (d.toDateString() === y.toDateString()) return "Yesterday";
  if (now.getTime() - d.getTime() < 6 * 86400e3) return d.toLocaleDateString([], { weekday: "short" });
  if (d.getFullYear() !== now.getFullYear()) return d.toLocaleDateString([], { year: "numeric", month: "short", day: "numeric" });
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

export function initials(name: string, addr: string): string {
  const src = (name || addr || "?").trim();
  const parts = src.split(/[\s@._-]+/).filter(Boolean);
  return ((parts[0]?.[0] ?? "?") + (parts[1]?.[0] ?? "")).toUpperCase();
}

export function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** Split a search snippet with \u0002..\u0003 markers into text segments. */
export function snippetParts(s: string): { text: string; hit: boolean }[] {
  const out: { text: string; hit: boolean }[] = [];
  let hit = false, buf = "";
  for (const ch of s) {
    if (ch === "\u0002" || ch === "\u0003") {
      if (buf) out.push({ text: buf, hit });
      buf = ""; hit = ch === "\u0002";
    } else buf += ch;
  }
  if (buf) out.push({ text: buf, hit });
  return out;
}

/** "Tue 20 Oct, 14:00–15:00" (local time); all-day events show the date only. */
export function eventRange(start: number, end: number, allDay: boolean): string {
  const s = new Date(start * 1000), e = new Date(end * 1000);
  const day = (d: Date) => d.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" });
  const time = (d: Date) => d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (allDay) {
    const last = new Date(e.getTime() - 1000);
    return last.toDateString() === s.toDateString() ? `${day(s)} · all day` : `${day(s)} – ${day(last)}`;
  }
  return s.toDateString() === e.toDateString() ? `${day(s)}, ${time(s)}–${time(e)}` : `${day(s)} ${time(s)} – ${day(e)} ${time(e)}`;
}

/** Agenda label: "Today 14:00", "Tomorrow 09:30", "Thu 10:00". */
export function agendaWhen(start: number, allDay: boolean): string {
  const d = new Date(start * 1000), now = new Date();
  const tomorrow = new Date(now); tomorrow.setDate(now.getDate() + 1);
  const dayName = d.toDateString() === now.toDateString() ? "Today"
    : d.toDateString() === tomorrow.toDateString() ? "Tomorrow"
    : d.toLocaleDateString([], { weekday: "short", day: "numeric" });
  return allDay ? dayName : `${dayName} ${d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
}
