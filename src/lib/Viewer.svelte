<script lang="ts">
  import Icon from "./Icon.svelte";
  import Composer from "./Composer.svelte";
  import { initials, shortDate, size } from "./format";
  import { api, errorText, type Account, type ComposeDraft, type ComposeMode, type ComposerResult, type MessageView } from "./api";

  let { message, account, isDraft, hoverUrl, onReload, onStatus, onComposerDone }: {
    message: MessageView | null; account: Account; isDraft: boolean; hoverUrl: string | null;
    onReload: (loadRemote: boolean) => void; onStatus: (s: string) => void; onComposerDone: (r: ComposerResult) => void;
  } = $props();

  let reply = $state<{ mode: ComposeMode; draft: ComposeDraft; key: number } | null>(null);
  let source = $state<string | null>(null);
  let confirmOpen = $state<{ index: number; reason: string } | null>(null);
  let lastId = -1;
  $effect(() => {
    // Switching messages closes source view and any inline composer.
    const id = message?.id ?? -1;
    if (id !== lastId) { lastId = id; source = null; reply = null; confirmOpen = null; }
  });

  const titles: Record<ComposeMode, string> = { reply: "Reply", reply_all: "Reply all", forward: "Forward", draft: "Edit draft" };

  async function startCompose(mode: ComposeMode) {
    if (!message) return;
    try {
      reply = { mode, draft: await api.composePrefill(message.id, mode), key: Date.now() };
      source = null;
    } catch (e) { onStatus(errorText(e)); }
  }
  async function toggleSource() {
    if (!message) return;
    if (source !== null) { source = null; return; }
    try { source = await api.messageSource(message.id); } catch (e) { onStatus(errorText(e)); }
  }
  async function trustSender() {
    if (!message) return;
    await api.setImageSenderTrust(account.id, message.from_addr, !message.sender_trusted);
    onReload(!message.sender_trusted);
  }
  async function save(index: number) {
    if (!message) return;
    try { const p = await api.saveAttachment(message.id, index); if (p) onStatus(`Saved to ${p}`); }
    catch (e) { onStatus(errorText(e)); }
  }
  async function openAtt(index: number, confirmed = false) {
    if (!message) return;
    try {
      const r = await api.openAttachment(message.id, index, confirmed);
      if (r.status === "needs_confirmation") confirmOpen = { index, reason: r.reason ?? "" };
      else { confirmOpen = null; onStatus("Opened with the default application"); }
    } catch (e) { onStatus(errorText(e)); }
  }
  function done(r: ComposerResult) { reply = null; onComposerDone(r); }
</script>

