<script lang="ts">
  // Standalone "new message" window (modal). Reply/forward use the inline
  // composer in the preview pane instead.
  import Composer from "./Composer.svelte";
  import type { Account, ComposeDraft, ComposerResult } from "./api";

  let { account, draft, onClose }: { account: Account; draft: ComposeDraft; onClose: (r: ComposerResult) => void } = $props();
</script>

<div class="backdrop" role="presentation">
  <div class="window" role="dialog" aria-modal="true" aria-label="New message">
    <Composer {account} {draft} title="New message" onDone={onClose} />
  </div>
</div>

<style>
  .backdrop { position: fixed; inset: 0; background: #0008; display: grid; place-items: center; z-index: 20; }
  .window { width: min(780px, 94vw); height: min(640px, 90vh); background: var(--panel); border: 1px solid var(--border);
    border-radius: 16px; padding: 16px 20px; box-shadow: 0 20px 60px #0006; display: flex; flex-direction: column; }
</style>
