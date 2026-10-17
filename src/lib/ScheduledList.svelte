<script lang="ts">
  // Messages waiting to be sent later: send now, change the time, edit
  // (back to Drafts) or delete.
  import { api, errorText, type Account, type Scheduled } from "./api";
  import { fromLocalInput, toLocalInput, whenLabel } from "./sendLater";

  let { items, accounts, onClose, onChanged, onStatus }:
    { items: Scheduled[]; accounts: Account[]; onClose: () => void; onChanged: () => void;
      onStatus: (text: string, warn?: boolean) => void } = $props();

  let busy = $state<number | null>(null);
  let editing = $state<{ id: number; value: string } | null>(null);
  let confirmDelete = $state<number | null>(null);
  let error = $state("");

  const email = (id: number) => accounts.find((a) => a.id === id)?.email ?? "";

  async function run(id: number, f: () => Promise<void>, done?: string) {
    busy = id; error = "";
    try { await f(); if (done) onStatus(done); onChanged(); }
    catch (e) { error = errorText(e); }
    finally { busy = null; }
  }
  const sendNow = (s: Scheduled) => run(s.id, () => api.sendScheduledNow(s.id));
  const toDrafts = (s: Scheduled) => run(s.id, () => api.cancelScheduled(s.id, true), "Moved back to Drafts: open it there to edit.");
  const remove = (s: Scheduled) => run(s.id, async () => { confirmDelete = null; await api.cancelScheduled(s.id, false); }, "Scheduled message deleted.");
  function saveTime(s: Scheduled) {
    const when = editing ? fromLocalInput(editing.value) : null;
    if (!when || when.getTime() < Date.now() + 30_000) { error = "Pick a time in the future."; return; }
    void run(s.id, async () => { await api.reschedule(s.id, Math.floor(when.getTime() / 1000)); editing = null; },
      `Rescheduled for ${whenLabel(when)}.`);
  }
</script>

<div class="modal" role="dialog" aria-modal="true" aria-label="Scheduled messages" tabindex="-1"
  onkeydown={(e) => { if (e.key === "Escape") onClose(); }}>
  <div class="card sched">
    <div class="head"><h3>Scheduled</h3><button class="btn" onclick={onClose}>Close</button></div>
    <p class="muted small">Sent by Neutron Mail at the set time. If the app is closed then, they go out when you next open it.</p>
    {#if !items.length}<p class="muted">Nothing scheduled.</p>{/if}
    <ul>
      {#each items as s (s.id)}
        <li class:failed={s.state === "failed"}>
          <div class="line1">
            <strong class="when">{s.state === "sending" ? "Sending…" : whenLabel(new Date(s.send_at * 1000))}</strong>
            <span class="to">to {s.recipients || "recipients"}</span>
          </div>
          <div class="subj">{s.subject || "(no subject)"}{#if accounts.length > 1}<span class="muted small"> · {email(s.account_id)}</span>{/if}</div>
          {#if s.last_error}
            <p class="err small">{s.state === "failed" ? "Couldn't send" : `Retrying (attempt ${s.attempts + 1})`}: {s.last_error}</p>
          {/if}
          {#if editing?.id === s.id}
            <div class="edit">
              <input type="datetime-local" bind:value={editing.value} aria-label="New time" />
              <button class="btn sm primary" onclick={() => saveTime(s)} disabled={busy === s.id}>Save</button>
              <button class="btn sm" onclick={() => (editing = null)}>Cancel</button>
            </div>
          {:else if confirmDelete === s.id}
            <div class="edit">
              <span class="small">Delete this message? It won't be sent.</span>
              <button class="btn sm danger-fill" onclick={() => remove(s)} disabled={busy === s.id}>Delete</button>
              <button class="btn sm" onclick={() => (confirmDelete = null)}>Keep</button>
            </div>
          {:else if s.state !== "sending"}
            <div class="actions">
              <button class="btn sm" onclick={() => sendNow(s)} disabled={busy === s.id}>Send now</button>
              <button class="btn sm" onclick={() => (editing = { id: s.id, value: toLocalInput(new Date(Math.max(s.send_at * 1000, Date.now() + 3_600_000))) })}
                disabled={busy === s.id}>Change time</button>
              <button class="btn sm" onclick={() => toDrafts(s)} disabled={busy === s.id} title="Move it back to Drafts to edit it">Edit</button>
              <button class="btn sm ghost" onclick={() => (confirmDelete = s.id)} disabled={busy === s.id}>Delete</button>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
    {#if error}<p class="err small" role="alert">{error}</p>{/if}
  </div>
</div>

<style>
  .modal { position: fixed; inset: 0; background: #0008; display: grid; place-items: center; z-index: 80; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: 12px; padding: 18px 20px; width: min(520px, 92vw);
    max-height: 80vh; overflow: auto; display: flex; flex-direction: column; gap: 10px; }
  .head { display: flex; justify-content: space-between; align-items: center; }
  h3, p { margin: 0; }
  ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
  li { border: 1px solid var(--border); border-radius: 10px; padding: 10px 12px; display: flex; flex-direction: column; gap: 4px; }
  li.failed { border-color: color-mix(in srgb, #e5534b 50%, var(--border)); }
  .line1 { display: flex; gap: 8px; align-items: baseline; min-width: 0; }
  .when { white-space: nowrap; }
  .to, .subj { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .to { color: var(--muted); font-size: 13px; }
  .actions, .edit { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; margin-top: 4px; }
  .edit input { flex: 1; min-width: 160px; }
  .small { font-size: 12px; }
  .err { color: #e5534b; }
  .danger-fill { background: #d64541; border-color: #d64541; color: #fff; }
</style>
