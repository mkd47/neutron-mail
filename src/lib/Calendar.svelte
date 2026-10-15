<script lang="ts">
  // Calendar app view: the next 30 days from GNOME Calendar (EDS), read-only.
  import { onMount } from "svelte";
  import { calendarApi, errorText, type Agenda } from "./api";
  let agenda = $state<Agenda | null>(null);
  let error = $state("");
  onMount(async () => { try { agenda = await calendarApi.upcoming(30); } catch (e) { error = errorText(e); } });
  const days = $derived.by(() => {
    const m = new Map<string, Agenda["events"]>();
    for (const e of agenda?.events ?? []) {
      const k = new Date(e.start * 1000).toLocaleDateString([], { weekday: "long", month: "long", day: "numeric" });
      m.set(k, [...(m.get(k) ?? []), e]);
    }
    return [...m.entries()];
  });
  const time = (s: number) => new Date(s * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
</script>

<div class="cal">
  <h1>Calendar</h1>
  <p class="muted">Next 30 days from your GNOME calendars. Create and edit events in GNOME Calendar.</p>
  {#if error}<p class="err">{error}</p>
  {:else if agenda && !agenda.available}<p class="muted">GNOME calendars (evolution-data-server) aren't available{agenda.error ? `: ${agenda.error}` : ""}.</p>
  {:else if agenda && !days.length}<p class="muted">Nothing scheduled.</p>{/if}
  {#each days as [day, evs] (day)}
    <h3>{day}</h3>
    {#each evs as e (e.uid + e.start)}
      <div class="ev" style:border-left-color={e.color || "var(--accent)"}>
        <span class="t">{e.all_day ? "All day" : `${time(e.start)} – ${time(e.end)}`}</span>
        <b>{e.summary}</b>{#if e.location}<span class="muted"> · {e.location}</span>{/if}
        <span class="muted small">{e.calendar}</span>
      </div>
    {/each}
  {/each}
</div>

<style>
  .cal { padding: 20px 28px; overflow: auto; height: 100%; max-width: 900px; }
  h1 { margin: 0 0 4px; } h3 { margin: 18px 0 6px; font-size: 14px; color: var(--muted); }
  .ev { display: flex; gap: 12px; align-items: baseline; padding: 10px 12px; border-left: 4px solid; background: var(--panel-2); border-radius: 8px; margin: 4px 0; }
  .t { width: 130px; color: var(--muted); font-size: 13px; flex: none; } .small { font-size: 12px; margin-left: auto; } .err { color: #e5534b; }
</style>
