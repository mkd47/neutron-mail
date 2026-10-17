<script lang="ts">
  // Automatic replies ("out of office"). Replies are sent from this computer
  // during sync (works with every server), or by the mail server itself
  // through a Sieve vacation rule when the server offers that.
  import { onMount } from "svelte";
  import { api, errorText, type Account, type AutoReply, type AutoReplyServerSupport } from "./api";

  let { account }: { account: Account } = $props();

  let saved = $state<AutoReply | null>(null);
  let enabled = $state(false);
  let subject = $state("");
  let body = $state("");
  let startDay = $state("");
  let endDay = $state("");
  let days = $state(4);
  let onServer = $state(false);
  let support = $state<AutoReplyServerSupport | null>(null);
  let busy = $state(false);
  let msg = $state<{ text: string; error: boolean } | null>(null);

  const pad = (n: number) => String(n).padStart(2, "0");
  /** Unix seconds → "YYYY-MM-DD" in local time. */
  function toDay(t: number | null): string {
    if (t == null) return "";
    const d = new Date(t * 1000);
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  }
  /** "YYYY-MM-DD" → unix seconds at the start (or end) of that local day. */
  function fromDay(s: string, endOfDay: boolean): number | null {
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
    if (!m) return null;
    const d = endOfDay ? new Date(+m[1], +m[2] - 1, +m[3], 23, 59, 59) : new Date(+m[1], +m[2] - 1, +m[3]);
    return Math.floor(d.getTime() / 1000);
  }

  function load(r: AutoReply) {
    saved = r; enabled = r.enabled; subject = r.subject; body = r.body;
    startDay = toDay(r.start); endDay = toDay(r.end); days = r.days; onServer = r.on_server;
  }

  onMount(async () => {
    try { load(await api.getAutoReply(account.id)); }
    catch (e) { msg = { text: errorText(e), error: true }; return; }
    try { support = await api.autoReplyServerSupport(account.id); }
    catch (e) { support = { available: false, dates: false, detail: errorText(e), provider_hint: null }; }
  });

  const serverOk = $derived(support?.available ?? false);
  const now = Math.floor(Date.now() / 1000);
  const status = $derived.by(() => {
    if (!saved?.enabled) return "Off";
    const where = saved.on_server ? "by the mail server" : "from this computer while Neutron Mail is running";
    if (saved.end != null && saved.end < now) return "On, but the end date has passed";
    if (saved.start != null && saved.start > now) return `Scheduled from ${toDay(saved.start)}, replies ${where}`;
    return `On, replies ${where}`;
  });

  async function save() {
    msg = null;
    const start = fromDay(startDay, false), end = fromDay(endDay, true);
    if (enabled && !body.trim()) { msg = { text: "Write the reply text first.", error: true }; return; }
    if (start != null && end != null && end < start) { msg = { text: "The end date is before the start date.", error: true }; return; }
    if (enabled && onServer && support && !support.available) {
      msg = { text: "The mail server can't send automatic replies right now. Choose \"From this computer\".", error: true };
      return;
    }
    if (enabled && onServer && support && !support.dates && (start != null || end != null)) {
      msg = { text: "This server can't limit automatic replies to dates. Clear the dates, or reply from this computer.", error: true };
      return;
    }
    busy = true;
    try {
      load(await api.setAutoReply(account.id, { enabled, subject: subject.trim(), body, start, end,
        days: Math.min(30, Math.max(1, Math.round(days) || 4)), on_server: onServer, enabled_at: saved?.enabled_at ?? 0 }));
      msg = { text: enabled ? "Saved. Automatic replies are on." : "Saved. Automatic replies are off.", error: false };
    } catch (e) { msg = { text: errorText(e), error: true }; }
    finally { busy = false; }
  }
</script>

