<script lang="ts">
  // Collapsible Today / Notes side panel in the mail view.
  import { Icon } from "@neutron/ui";
  import { dayEnd, dueLabel, errorText, pimApi, type NoteSummary, type Task, type TaskList } from "./api";

  let { refreshKey = 0, onClose, onOpenTodo, onOpenNote, onNewNote, onStatus }: {
    refreshKey?: number; onClose: () => void; onOpenTodo: () => void; onOpenNote: (id: number | null) => void; onNewNote: () => void;
    onStatus: (t: string, warn?: boolean) => void;
  } = $props();
  let tasks = $state<Task[]>([]);
  let lists = $state<TaskList[]>([]);
  let notes = $state<NoteSummary[]>([]);
  async function load() {
    try {
      [tasks, lists, notes] = await Promise.all([pimApi.tasks({ kind: "today", day_end: dayEnd() }), pimApi.taskLists(),
        pimApi.notes({ folder_id: null, tag: null, query: "" })]);
    } catch (e) { onStatus(errorText(e), true); }
  }
  $effect(() => { void refreshKey; void load(); });
  async function toggle(t: Task) { try { await pimApi.setTaskCompleted(t.id, true); await load(); onStatus(`Done: ${t.title}`); } catch (e) { onStatus(errorText(e), true); } }
  const list = (id: number | null) => lists.find((l) => l.id === id);
  const when = (s: number) => {
    const d = new Date(s * 1000), t = new Date();
    return d.toDateString() === t.toDateString() ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : d.toLocaleDateString([], { month: "short", day: "numeric" });
  };
</script>

<aside class="tp" aria-label="Today and notes">
  <div class="head"><b>Today</b><span class="pill">{tasks.length} task{tasks.length === 1 ? "" : "s"}</span><span class="grow"></span>
    <button class="icon-btn" title="Open To-do" aria-label="Open To-do" onclick={onOpenTodo}><Icon name="chevron" size={16} /></button>
    <button class="icon-btn" title="Hide panel" aria-label="Hide panel" onclick={onClose}><Icon name="close" size={16} /></button></div>
  <ul class="tasks">
    {#each tasks as t (t.id)}
      <li>
        <button class="check" aria-label="Mark “{t.title}” as done" onclick={() => toggle(t)}></button>
        <div class="grow"><div class="tt">{#if t.email}<span class="env"><Icon name="mail" size={12} /></span> {/if}{t.title}</div>
          <div class="sub"><span style:color={list(t.list_id)?.color ?? "var(--muted)"}>{list(t.list_id)?.name ?? ""}</span><span class="muted">{dueLabel(t.due)}</span></div></div>
      </li>
    {:else}<li class="muted small">Nothing due today.</li>{/each}
  </ul>
  <div class="head"><b>Notes</b><span class="grow"></span><button class="icon-btn" title="New note" aria-label="New note" onclick={onNewNote}><Icon name="plus" size={16} /></button></div>
  <div class="notes">
    {#each notes.slice(0, 3) as n (n.id)}
      <button class="ncard" onclick={() => onOpenNote(n.id)}>
        <span class="ni"><Icon name="note" size={15} /></span>
        <span class="grow"><span class="nt">{n.title || "Untitled note"}</span><span class="ns">{n.snippet}</span></span>
        <span class="muted small">{when(n.modified)}</span>
      </button>
    {:else}<p class="muted small">No notes yet.</p>{/each}
  </div>
  <button class="link" onclick={() => onOpenNote(null)}>View all notes <Icon name="external" size={13} /></button>
</aside>

<style>
  .tp { border-left: 1px solid var(--border); background: var(--panel); display: flex; flex-direction: column; min-height: 0; overflow: auto; padding: 12px 14px; gap: 6px; }
  .head { display: flex; align-items: center; gap: 8px; padding: 4px 0; }
  .pill { font-size: 11px; background: var(--accent-soft); color: var(--accent); border-radius: 10px; padding: 1px 8px; }
  .grow { flex: 1; min-width: 0; }
  .tasks { list-style: none; margin: 0 0 8px; padding: 0; border-bottom: 1px solid var(--border); }
  .tasks li { display: flex; gap: 10px; padding: 8px 0; align-items: flex-start; }
  .check { width: 18px; height: 18px; border-radius: 50%; border: 1.6px solid var(--muted); background: transparent; padding: 0; flex: none; margin-top: 1px; }
  .check:hover { border-color: var(--accent); background: var(--accent-soft); }
  .tt { font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .env { color: var(--accent); }
  .sub { display: flex; justify-content: space-between; font-size: 11.5px; margin-top: 2px; }
  .notes { display: flex; flex-direction: column; gap: 8px; }
  .ncard { display: flex; gap: 10px; text-align: left; border: 1px solid var(--border); background: var(--panel-2); border-radius: 10px; padding: 10px; align-items: flex-start; }
  .ncard:hover { border-color: var(--accent); }
  .ni { color: var(--accent); }
  .nt { display: block; font-weight: 600; font-size: 12.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .ns { display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; font-size: 12px; color: var(--muted); }
  .small { font-size: 11.5px; white-space: nowrap; }
  .link { border: 0; background: transparent; color: var(--accent); text-align: left; padding: 8px 0; display: inline-flex; gap: 6px; align-items: center; font-size: 13px; }
</style>
