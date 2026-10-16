<script lang="ts">
  // Standalone "new message" window (modal). Reply/forward use the inline
  // composer in the preview pane instead. The window can be resized from
  // its corner or maximized; the size is remembered for next time.
  import { Icon } from "@neutron/ui";
  import Composer from "./Composer.svelte";
  import type { Account, ComposeDraft, ComposerResult } from "./api";

  let { account, draft, onClose }: { account: Account; draft: ComposeDraft; onClose: (r: ComposerResult) => void } = $props();

  const MIN_W = 480, MIN_H = 380;
  function saved(): { w: number; h: number; max: boolean } {
    try {
      const v = JSON.parse(localStorage.getItem("composeSize") ?? "null");
      if (v && typeof v.w === "number" && typeof v.h === "number") return { w: v.w, h: v.h, max: !!v.max };
    } catch { /* storage unavailable or corrupt: use defaults */ }
    return { w: 780, h: 640, max: false };
  }
  const initial = saved();
  let width = $state(initial.w);
  let height = $state(initial.h);
  let maximized = $state(initial.max);
  function remember() {
    try { localStorage.setItem("composeSize", JSON.stringify({ w: width, h: height, max: maximized })); } catch { /* ignore */ }
  }

  let drag: { x: number; y: number; w: number; h: number } | null = null;
  function startResize(e: PointerEvent) {
    if (maximized) return;
    drag = { x: e.clientX, y: e.clientY, w: width, h: height };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }
  function onResize(e: PointerEvent) {
    if (!drag) return;
    // The window is centered, so it grows on both sides: double the delta
    // to keep the corner under the pointer.
    width = Math.round(Math.min(window.innerWidth - 16, Math.max(MIN_W, drag.w + 2 * (e.clientX - drag.x))));
    height = Math.round(Math.min(window.innerHeight - 16, Math.max(MIN_H, drag.h + 2 * (e.clientY - drag.y))));
  }
  function endResize() { if (drag) { drag = null; remember(); } }
  function toggleMax() { maximized = !maximized; remember(); }
</script>

<div class="backdrop" role="presentation">
  <div class="window" class:maximized role="dialog" aria-modal="true" aria-label="New message"
    style:width={maximized ? null : `min(${width}px, calc(100vw - 16px))`} style:height={maximized ? null : `min(${height}px, calc(100vh - 16px))`}>
    <button class="max" onclick={toggleMax} title={maximized ? "Restore size" : "Maximize"} aria-pressed={maximized}
      aria-label={maximized ? "Restore window size" : "Maximize window"}>{maximized ? "❐" : "□"}</button>
    <Composer {account} {draft} title="New message" onDone={onClose} />
    {#if !maximized}
      <div class="grip" role="separator" aria-label="Resize window" title="Drag to resize"
        onpointerdown={startResize} onpointermove={onResize} onpointerup={endResize} onpointercancel={endResize}>
        <Icon name="chevron" size={12} />
      </div>
    {/if}
  </div>
</div>

<style>
  .backdrop { position: fixed; inset: 0; background: #0008; display: grid; place-items: center; z-index: 20; }
  .window { position: relative; background: var(--panel); border: 1px solid var(--border);
    border-radius: 16px; padding: 16px 20px; box-shadow: 0 20px 60px #0006; display: flex; flex-direction: column; }
  .window.maximized { width: calc(100vw - 16px); height: calc(100vh - 16px); border-radius: 10px; }
  .max { position: absolute; top: 10px; right: 12px; z-index: 2; width: 28px; height: 28px; border: 1px solid var(--border);
    background: var(--panel-2); border-radius: 6px; font-size: 14px; line-height: 1; color: var(--text); display: grid; place-items: center; }
  .max:hover { background: var(--hover); }
  /* Keep the header's Cc / Bcc link clear of the maximize button. */
  .window :global(.composer > header) { padding-right: 36px; }
  .grip { position: absolute; right: 2px; bottom: 2px; width: 18px; height: 18px; cursor: nwse-resize; color: var(--muted);
    display: grid; place-items: center; transform: rotate(-45deg); touch-action: none; }
</style>