<section class="card">
  <div class="head">
    <h3>Automatic replies</h3>
    {#if saved}<span class="badge" class:on={saved.enabled}>{status}</span>{/if}
  </div>
  <p class="muted">Answer incoming mail automatically while you're away. Each sender gets at most one reply per period,
    and mailing lists, newsletters and other automatic mail are never answered.</p>
  {#if saved}
    <label class="toggle"><input type="checkbox" bind:checked={enabled} /> Send automatic replies</label>
    <fieldset disabled={!enabled}>
      <label><span>Subject <span class="muted">(optional)</span></span>
        <input bind:value={subject} maxlength="200" placeholder="Automatic reply: <original subject>" /></label>
      <label>Message
        <textarea bind:value={body} rows="5" maxlength="16000"
          placeholder="Thanks for your mail. I'm out of the office until …, with limited access to email."></textarea></label>
      <div class="row">
        <label><span>From <span class="muted">(optional)</span></span><input type="date" bind:value={startDay} aria-label="Start date" /></label>
        <label><span>Until <span class="muted">(optional)</span></span><input type="date" bind:value={endDay} aria-label="End date" /></label>
        <label><span>Reply to the same sender once every</span>
          <span class="days"><input type="number" min="1" max="30" bind:value={days} aria-label="Days between replies" /> days</span></label>
      </div>
      <div class="where" role="radiogroup" aria-label="Where automatic replies are sent from">
        <label class="opt"><input type="radio" name="where" value={false} bind:group={onServer} />
          <span><strong>From this computer</strong><br /><span class="muted">Works with every mail server, but only while
            Neutron Mail is running and syncing.</span></span></label>
        <label class="opt" class:off={!serverOk}><input type="radio" name="where" value={true} bind:group={onServer} disabled={!serverOk} />
          <span><strong>On the mail server</strong><br /><span class="muted">
            {#if support == null}Checking whether your server can do this…
            {:else if serverOk}Keeps replying while this computer is off.{#if !support.dates} This server ignores the dates, so set none.{/if}
            {:else}Not available: {support.detail}{/if}</span></span></label>
      </div>
    </fieldset>
    {#if support?.provider_hint}<p class="hint" role="note">{support.provider_hint}</p>{/if}
    <div class="actions">
      {#if msg}<span class:error={msg.error} class:ok={!msg.error} role="status">{msg.text}</span>{/if}
      <button class="btn primary" onclick={save} disabled={busy}>{busy ? "Saving…" : "Save automatic replies"}</button>
    </div>
  {:else if msg}
    <p class="error" role="alert">{msg.text}</p>
  {/if}
</section>

<style>
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: 16px; padding: 20px;
    display: flex; flex-direction: column; gap: 10px; }
  h3, p { margin: 0; }
  .head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
  .badge { font-size: 12px; padding: 2px 10px; border-radius: 999px; border: 1px solid var(--border); color: var(--muted, inherit); }
  .badge.on { border-color: var(--accent); color: var(--accent); }
  fieldset { border: 0; padding: 0; margin: 0; display: flex; flex-direction: column; gap: 10px; min-width: 0; }
  fieldset:disabled { opacity: 0.6; }
  label { display: flex; flex-direction: column; gap: 4px; }
  label.toggle, label.opt { flex-direction: row; align-items: flex-start; gap: 8px; }
  .toggle { font-weight: 600; }
  textarea { resize: vertical; font: inherit; }
  .row { display: flex; gap: 12px; flex-wrap: wrap; }
  .row label { flex: 1 1 160px; }
  .days { display: flex; align-items: center; gap: 6px; }
  .days input { width: 64px; }
  .where { display: flex; flex-direction: column; gap: 8px; }
  .opt { padding: 8px 10px; border: 1px solid var(--border); border-radius: 10px; }
  .opt.off { opacity: 0.7; }
  .opt input { margin-top: 3px; }
  .hint { font-size: 13px; padding: 8px 10px; border-radius: 10px; background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .actions { display: flex; justify-content: flex-end; align-items: center; gap: 8px; }
  .actions span { flex: 1; text-align: right; }
  .actions button { white-space: nowrap; }
  .ok { color: var(--accent); }
  .error { color: var(--danger, #d33); }
</style>
