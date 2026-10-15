<script lang="ts">
  // To-do: smart lists, custom lists, quick add, due dates, reminders,
  // priority, subtasks, task notes and linked emails. Local storage.
  import { onMount } from "svelte";
  import Icon from "./Icon.svelte";
  import { dayAt, dayEnd, dueLabel, errorText, pimApi, taskInput, type EmailLink, type MessageSummary, type Task,
    type TaskCounts, type TaskList, type TaskView } from "./api";
  import { shortDate } from "./format";

  let { onOpenEmail, onOpenMessage, onStatus, refreshKey = 0 }: {
    onOpenEmail: (l: EmailLink) => void; onOpenMessage: (id: number) => void; onStatus: (t: string, warn?: boolean) => void; refreshKey?: number;
  } = $props();

  type Sel = "today" | "upcoming" | "flagged" | "completed" | number;
  let sel = $state<Sel>("today");
  let lists = $state<TaskList[]>([]);
  let counts = $state<TaskCounts | null>(null);
  let tasks = $state<Task[]>([]);
  let flagged = $state<MessageSummary[]>([]);
  let current = $state<Task | null>(null);
  let quick = $state("");
  let newList = $state<string | null>(null);
  let subInput = $state("");
  let confirmDelete = $state(false);

  const view = (s: Sel): TaskView | null =>
    s === "today" ? { kind: "today", day_end: dayEnd() } : s === "upcoming" ? { kind: "upcoming", day_end: dayEnd() }
    : s === "completed" ? { kind: "completed" } : s === "flagged" ? null : { kind: "list", id: s };
  async function load() {
    try {
      [lists, counts] = await Promise.all([pimApi.taskLists(), pimApi.taskCounts(dayEnd())]);
      const v = view(sel);
      if (v) {
        tasks = await pimApi.tasks(v); flagged = [];
        // Today also shows what was finished today, struck through, at the bottom.
        if (v.kind === "today") {
          const start = dayEnd() - 86400;
          const done = (await pimApi.tasks({ kind: "completed" })).filter((t) => (t.completed ?? 0) >= start);
          tasks = [...tasks, ...done];
        }
      } else { flagged = await pimApi.flaggedEmails(); tasks = await pimApi.tasks({ kind: "from_email" }); }
      if (current) current = tasks.find((t) => t.id === current!.id) ?? (await pimApi.tasks({ kind: "completed" })).find((t) => t.id === current!.id) ?? null;
    } catch (e) { onStatus(errorText(e), true); }
  }
  onMount(load);
  $effect(() => { void refreshKey; void load(); });
  async function pick(s: Sel) { sel = s; current = null; await load(); }

  const title = $derived(sel === "today" ? "Today" : sel === "upcoming" ? "Upcoming" : sel === "flagged" ? "Flagged emails" : sel === "completed" ? "Completed" : lists.find((l) => l.id === sel)?.name ?? "List");
  const listName = (id: number | null) => lists.find((l) => l.id === id)?.name ?? "";
  const listColor = (id: number | null) => lists.find((l) => l.id === id)?.color ?? "var(--muted)";
  const prio: Record<number, [string, string]> = { 1: ["High", "#ef4444"], 5: ["Medium", "#f5b14c"], 9: ["Low", "#3b82f6"] };
  const overdue = (t: Task) => t.due !== null && !t.completed && t.due < dayEnd(-1);

  async function addQuick() {
    const text = quick.trim(); if (!text) return;
    // "!high" / "!med" / "!low" set the priority; the current list or Today is kept.
    let priority = 0, t = text;
    const m = t.match(/\s!(high|med|medium|low)\b/i);
    if (m) { priority = { high: 1, med: 5, medium: 5, low: 9 }[m[1].toLowerCase() as "high"]; t = t.replace(m[0], "").trim(); }
    try {
      const listId = typeof sel === "number" ? sel : lists[0]?.id ?? null;
      const due = sel === "today" ? dayAt(0) : sel === "upcoming" ? dayAt(1) : null;
      await pimApi.saveTask({ id: null, list_id: listId, parent_id: null, title: t, notes: "", due, due_time: false, reminder: null, priority, email: null });
      quick = ""; await load();
    } catch (e) { onStatus(errorText(e), true); }
  }
  async function toggle(t: Task) {
    try { const r = await pimApi.setTaskCompleted(t.id, t.completed === null); if (current?.id === t.id) current = r; await load(); }
    catch (e) { onStatus(errorText(e), true); }
  }
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  function edit(delay = 500) { clearTimeout(saveTimer); saveTimer = setTimeout(() => void save(), delay); }
  async function save() {
    if (!current || !current.title.trim()) return;
    try { current = await pimApi.saveTask(taskInput(current)); await load(); } catch (e) { onStatus(errorText(e), true); }
  }
  async function addSub() {
    if (!current || !subInput.trim()) return;
    try {
      await pimApi.saveTask({ id: null, list_id: current.list_id, parent_id: current.id, title: subInput, notes: "", due: null, due_time: false, reminder: null, priority: 0, email: null });
      subInput = ""; await load();
    } catch (e) { onStatus(errorText(e), true); }
  }
  async function toggleSub(s: Task) { try { await pimApi.setTaskCompleted(s.id, s.completed === null); await load(); } catch (e) { onStatus(errorText(e), true); } }
  async function removeSub(s: Task) { try { await pimApi.deleteTask(s.id); await load(); } catch (e) { onStatus(errorText(e), true); } }
  async function remove() {
    if (!current) return;
    try { await pimApi.deleteTask(current.id); current = null; confirmDelete = false; await load(); } catch (e) { onStatus(errorText(e), true); }
  }
  async function addList() {
    const n = newList?.trim(); newList = null; if (!n) return;
    const palette = ["#8b5cf6", "#3b82f6", "#22c55e", "#eab308", "#ef4444", "#14b8a6"];
    try { const id = await pimApi.createTaskList(n, palette[lists.length % palette.length]); await load(); await pick(id); } catch (e) { onStatus(errorText(e), true); }
  }
  async function fromFlagged(m: MessageSummary) {
    try {
      const email = await pimApi.emailLink(m.id);
      await pimApi.saveTask({ id: null, list_id: lists[0]?.id ?? null, parent_id: null, title: m.subject || "(no subject)", notes: "", due: dayAt(0), due_time: false, reminder: null, priority: 0, email });
      onStatus(`Added “${m.subject}” to Today`); await load();
    } catch (e) { onStatus(errorText(e), true); }
  }
  // <input type="date"/"datetime-local"> helpers (local time).
  const pad = (n: number) => String(n).padStart(2, "0");
  const toDate = (s: number | null) => { if (s === null) return ""; const d = new Date(s * 1000); return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`; };
  function setDue(v: string) { if (!current) return; current.due = v ? Math.floor(new Date(`${v}T12:00`).getTime() / 1000) : null; edit(0); }
  const toTime = (s: number | null) => { if (s === null) return ""; const d = new Date(s * 1000); return `${pad(d.getHours())}:${pad(d.getMinutes())}`; };
  // Separate date + time inputs: WebKitGTK's datetime-local editing is unreliable.
  function setReminder(date: string, time: string) {
    if (!current) return;
    if (!date && !time) { current.reminder = null; edit(0); return; }
    const d = date || toDate(current.due) || toDate(Math.floor(Date.now() / 1000));
    current.reminder = Math.floor(new Date(`${d}T${time || "09:00"}`).getTime() / 1000);
    edit(0);
  }
  const today = new Date().toLocaleDateString([], { weekday: "long", month: "long", day: "numeric" });
</script>

<div class="todo" class:detail={current !== null}>
  <aside class="side">
    <div class="sechead">Smart lists</div>
    <button class="row" class:active={sel === "today"} onclick={() => pick("today")}><Icon name="sun" size={16} /> <span class="grow">Today</span>{#if counts?.today}<span class="badge">{counts.today}</span>{/if}</button>
    <button class="row" class:active={sel === "upcoming"} onclick={() => pick("upcoming")}><Icon name="calendar" size={16} /> <span class="grow">Upcoming</span><span class="muted small">{counts?.upcoming || ""}</span></button>
    <button class="row" class:active={sel === "flagged"} onclick={() => pick("flagged")}><Icon name="flag" size={16} /> <span class="grow">Flagged emails</span><span class="muted small">{counts?.from_email || ""}</span></button>
    <button class="row" class:active={sel === "completed"} onclick={() => pick("completed")}><Icon name="checkcircle" size={16} /> <span class="grow">Completed</span><span class="muted small">{counts?.completed || ""}</span></button>
    <div class="sechead">Lists</div>
    {#each lists as l (l.id)}
      <button class="row" class:active={sel === l.id} onclick={() => pick(l.id)}><span class="ldot" style:background={l.color}></span> <span class="grow">{l.name}</span><span class="muted small">{l.open || ""}</span></button>
    {/each}
    {#if newList !== null}
      <input class="newin" placeholder="List name" bind:value={newList} maxlength="80" use:focusEl onkeydown={(e) => { if (e.key === "Enter") void addList(); if (e.key === "Escape") newList = null; }} onblur={addList} />
    {/if}
    <button class="row add" onclick={() => (newList = "")}><Icon name="plus" size={16} /> New list</button>
  </aside>

  <section class="main">
    <header>
      <h1>{title}</h1>
      {#if sel === "today"}<span class="muted">{today}</span>{/if}
      {#if typeof sel === "number"}
        <span class="grow"></span>
        <button class="btn sm" onclick={async () => { const n = prompt("Rename list", listName(sel as number)); if (n) { try { await pimApi.renameTaskList(sel as number, n); await load(); } catch (e) { onStatus(errorText(e), true); } } }}>Rename</button>
        <button class="btn sm danger" onclick={async () => { if (confirm(`Delete the list “${listName(sel as number)}” and its tasks?`)) { await pimApi.deleteTaskList(sel as number); await pick("today"); } }}>Delete list</button>
      {/if}
    </header>
    {#if sel !== "completed"}
      <form class="quick" onsubmit={(e) => { e.preventDefault(); void addQuick(); }}>
        <Icon name="plus" size={18} />
        <input placeholder="Add a task…" bind:value={quick} maxlength="300" aria-label="Add a task"
          onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); void addQuick(); } }} />
        <kbd>Ctrl+Enter</kbd>
      </form>
    {/if}
    <ul class="tasks">
      {#each tasks as t (t.id)}
        <li>
          <div class="task" class:done={t.completed !== null} class:selected={current?.id === t.id} role="button" tabindex="0"
            onclick={() => { current = t; confirmDelete = false; }} onkeydown={(e) => { if (e.key === "Enter") current = t; }}>
            <button class="check" class:on={t.completed !== null} aria-label={t.completed ? "Mark as not done" : "Mark as done"} onclick={(e) => { e.stopPropagation(); void toggle(t); }}>
              {#if t.completed}<Icon name="check" size={13} />{/if}</button>
            {#if t.email}<span class="env" title="Created from email"><Icon name="mail" size={15} /></span>{/if}
            <span class="tt">{t.title}
              {#if t.subtasks.length}<span class="muted small"> · {t.subtasks.filter((s) => s.completed).length}/{t.subtasks.length}</span>{/if}
              {#if t.list_id !== null && typeof sel !== "number"}<span class="lname" style:color={listColor(t.list_id)}>{listName(t.list_id)}</span>{/if}
            </span>
            {#if t.reminder && !t.completed}<span class="muted" title="Reminder {new Date(t.reminder * 1000).toLocaleString()}"><Icon name="bell" size={14} /></span>{/if}
            {#if t.due !== null}<span class="chip" class:late={overdue(t)}>{dueLabel(t.due)}</span>{/if}
            {#if prio[t.priority]}<span class="pdot" style:background={prio[t.priority][1]} title="{prio[t.priority][0]} priority"></span>{/if}
          </div>
        </li>
      {/each}
      {#each flagged as m (m.id)}
        <li>
          <div class="task flaggedmail">
            <span class="env"><Icon name="flag" size={15} /></span>
            <span class="tt">{m.subject || "(no subject)"} <span class="muted small">· {m.from_name || m.from_addr} · {shortDate(m.date)}</span></span>
            <button class="btn sm" onclick={() => fromFlagged(m)}>Add to Today</button>
            <button class="btn sm" onclick={() => onOpenMessage(m.id)}>Open email</button>
          </div>
        </li>
      {/each}
      {#if !tasks.length && !flagged.length}<li class="empty muted">{sel === "completed" ? "Nothing completed yet." : sel === "flagged" ? "No starred emails or tasks from email." : "No tasks. Add one above."}</li>{/if}
    </ul>
  </section>

  {#if current}
    {@const c = current}
    <aside class="panel" aria-label="Task details">
      <div class="phead">
        <button class="check big" class:on={c.completed !== null} aria-label="Toggle done" onclick={() => toggle(c)}>{#if c.completed}<Icon name="check" size={15} />{/if}</button>
        <textarea class="ptitle" bind:value={c.title} oninput={() => edit()} rows="2" maxlength="300" aria-label="Title"></textarea>
        <button class="icon-btn" aria-label="Close details" onclick={() => (current = null)}><Icon name="close" size={16} /></button>
      </div>
      {#if c.email}<div class="from"><Icon name="mail" size={14} /> Created from email</div>{/if}
      <label class="field"><span><Icon name="calendar" size={15} /> Due</span>
        <input type="date" value={toDate(c.due)} onchange={(e) => setDue((e.currentTarget as HTMLInputElement).value)} /></label>
      <label class="field"><span><Icon name="bell" size={15} /> Reminder</span>
        <span class="remind"><input type="date" aria-label="Reminder date" value={toDate(c.reminder)} onchange={(e) => setReminder((e.currentTarget as HTMLInputElement).value, toTime(c.reminder))} /><input class="rtime" aria-label="Reminder time" placeholder="HH:MM" maxlength="5" value={toTime(c.reminder)} onchange={(e) => { const v = (e.currentTarget as HTMLInputElement).value.trim(); const m = /^([01]?\d|2[0-3])[:.]?([0-5]\d)$/.exec(v); if (m) setReminder(toDate(c.reminder), `${pad(+m[1])}:${m[2]}`); else if (!v) setReminder("", ""); else onStatus("Reminder time must be HH:MM", true); }} />{#if c.reminder}<button class="clr" title="Clear reminder" onclick={() => setReminder("", "")}><Icon name="close" size={13} /></button>{/if}</span></label>
      <label class="field"><span><Icon name="todo" size={15} /> List</span>
        <select bind:value={c.list_id} onchange={() => edit(0)}>{#each lists as l (l.id)}<option value={l.id}>{l.name}</option>{/each}<option value={null}>No list</option></select></label>
      <label class="field"><span><Icon name="flag" size={15} /> Priority</span>
        <select bind:value={c.priority} onchange={() => edit(0)}><option value={0}>None</option><option value={1}>High</option><option value={5}>Medium</option><option value={9}>Low</option></select></label>
      <div class="label">Notes</div>
      <textarea class="pnotes" bind:value={c.notes} oninput={() => edit()} rows="4" placeholder="Add notes" maxlength="20000"></textarea>
      <div class="label">Subtasks</div>
      <ul class="subs">
        {#each c.subtasks as s (s.id)}
          <li class:done={s.completed !== null}>
            <button class="check sm" class:on={s.completed !== null} aria-label="Toggle subtask" onclick={() => toggleSub(s)}>{#if s.completed}<Icon name="check" size={11} />{/if}</button>
            <span class="grow">{s.title}</span>
            <button class="icon-btn sm" aria-label="Delete subtask" onclick={() => removeSub(s)}><Icon name="close" size={12} /></button>
          </li>
        {/each}
        <li><input placeholder="+ Add subtask" bind:value={subInput} maxlength="300" onkeydown={(e) => { if (e.key === "Enter") void addSub(); }} aria-label="Add subtask" /></li>
      </ul>
      {#if c.email}
        {@const e = c.email}
        <div class="label">Linked email</div>
        <div class="ecard">
          <div class="eh"><Icon name="mail" size={18} /><b class="grow">{e.subject || "(no subject)"}</b></div>
          <div class="muted small">From: {e.sender}{e.date ? ` · ${new Date(e.date * 1000).toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" })}` : ""}</div>
          <button class="btn primary sm" onclick={() => onOpenEmail(e)}><Icon name="external" size={14} /> Open email</button>
        </div>
      {/if}
      <div class="pfoot">
        <span class="muted small">Created {new Date(c.created * 1000).toLocaleDateString()}</span>
        {#if confirmDelete}<button class="btn sm" onclick={() => (confirmDelete = false)}>Cancel</button><button class="btn sm danger" onclick={remove}>Delete task</button>
        {:else}<button class="btn sm danger" onclick={() => (confirmDelete = true)}>Delete…</button>{/if}
      </div>
    </aside>
  {/if}
</div>

<script module lang="ts">
  export function focusEl(el: HTMLElement) { el.focus(); }
</script>

<style>
  .todo { display: grid; grid-template-columns: 250px 1fr; height: 100%; min-height: 0; }
  .todo.detail { grid-template-columns: 250px 1fr 340px; }
  .side { background: var(--panel-2); border-right: 1px solid var(--border); padding: 12px 10px; display: flex; flex-direction: column; gap: 1px; overflow: auto; }
  .sechead { font-size: 11px; letter-spacing: .06em; text-transform: uppercase; color: var(--muted); padding: 10px 8px 4px; }
  .row { display: flex; gap: 10px; align-items: center; border: 0; background: transparent; text-align: left; padding: 8px 10px; border-radius: 8px; font-size: 13.5px; }
  .row:hover { background: var(--hover); } .row.active { background: var(--selected); }
  .row.add { color: var(--muted); margin-top: 4px; }
  .grow { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .badge { background: var(--accent); color: #fff; border-radius: 10px; font-size: 12px; padding: 0 8px; }
  .ldot { width: 10px; height: 10px; border-radius: 3px; flex: none; }
  .small { font-size: 12px; } .newin { margin: 4px 8px; font-size: 13px; }
  .main { display: flex; flex-direction: column; min-height: 0; min-width: 0; background: var(--panel); padding: 18px 24px 0; }
  header { display: flex; align-items: baseline; gap: 14px; }
  h1 { margin: 0 0 12px; font-size: 26px; }
  .quick { display: flex; align-items: center; gap: 10px; border: 1px solid var(--border); background: var(--panel-2); border-radius: 10px; padding: 0 12px; color: var(--muted); margin-bottom: 10px; }
  .quick input { flex: 1; border: 0; background: transparent; padding: 12px 0; outline: none; font-size: 14px; }
  kbd { font-size: 11px; border: 1px solid var(--border); border-radius: 5px; padding: 1px 6px; color: var(--muted); }
  .tasks { list-style: none; margin: 0; padding: 0 0 20px; overflow: auto; flex: 1; }
  .task { display: flex; align-items: center; gap: 10px; padding: 11px 12px; border-bottom: 1px solid var(--border); border-radius: 8px; cursor: default; }
  .task:hover { background: var(--hover); } .task.selected { background: var(--selected); }
  .task.done .tt { text-decoration: line-through; color: var(--muted); }
  .check { width: 20px; height: 20px; border-radius: 50%; border: 1.8px solid var(--muted); background: transparent; display: grid; place-items: center; padding: 0; flex: none; color: #fff; }
  .check:hover { border-color: var(--accent); }
  .check.on { background: var(--accent); border-color: var(--accent); }
  .check.big { width: 24px; height: 24px; margin-top: 4px; } .check.sm { width: 16px; height: 16px; }
  .env { color: var(--accent); display: inline-flex; }
  .tt { flex: 1; min-width: 0; font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .lname { font-size: 12px; margin-left: 8px; }
  .chip { font-size: 12px; border: 1px solid var(--border); border-radius: 10px; padding: 1px 9px; color: var(--muted); white-space: nowrap; }
  .chip.late { color: #ef4444; border-color: #ef444466; }
  .pdot { width: 9px; height: 9px; border-radius: 50%; flex: none; }
  .empty { padding: 30px; text-align: center; }
  .sm { padding: 3px 8px; font-size: 12px; } .danger { color: #e5534b; }
  .panel { border-left: 1px solid var(--border); background: var(--panel-2); padding: 16px; display: flex; flex-direction: column; gap: 10px; overflow: auto; }
  .phead { display: flex; gap: 10px; align-items: flex-start; }
  .ptitle { flex: 1; font-size: 17px; font-weight: 600; border: 0; background: transparent; resize: none; color: var(--text); outline: none; padding: 2px; font-family: inherit; }
  .from { display: flex; gap: 6px; align-items: center; font-size: 12px; color: var(--accent); }
  .field { display: grid; grid-template-columns: 100px 1fr; align-items: center; gap: 8px; font-size: 13px; }
  .field span { display: inline-flex; gap: 6px; align-items: center; color: var(--muted); }
  .field input, .field select { font-size: 13px; color: var(--text); color-scheme: dark; }
  .label { font-size: 11px; letter-spacing: .06em; text-transform: uppercase; color: var(--muted); margin-top: 6px; }
  .pnotes { font-family: inherit; font-size: 13px; resize: vertical; }
  .subs { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
  .subs li { display: flex; gap: 8px; align-items: center; font-size: 13px; }
  .subs li.done .grow { text-decoration: line-through; color: var(--muted); }
  .subs input { flex: 1; font-size: 13px; }
  .ecard { border: 1px solid var(--border); border-radius: 10px; padding: 12px; display: flex; flex-direction: column; gap: 6px; background: var(--panel); align-items: flex-start; }
  .eh { display: flex; gap: 8px; align-items: center; width: 100%; color: var(--accent); } .eh b { color: var(--text); font-size: 13px; }
  .pfoot { margin-top: auto; display: flex; gap: 8px; align-items: center; padding-top: 10px; border-top: 1px solid var(--border); }
  .pfoot span { flex: 1; }
  :global([data-theme="light"]) .field input, :global([data-theme="light"]) .field select { color-scheme: light; }
  .remind { display: flex; gap: 4px; align-items: center; min-width: 0; }
  .remind input { min-width: 0; flex: 1; }
  .remind .rtime { flex: 0 0 64px; }
  .clr { background: none; border: 0; color: var(--muted); cursor: pointer; padding: 2px; }
</style>
