<script lang="ts">
  // Confirmation before leaving the app: shows the real destination host,
  // unwrapped tracking redirects, and lets the user open the cleaned or the
  // original link in the default browser (opener plugin, http/https only).
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { Icon } from "@neutron/ui";
  import { errorText, type LinkInfo } from "./api";

  let { link, onClose }: { link: LinkInfo; onClose: (opened: string | null) => void } = $props();
  let error = $state("");
  const cleaned = $derived(link.clean !== link.original);

  async function open(url: string) {
    try { await openUrl(url); onClose(url); } catch (e) { error = errorText(e); }
  }
  function onKey(e: KeyboardEvent) { if (e.key === "Escape") onClose(null); }
</script>

<svelte:window onkeydown={onKey} />
<div class="backdrop" role="presentation">
  <div class="dialog" role="dialog" aria-modal="true" aria-label="Open link">
    <h3><Icon name="external" size={18} /> Open link in your browser?</h3>
    <div class="host">{link.host}</div>
    <div class="url" title={link.clean}>{link.clean}</div>
    {#if link.note}<p class="note"><Icon name="shield" size={14} /> {link.note}</p>{/if}
    {#if cleaned}
      <details><summary>Original link</summary><div class="url small">{link.original}</div></details>
    {/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions">
      <button class="btn" onclick={() => onClose(null)}>Cancel</button>
      {#if cleaned}<button class="btn" onclick={() => open(link.original)}>Open original</button>{/if}
      <button class="btn primary" onclick={() => open(link.clean)}>{cleaned ? "Open clean link" : "Open link"}</button>
    </div>
  </div>
</div>

<style>
  .backdrop { position: fixed; inset: 0; background: #0008; display: grid; place-items: center; z-index: 50; }
  .dialog { width: min(560px, 92vw); background: var(--panel); border: 1px solid var(--border); border-radius: 14px; padding: 20px 22px; box-shadow: 0 20px 60px #0006; }
  h3 { margin: 0 0 12px; display: flex; gap: 8px; align-items: center; }
  .host { font-size: 18px; font-weight: 700; }
  .url { font: 12px/1.4 ui-monospace, "DejaVu Sans Mono", monospace; color: var(--muted); word-break: break-all; margin-top: 4px;
    background: var(--panel-2); border: 1px solid var(--border); border-radius: 8px; padding: 6px 8px; max-height: 120px; overflow: auto; }
  .small { font-size: 11px; }
  .note { display: flex; gap: 6px; align-items: center; color: var(--unread); font-size: 13px; margin: 10px 0 0; }
  details { margin-top: 10px; font-size: 13px; color: var(--muted); }
  summary { cursor: pointer; }
  .actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; }
</style>
