<script lang="ts">
  // Read-only agenda strip: the next events from GNOME Calendar (EDS).
  import Icon from "./Icon.svelte";
  import { agendaWhen, eventRange } from "./format";
  import { calendarApi, type Agenda } from "./api";

  let { refreshKey = 0 }: { refreshKey?: number } = $props();
  let agenda = $state<Agenda | null>(null);

  // One chip per event: a daily series shows only its next occurrence.
  const shown = $derived.by(() => {
    const seen = new Set<string>();
    return (agenda?.events ?? []).filter((e) => !seen.has(e.uid) && (seen.add(e.uid), true)).slice(0, 12);
  });
  async function load() {
    try { agenda = await calendarApi.upcoming(7); } catch { agenda = null; }
  }
  $effect(() => {
    void refreshKey;
    void load();
    const t = setInterval(load, 5 * 60_000);
    return () => clearInterval(t);
  });
</script>

{#if agenda?.available}
  <div class="agenda" role="region" aria-label="Upcoming events"
    onwheel={(ev) => { if (Math.abs(ev.deltaY) > Math.abs(ev.deltaX)) { ev.currentTarget.scrollLeft += ev.deltaY; ev.preventDefault(); } }}>
    <span class="head"><Icon name="calendar" size={16} /> Next 7 days</span>
    {#each shown as e (e.uid + e.start)}
      <span class="ev" title={`${e.summary}\n${eventRange(e.start, e.end, e.all_day)}${e.location ? `\n${e.location}` : ""}\n${e.calendar}`}>
        <span class="bar" style:background={e.color || "var(--accent)"}></span>
        <span class="when">{agendaWhen(e.start, e.all_day)}</span>
        <span class="what">{e.summary || "(untitled)"}</span>
      </span>
    {:else}
      <span class="muted none">Nothing scheduled</span>
    {/each}
  </div>
{/if}

<style>
  .agenda { display: flex; gap: 8px; align-items: center; padding: 8px 16px; border-bottom: 1px solid var(--border);
    overflow-x: auto; white-space: nowrap; background: var(--panel); font-size: 13px; scrollbar-width: none; }
  .agenda::-webkit-scrollbar { display: none; }
  .head { display: inline-flex; gap: 6px; align-items: center; color: var(--muted); font-weight: 600; margin-right: 4px; }
  .ev { display: inline-flex; gap: 6px; align-items: center; padding: 4px 10px 4px 6px; border-radius: 8px;
    background: var(--panel-2); border: 1px solid var(--border); max-width: 260px; }
  .bar { width: 3px; align-self: stretch; border-radius: 2px; }
  .when { color: var(--muted); }
  .what { overflow: hidden; text-overflow: ellipsis; font-weight: 500; }
  .none { font-size: 13px; }
</style>
