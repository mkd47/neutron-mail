<script lang="ts">
  import Icon from "./Icon.svelte";
  import { initials, shortDate, size } from "./format";
  import type { MessageDetail } from "./api";

  let { message, onReply }: { message: (MessageDetail & { document: string | null }) | null; onReply: () => void } = $props();
</script>

{#if message}
  <article class="viewer">
    <div class="toolbar">
      <button class="icon-btn" onclick={onReply} title="Reply" aria-label="Reply">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
          stroke-linecap="round" stroke-linejoin="round"><path d="M9 14L4 9l5-5 M4 9h10a6 6 0 0 1 6 6v5" /></svg>
      </button>
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
    {#if message.remote_images_blocked > 0}
      <div class="notice" role="status">
        <Icon name="shield" size={16} />
        {message.remote_images_blocked} remote image{message.remote_images_blocked === 1 ? "" : "s"} blocked to protect your privacy.
      </div>
    {/if}
    {#if message.document}
      <!-- Empty sandbox: no scripts, no same-origin, no forms, no popups, no top navigation.
           The document additionally carries a restrictive CSP (img-src data: only). -->
      <iframe class="body" title="Message content" sandbox="" referrerpolicy="no-referrer" srcdoc={message.document}></iframe>
    {:else}
      <pre class="body text">{message.text}</pre>
    {/if}
    {#if message.attachments.length}
      <div class="attachments">
        {#each message.attachments as a (a.name + a.size)}
          <div class="att"><Icon name="clip" /><div><div>{a.name}</div><div class="muted small">{a.mime} · {size(a.size)}</div></div></div>
        {/each}
      </div>
    {/if}
  </article>
{:else}
  <div class="empty muted">Select a message to read it</div>
{/if}

<style>
  .viewer { display: flex; flex-direction: column; height: 100%; padding: 12px 28px 20px; gap: 12px; overflow: hidden; }
  .toolbar { display: flex; justify-content: flex-end; }
  h1 { font-size: 22px; margin: 0; font-weight: 600; }
  .head { display: flex; gap: 12px; align-items: flex-start; }
  .who { flex: 1; min-width: 0; }
  .name { font-weight: 600; }
  .addr { font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .avatar { width: 44px; height: 44px; border-radius: 50%; background: var(--accent); color: #fff;
    display: grid; place-items: center; font-weight: 600; flex: none; }
  .notice { display: flex; gap: 8px; align-items: center; background: var(--accent-soft); border-radius: 8px;
    padding: 8px 12px; font-size: 13px; }
  .body { flex: 1; width: 100%; border: 0; min-height: 200px; border-radius: 8px; background: transparent; }
  .text { white-space: pre-wrap; word-break: break-word; font: inherit; line-height: 1.55; overflow: auto; margin: 0; }
  .attachments { display: flex; flex-wrap: wrap; gap: 10px; }
  .att { display: flex; gap: 10px; align-items: center; border: 1px solid var(--border); border-radius: 10px;
    padding: 10px 14px; color: var(--unread); }
  .att > div { color: var(--text); }
  .small { font-size: 12px; }
  .empty { height: 100%; display: grid; place-items: center; }
</style>
