<script lang="ts">
  // Shared message editor: used inline in the preview pane (Reply / Reply all /
  // Forward / Edit draft) and inside the standalone Compose window.
  import Icon from "./Icon.svelte";
  import RecipientField from "./RecipientField.svelte";
  import { size } from "./format";
  import { api, errorText, type Account, type ComposeDraft, type ComposerResult, type ForwardedAttachment, type PickedAttachment } from "./api";

  let { account, draft, title, inline = false, replaceDraft = null, onDone }:
    { account: Account; draft: ComposeDraft; title: string; inline?: boolean; replaceDraft?: number | null;
      onDone: (r: ComposerResult) => void } = $props();

  // svelte-ignore state_referenced_locally
  const init = draft;
  let to = $state<string[]>([...init.to]);
  let cc = $state<string[]>([...init.cc]);
  let bcc = $state<string[]>([...init.bcc]);
  let showCcBcc = $state(init.cc.length > 0 || init.bcc.length > 0);
  let subject = $state(init.subject);
  let body = $state(init.body);
  let carried = $state<ForwardedAttachment[]>([...init.attachments]);
  let picked = $state<PickedAttachment[]>([]);
  let busy = $state<"" | "send" | "draft" | "pick">("");
  let error = $state("");
  let notice = $state("");
  let bodyEl: HTMLTextAreaElement | undefined = $state();

  $effect(() => {
    // Replies: cursor at the top, above the quote.
    if (inline && bodyEl && init.to.length) { bodyEl.focus(); bodyEl.setSelectionRange(0, 0); bodyEl.scrollTop = 0; }
  });

  function message() {
    return {
      to, cc, bcc, subject, body,
      in_reply_to: init.in_reply_to, references: init.references,
      attachments: [
        ...carried.map((a) => ({ kind: "message" as const, message_id: a.message_id, index: a.index })),
        ...picked.map((p) => ({ kind: "file" as const, token: p.token })),
      ],
    };
  }
  const total = $derived(carried.reduce((n, a) => n + a.size, 0) + picked.reduce((n, a) => n + a.size, 0));

  async function pick() {
    busy = "pick"; error = ""; notice = "";
    try {
      const r = await api.pickAttachments();
      picked = [...picked, ...r.files];
      if (r.rejected.length) notice = `Not attached: ${r.rejected.join("; ")}`;
    } catch (e) { error = errorText(e); } finally { busy = ""; }
  }
  async function send() {
    if (!to.length && !cc.length && !bcc.length) { error = "Add at least one recipient."; return; }
    busy = "send"; error = "";
    try {
      const r = await api.send(account.id, message(), replaceDraft);
      const warn = r.sent_copy === "failed"
        ? `Sent, but the copy could not be saved to Sent: ${r.sent_copy_error}`
        : r.sent_copy === "no_sent_folder" ? "Sent. This account has no Sent folder, so no copy was saved."
        : r.draft_cleanup_error ? `Sent, but the old draft could not be removed: ${r.draft_cleanup_error}` : null;
      onDone({ kind: "sent", warning: warn });
    } catch (e) { error = errorText(e); } finally { busy = ""; }
  }
  async function saveDraft() {
    busy = "draft"; error = "";
    try { await api.saveDraft(account.id, message(), replaceDraft); onDone({ kind: "draft" }); }
    catch (e) { error = errorText(e); } finally { busy = ""; }
  }
  function onKey(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") { e.preventDefault(); void send(); }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section class="composer" class:inline aria-label={title} onkeydown={onKey} role="form">
  <header>
    <strong>{title}</strong>
    <span class="from muted">from {account.display_name || account.email} &lt;{account.email}&gt;</span>
    {#if !showCcBcc}<button type="button" class="link" onclick={() => (showCcBcc = true)}>Cc / Bcc</button>{/if}
  </header>
  <RecipientField label="To" accountId={account.id} bind:values={to} autofocus={!init.to.length} />
  {#if showCcBcc}
    <RecipientField label="Cc" accountId={account.id} bind:values={cc} />
    <RecipientField label="Bcc" accountId={account.id} bind:values={bcc} />
  {/if}
  <label class="subject"><span>Subject</span><input bind:value={subject} aria-label="Subject" /></label>
  <textarea bind:this={bodyEl} bind:value={body} placeholder="Write your message…" aria-label="Message body"></textarea>
  {#if carried.length || picked.length}
    <div class="atts" aria-label="Attachments">
      {#each carried as a, i (`m${a.message_id}-${a.index}`)}
        <span class="att" title="Forwarded from the original message"><Icon name="clip" size={14} />{a.name}<em>{size(a.size)}</em>
          <button type="button" aria-label={`Remove ${a.name}`} onclick={() => (carried = carried.filter((_, j) => j !== i))}>×</button></span>
      {/each}
      {#each picked as p, i (p.token)}
        <span class="att"><Icon name="clip" size={14} />{p.name}<em>{size(p.size)}</em>
          <button type="button" aria-label={`Remove ${p.name}`} onclick={() => (picked = picked.filter((_, j) => j !== i))}>×</button></span>
      {/each}
      <span class="muted total">{size(total)} total · max 25 MB</span>
    </div>
  {/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <footer>
    <button type="button" class="btn primary" onclick={send} disabled={busy !== ""}>{busy === "send" ? "Sending…" : "Send"}</button>
    <button type="button" class="btn" onclick={pick} disabled={busy !== ""} title="Attach files"><Icon name="clip" size={15} /> Attach</button>
    <button type="button" class="btn" onclick={saveDraft} disabled={busy !== ""}>{busy === "draft" ? "Saving…" : "Save draft"}</button>
    <span class="spacer"></span>
    <button type="button" class="btn ghost" onclick={() => onDone({ kind: "discarded" })} disabled={busy === "send"}>Discard</button>
  </footer>
</section>

<style>
  .composer { display: flex; flex-direction: column; gap: 4px; background: var(--panel); min-height: 0; height: 100%; }
  .composer.inline { border: 1px solid var(--border); border-radius: 14px; padding: 12px 16px; box-shadow: 0 -6px 24px #0003; }
  header { display: flex; align-items: baseline; gap: 10px; padding-bottom: 4px; }
  .from { font-size: 12px; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .link { border: 0; background: transparent; color: var(--unread); padding: 0; font-size: 13px; }
  .subject { display: flex; align-items: center; gap: 10px; border-bottom: 1px solid var(--border); padding: 2px 0; }
  .subject span { width: 56px; color: var(--muted); flex: none; }
  .subject input { flex: 1; border: 0; background: transparent; }
  .subject input:focus { outline: none; }
  textarea { flex: 1; min-height: 120px; resize: none; border: 0; background: transparent; padding: 10px 2px; line-height: 1.5; font-family: inherit; }
  textarea:focus { outline: none; }
  .atts { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; padding: 4px 0; }
  .att { display: inline-flex; align-items: center; gap: 6px; border: 1px solid var(--border); border-radius: 8px; padding: 4px 6px 4px 8px; font-size: 13px; }
  .att em { color: var(--muted); font-style: normal; font-size: 12px; }
  .att button { border: 0; background: transparent; color: var(--muted); padding: 0 4px; }
  .total { font-size: 12px; }
  .notice { margin: 0; color: #f5b14c; font-size: 13px; }
  .error { margin: 0; font-size: 13px; }
  footer { display: flex; gap: 8px; align-items: center; padding-top: 6px; }
  footer .btn { display: inline-flex; gap: 6px; align-items: center; }
  .spacer { flex: 1; }
  .ghost { background: transparent; }
</style>
