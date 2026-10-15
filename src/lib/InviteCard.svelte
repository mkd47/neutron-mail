<script lang="ts">
  // Invitation card (iMIP, RFC 6047). Data comes from the Rust iCalendar
  // parser; everything is rendered as text nodes, never as HTML.
  import Icon from "./Icon.svelte";
  import { eventRange } from "./format";
  import { calendarApi, errorText, type InviteView, type PartStat } from "./api";

  let { messageId, onStatus, onResponded }: {
    messageId: number; onStatus: (s: string, warn?: boolean) => void; onResponded: () => void;
  } = $props();

  let invite = $state<InviteView | null>(null);
  let busy = $state<PartStat | null>(null);
  let addToCalendar = $state(true);
  let loadedFor = -1;
  $effect(() => {
    const id = messageId;
    if (id === loadedFor) return;
    loadedFor = id; invite = null;
    calendarApi.getInvite(id).then((v) => { if (loadedFor === id) invite = v; }).catch(() => {});
  });

  const label: Record<string, string> = {
    ACCEPTED: "You accepted", TENTATIVE: "You answered maybe", DECLINED: "You declined", "NEEDS-ACTION": "Not answered yet",
  };
  async function respond(p: PartStat) {
    busy = p;
    try {
      const r = await calendarApi.respond(messageId, p, addToCalendar && p !== "DECLINED");
      invite = r.invite;
      const to = r.invite.event.organizer_name || r.invite.event.organizer_email;
      let msg = `Reply sent to ${to}`;
      if (r.invite.in_calendar && p !== "DECLINED") msg += " · added to your calendar";
      if (r.calendar_error) onStatus(`${msg}, but the calendar couldn't be updated: ${r.calendar_error}`, true);
      else if (r.sent_copy === "failed") onStatus(`${msg} (the copy to Sent failed)`, true);
      else onStatus(msg);
      onResponded();
    } catch (e) { onStatus(`Couldn't send the reply: ${errorText(e)}`, true); }
    finally { busy = null; }
  }
</script>

{#if invite}
  {@const e = invite.event}
  <section class="invite" class:cancelled={invite.method === "CANCEL"} aria-label="Calendar invitation">
    <div class="ic"><Icon name="calendar" size={22} /></div>
    <div class="info">
      <div class="kind muted">{invite.method === "CANCEL" ? "Cancelled event" : invite.method === "REQUEST" ? "Invitation" : invite.method === "REPLY" ? "Reply to your invitation" : "Event"}</div>
      <div class="title">{e.summary || "(untitled event)"}</div>
      <div class="when">{eventRange(e.start, e.end, e.all_day)}{e.rrule ? " · repeats" : ""}</div>
      {#if e.location}<div class="muted">{e.location}</div>{/if}
      {#if e.organizer_email}<div class="muted small">Organizer: {e.organizer_name || e.organizer_email}{e.attendees.length ? ` · ${e.attendees.length} invited` : ""}</div>{/if}
      {#if invite.can_respond}
        <div class="status" class:done={invite.responded}>
          {#if invite.responded}<Icon name="check" size={14} />{/if}
          {label[invite.my_partstat] ?? invite.my_partstat}{invite.in_calendar ? " · in your calendar" : ""}
        </div>
      {/if}
    </div>
    {#if invite.can_respond}
      <div class="actions">
        <div class="buttons" role="group" aria-label="Respond">
          {#each [["ACCEPTED", "Accept"], ["TENTATIVE", "Maybe"], ["DECLINED", "Decline"]] as [p, t] (p)}
            <button class="btn" class:primary={invite.my_partstat === p} disabled={busy !== null}
              aria-pressed={invite.my_partstat === p} onclick={() => respond(p as PartStat)}>
              {busy === p ? "Sending…" : t}</button>
          {/each}
        </div>
        <label class="small muted"><input type="checkbox" bind:checked={addToCalendar} /> Add to GNOME Calendar</label>
      </div>
    {/if}
  </section>
{/if}

<style>
  .invite { display: flex; gap: 14px; align-items: flex-start; margin: 0 0 14px; padding: 14px 16px;
    border: 1px solid var(--border); border-left: 4px solid var(--accent); border-radius: var(--radius); background: var(--panel-2); }
  .invite.cancelled { border-left-color: #d64545; }
  .invite.cancelled .title { text-decoration: line-through; }
  .ic { color: var(--accent); padding-top: 2px; }
  .info { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .kind { font-size: 12px; text-transform: uppercase; letter-spacing: .04em; }
  .title { font-weight: 600; font-size: 16px; }
  .small { font-size: 12px; }
  .status { margin-top: 6px; font-size: 13px; display: inline-flex; gap: 4px; align-items: center; color: var(--muted); }
  .status.done { color: var(--accent); }
  .actions { display: flex; flex-direction: column; gap: 8px; align-items: flex-end; }
  .buttons { display: flex; gap: 6px; }
  .buttons .btn { padding: 6px 12px; }
  label { display: inline-flex; gap: 6px; align-items: center; }
  label input { margin: 0; }
</style>