{#if message}
  <article class="viewer" class:composing={reply !== null}>
    <div class="message">
      <div class="toolbar" role="toolbar" aria-label="Message actions">
        {#if isDraft}
          <button class="tbtn primary" onclick={() => startCompose("draft")}><Icon name="drafts" size={16} /> Edit draft</button>
        {:else}
          <button class="tbtn" onclick={() => startCompose("reply")} aria-pressed={reply?.mode === "reply"}><Icon name="reply" size={16} /> Reply</button>
          <button class="tbtn" onclick={() => startCompose("reply_all")} aria-pressed={reply?.mode === "reply_all"}><Icon name="replyall" size={16} /> Reply all</button>
          <button class="tbtn" onclick={() => startCompose("forward")} aria-pressed={reply?.mode === "forward"}><Icon name="forward" size={16} /> Forward</button>
        {/if}
        <span class="spacer"></span>
        <button class="tbtn" onclick={toggleSource} aria-pressed={source !== null} title="View the raw message source">
          <Icon name="code" size={16} /> {source !== null ? "Message" : "View source"}</button>
      </div>
      <h1>{message.subject || "(no subject)"}</h1>
      <div class="head">
        <div class="avatar">{initials(message.from_name, message.from_addr)}</div>
        <div class="who">
          <div class="name">{message.from_name || message.from_addr}</div>
          <div class="muted addr">{message.from_addr}</div>
          <div class="muted addr">to {message.to}{message.cc ? `, cc ${message.cc}` : ""}</div>
        </div>
        <div class="muted">{shortDate(message.date)}</div>
      </div>
      {#if source !== null}
        <!-- Raw RFC 822 source as escaped text (Svelte text node, never HTML). -->
        <pre class="source" aria-label="Message source">{source}</pre>
      {:else}
        {#if message.remote_images_blocked > 0 || message.remote_images_loaded > 0}
          <div class="notice" role="status">
            <Icon name="shield" size={16} />
            {#if message.remote_images_loaded > 0}
              <span>{message.remote_images_loaded} remote image{message.remote_images_loaded === 1 ? "" : "s"} loaded{message.remote_images_blocked ? `, ${message.remote_images_blocked} blocked (not a valid image or not allowed)` : ""}.</span>
            {:else}
              <span>{message.remote_images_blocked} remote image{message.remote_images_blocked === 1 ? "" : "s"} blocked to protect your privacy.</span>
              {#if message.remote_images_available > 0}<button class="link" onclick={() => onReload(true)}>Load remote images</button>{/if}
            {/if}
            {#if message.from_addr && message.remote_images_available > 0}
              <button class="link" onclick={trustSender}>{message.sender_trusted ? "Stop loading for this sender" : "Always load from this sender"}</button>
            {/if}
          </div>
        {/if}
        {#if message.document}
          <!-- Empty sandbox: no scripts, no same-origin, no forms, no popups, no top navigation.
               Link clicks are intercepted by the app shell (navigation policy) and confirmed. -->
          <iframe class="body" title="Message content" sandbox="" referrerpolicy="no-referrer" srcdoc={message.document}></iframe>
        {:else}
          <pre class="body text">{message.text}</pre>
        {/if}
      {/if}
      {#if message.attachments.length}
        <div class="attachments">
          {#each message.attachments as a (a.index)}
            <div class="att" class:risky={a.risky}>
              <Icon name="clip" />
              <div class="meta"><div class="aname" title={a.name}>{a.name}</div><div class="muted small">{a.mime} · {size(a.size)}{a.risky ? " · program/script" : ""}</div></div>
              <button class="mini" onclick={() => save(a.index)} title="Save…" aria-label={`Save ${a.name}`}>Save</button>
              <button class="mini" onclick={() => openAtt(a.index)} title="Open with default app" aria-label={`Open ${a.name}`}>Open</button>
            </div>
          {/each}
        </div>
      {/if}
      {#if hoverUrl}<div class="hover" role="status" aria-live="off">{hoverUrl}</div>{/if}
    </div>
    {#if reply}
      {#key reply.key}
        <div class="inline-composer">
          <Composer {account} draft={reply.draft} title={titles[reply.mode]} inline
            replaceDraft={reply.mode === "draft" ? message.id : null} onDone={done} />
        </div>
      {/key}
    {/if}
  </article>
  {#if confirmOpen}
    <div class="backdrop" role="presentation">
      <div class="dialog" role="alertdialog" aria-modal="true" aria-label="Open program?">
        <h3>⚠ Open a program or script?</h3>
        <p>{confirmOpen.reason}</p>
        <div class="actions">
          <button class="btn" onclick={() => (confirmOpen = null)}>Cancel</button>
          <button class="btn danger" onclick={() => confirmOpen && openAtt(confirmOpen.index, true)}>Open anyway</button>
        </div>
      </div>
    </div>
  {/if}
{:else}
  <div class="empty muted">Select a message to read it</div>
{/if}

<style>
  .viewer { display: flex; flex-direction: column; height: 100%; overflow: hidden; position: relative; }
  .message { display: flex; flex-direction: column; gap: 10px; padding: 12px 28px 16px; flex: 1; min-height: 0; overflow: auto; position: relative; }
  .composing .message { flex: 1 1 45%; }
  .inline-composer { flex: 1 1 55%; min-height: 300px; padding: 0 16px 14px; display: flex; flex-direction: column; }
  .toolbar { display: flex; gap: 6px; align-items: center; }
  .tbtn { display: inline-flex; gap: 6px; align-items: center; border: 1px solid var(--border); background: var(--panel-2); border-radius: 8px; padding: 6px 12px; font-size: 13px; }
  .tbtn:hover { background: var(--hover); }
  .tbtn[aria-pressed="true"] { background: var(--selected); border-color: var(--accent); }
  .tbtn.primary { background: var(--accent); color: #fff; border-color: var(--accent); }
  .spacer { flex: 1; }
  h1 { font-size: 22px; margin: 4px 0 0; font-weight: 600; }
  .head { display: flex; gap: 12px; align-items: flex-start; }
  .who { flex: 1; min-width: 0; }
  .name { font-weight: 600; }
  .addr { font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .avatar { width: 44px; height: 44px; border-radius: 50%; background: var(--accent); color: #fff; display: grid; place-items: center; font-weight: 600; flex: none; }
  .notice { display: flex; flex-wrap: wrap; gap: 8px 12px; align-items: center; background: var(--accent-soft); border-radius: 8px; padding: 8px 12px; font-size: 13px; }
  .link { border: 0; background: transparent; color: var(--unread); padding: 0; font-weight: 600; font-size: 13px; }
  .link:hover { text-decoration: underline; }
  .body { flex: 1 0 auto; width: 100%; border: 0; min-height: 360px; border-radius: 8px; background: transparent; }
  .composing .body { min-height: 260px; }
  .text { white-space: pre-wrap; word-break: break-word; font: inherit; line-height: 1.55; margin: 0; min-height: 0; }
  .source { flex: 1 0 auto; margin: 0; padding: 12px; background: var(--panel-2); border: 1px solid var(--border); border-radius: 8px;
    font: 12px/1.45 ui-monospace, "DejaVu Sans Mono", monospace; white-space: pre-wrap; word-break: break-all; }
  .attachments { display: flex; flex-wrap: wrap; gap: 10px; }
  .att { display: flex; gap: 10px; align-items: center; border: 1px solid var(--border); border-radius: 10px; padding: 8px 10px; color: var(--unread); max-width: 340px; }
  .att.risky { border-color: #e0525e; color: #e0525e; }
  .meta { color: var(--text); min-width: 0; flex: 1; }
  .aname { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .mini { border: 1px solid var(--border); background: var(--panel-2); border-radius: 6px; padding: 3px 8px; font-size: 12px; color: var(--text); }
  .mini:hover { background: var(--hover); }
  .small { font-size: 12px; }
  .hover { position: sticky; bottom: 0; align-self: flex-start; max-width: 100%; background: var(--panel-2); border: 1px solid var(--border);
    border-radius: 6px; padding: 3px 8px; font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .empty { height: 100%; display: grid; place-items: center; }
  .backdrop { position: fixed; inset: 0; background: #0008; display: grid; place-items: center; z-index: 40; }
  .dialog { width: min(480px, 92vw); background: var(--panel); border: 1px solid var(--border); border-radius: 14px; padding: 20px 22px; }
  .dialog h3 { margin: 0 0 8px; }
  .actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 14px; }
  :global(.btn.danger) { background: #c9374a; border-color: #c9374a; color: #fff; }
</style>
