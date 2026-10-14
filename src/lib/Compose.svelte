<script lang="ts">
  import Icon from "./Icon.svelte";
  import { api, errorText, type Account } from "./api";

  let { account, onClose, initialTo = "", initialSubject = "", initialBody = "" }:
    { account: Account; onClose: (sent: boolean) => void; initialTo?: string; initialSubject?: string; initialBody?: string } = $props();

  // svelte-ignore state_referenced_locally
  let to = $state(initialTo);
  let cc = $state("");
  // svelte-ignore state_referenced_locally
  let subject = $state(initialSubject);
  // svelte-ignore state_referenced_locally
  let body = $state(initialBody);
  let busy = $state(false);
  let error = $state("");

  const split = (s: string) => s.split(/[,;]/).map((x) => x.trim()).filter(Boolean);

  async function send() {
    busy = true; error = "";
    try {
      await api.send(account.id, { to: split(to), cc: split(cc), subject, body });
      onClose(true);
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="backdrop" role="presentation">
  <div class="compose" role="dialog" aria-modal="true" aria-label="New message">
    <header>
      <strong>New message</strong>
      <button class="icon-btn" onclick={() => onClose(false)} aria-label="Close"><Icon name="close" /></button>
    </header>
    <div class="row"><span>From</span><div class="from">{account.display_name} &lt;{account.email}&gt;</div></div>
    <label class="row"><span>To</span><input bind:value={to} placeholder="name@example.org, …" /></label>
    <label class="row"><span>Cc</span><input bind:value={cc} /></label>
    <label class="row"><span>Subject</span><input bind:value={subject} /></label>
    <textarea bind:value={body} placeholder="Write your message…" aria-label="Message body"></textarea>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <footer>
      <span class="muted">Plain text · sent over encrypted SMTP</span>
      <button class="btn primary" onclick={send} disabled={busy || !to.trim()}>
        {busy ? "Sending…" : "Send"}
      </button>
    </footer>
  </div>
</div>

<style>
  .backdrop { position: fixed; inset: 0; background: #0008; display: grid; place-items: center; z-index: 20; }
  .compose { width: min(760px, 92vw); height: min(620px, 88vh); background: var(--panel); border: 1px solid var(--border);
    border-radius: 16px; display: flex; flex-direction: column; padding: 16px 20px; gap: 8px; box-shadow: 0 20px 60px #0006; }
  header, footer { display: flex; justify-content: space-between; align-items: center; }
  .row { display: flex; align-items: center; gap: 10px; border-bottom: 1px solid var(--border); padding: 4px 0; }
  .row span { width: 64px; color: var(--muted); }
  .row input { flex: 1; border: 0; background: transparent; }
  .from { padding: 8px 10px; }
  textarea { flex: 1; resize: none; border: 0; background: transparent; padding: 12px 2px; line-height: 1.5; }
</style>
