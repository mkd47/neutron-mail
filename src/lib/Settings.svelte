<script lang="ts">
  // Per-account settings: server/login (re-verified on save), signature,
  // sync window, undo send, automatic replies, trusted remote-image senders,
  // and account removal.
  import { onMount } from "svelte";
  import AccountSetup from "./AccountSetup.svelte";
  import RichEditor from "./RichEditor.svelte";
  import AutoReply from "./AutoReply.svelte";
  import { Icon } from "@neutron/ui";
  import { api, errorText, type Account, type AccountSettings, type MailAddress } from "./api";
  import { UNDO_SEND_CHOICES, setUndoSendSeconds, undoSendSeconds } from "./prefs";

  let { account, onClose, onUpdated, onRemoved }:
    { account: Account; onClose: () => void; onUpdated: (a: Account) => void; onRemoved: (id: number) => void } = $props();

  let settings = $state<AccountSettings | null>(null);
  let signature = $state("");
  let syncDays = $state(0);
  let identities = $state<MailAddress[]>([]);
  let aliasSigs = $state<Record<string, string>>({});
  // Which alias has its signature editor open (by row index).
  let sigOpen = $state<number | null>(null);
  let undoDelay = $state(undoSendSeconds());
  let senders = $state<string[]>([]);
  let prefsBusy = $state(false);
  let prefsMsg = $state<{ text: string; error: boolean } | null>(null);
  let serverMsg = $state("");
  let confirmRemove = $state(false);
  let removeBusy = $state(false);
  let removeError = $state("");
  let loadError = $state("");

  const windows = [
    { days: 0, label: "All mail" }, { days: 30, label: "Last 30 days" }, { days: 90, label: "Last 90 days" },
    { days: 365, label: "Last year" }, { days: 730, label: "Last 2 years" },
  ];

  onMount(async () => {
    try {
      const [s, list] = await Promise.all([api.getAccountSettings(account.id), api.listImageSenders(account.id)]);
      settings = s; signature = s.signature_html; syncDays = s.sync_days; senders = list;
      identities = (s.identities ?? []).map((i) => ({ ...i }));
      aliasSigs = { ...(s.identity_signatures ?? {}) };
    } catch (e) { loadError = errorText(e); }
  });

  async function savePrefs() {
    prefsBusy = true; prefsMsg = null;
    try {
      const kept = identities.filter((i) => i.email.trim());
      const identity_signatures = Object.fromEntries(kept
        .map((i) => [i.email.trim().toLowerCase(), aliasSigs[i.email.trim().toLowerCase()] ?? ""] as const)
        .filter(([, h]) => h.replace(/<[^>]*>|&nbsp;/g, "").trim() || h.includes("<img")));
      const saved = await api.setAccountSettings(account.id, { signature_html: signature, sync_days: syncDays,
        identities: kept, identity_signatures });
      identities = saved.identities.map((i) => ({ ...i }));
      aliasSigs = { ...(saved.identity_signatures ?? {}) };
      const narrowed = settings !== null && saved.sync_days !== 0 && (settings.sync_days === 0 || saved.sync_days < settings.sync_days);
      settings = saved;
      prefsMsg = { text: narrowed ? "Saved. Older mail will be dropped from the local cache at the next sync (it stays on the server)." : "Saved.", error: false };
    } catch (e) { prefsMsg = { text: errorText(e), error: true }; }
    finally { prefsBusy = false; }
  }

  async function untrust(email: string) {
    try {
      await api.setImageSenderTrust(account.id, email, false);
      senders = senders.filter((s) => s !== email);
    } catch (e) { prefsMsg = { text: errorText(e), error: true }; }
  }

  async function remove() {
    removeBusy = true; removeError = "";
    try { await api.removeAccount(account.id); onRemoved(account.id); }
    catch (e) { removeError = errorText(e); removeBusy = false; }
  }
</script>

<div class="settings">
  <header>
    <h2><Icon name="settings" /> Account settings <span class="muted">{account.email}</span></h2>
    <button class="btn" onclick={onClose}>Done</button>
  </header>
  {#if loadError}<p class="error" role="alert">{loadError}</p>{/if}

  <section>
    {#key account.id}
      <AccountSetup existing={account} onAdded={(a) => { serverMsg = "Login verified and settings saved."; onUpdated(a); }} />
    {/key}
    {#if serverMsg}<p class="ok" role="status">{serverMsg}</p>{/if}
  </section>

  {#if settings}
    <section class="card">
      <h3>Signature</h3>
      <p class="muted">Added below your text in new mail, replies and forwards. Remote images aren't allowed.</p>
      <div class="sig"><RichEditor html={signature} onInput={(h) => (signature = h)} onError={(m) => (prefsMsg = { text: m, error: true })} /></div>
      <h3>Sender addresses</h3>
      <p class="muted">Aliases you can send from with this account, besides {account.email}. Your mail server must allow
        sending as these addresses. Replies use the alias the message was sent to.</p>
      <div class="ids">
        {#each identities as id, i (i)}
          {@const key = id.email.trim().toLowerCase()}
          <div class="id-row">
            <input placeholder="Name (optional)" bind:value={id.name} maxlength="200" aria-label={`Alias ${i + 1} name`} />
            <input placeholder="alias@example.com" type="email" bind:value={id.email} maxlength="320" aria-label={`Alias ${i + 1} address`} />
            <button class="btn sm" onclick={() => (sigOpen = sigOpen === i ? null : i)} disabled={!key} aria-expanded={sigOpen === i}
              title="Signature for mail sent from this address">{aliasSigs[key] ? "Signature ✓" : "Signature"}</button>
            <button class="btn sm" onclick={() => { identities = identities.filter((_, j) => j !== i); sigOpen = null; }} aria-label={`Remove alias ${i + 1}`}>Remove</button>
          </div>
          {#if sigOpen === i && key}
            <div class="alias-sig">
              <p class="muted">Signature for mail from {key}. Leave empty to use the account signature above.</p>
              {#key key}
                <div class="sig"><RichEditor html={aliasSigs[key] ?? ""} onInput={(h) => (aliasSigs = { ...aliasSigs, [key]: h })}
                  onError={(m) => (prefsMsg = { text: m, error: true })} /></div>
              {/key}
            </div>
          {/if}
        {/each}
        {#if identities.length < 20}
          <button class="btn sm add" onclick={() => (identities = [...identities, { name: "", email: "" }])}>+ Add sender address</button>
        {/if}
      </div>
      <h3>Sync window</h3>
      <p class="muted">Only download and keep mail from this period on this computer. Nothing is deleted on the server.</p>
      <select bind:value={syncDays} aria-label="Sync window">
        {#each windows as w (w.days)}<option value={w.days}>{w.label}</option>{/each}
      </select>
      <div class="actions">
        {#if prefsMsg}<span class:error={prefsMsg.error} class:ok={!prefsMsg.error} role="status">{prefsMsg.text}</span>{/if}
        <button class="btn primary" onclick={savePrefs} disabled={prefsBusy}>{prefsBusy ? "Saving…" : "Save signature, addresses and sync window"}</button>
      </div>
    </section>

    <section class="card">
      <h3>Undo send</h3>
      <p class="muted">After you press Send, the message waits this long so you can still undo it (all accounts on this computer).</p>
      <select value={undoDelay} onchange={(e) => { undoDelay = Number((e.currentTarget as HTMLSelectElement).value); setUndoSendSeconds(undoDelay); }}
        aria-label="Undo send delay">
        {#each UNDO_SEND_CHOICES as s (s)}<option value={s}>{s === 0 ? "Off: send immediately" : `${s} seconds`}</option>{/each}
      </select>
    </section>

    {#key account.id}<AutoReply {account} />{/key}

    <section class="card">
      <h3>Remote images</h3>
      <p class="muted">Senders whose remote images load automatically. Everyone else's are blocked until you allow them.</p>
      {#if senders.length}
        <ul class="senders">
          {#each senders as s (s)}
            <li><span>{s}</span><button class="btn small" onclick={() => untrust(s)} aria-label={`Stop trusting ${s}`}>Remove</button></li>
          {/each}
        </ul>
      {:else}<p class="muted">No trusted senders.</p>{/if}
    </section>
  {/if}

  <section class="card danger">
    <h3>Remove account</h3>
    <p class="muted">Removes the account from Neutron Mail: its password is deleted from the keyring and its cached mail
      is deleted from this computer. Nothing is deleted on the server.</p>
    {#if confirmRemove}
      <div class="actions">
        <span>Remove <strong>{account.email}</strong>?</span>
        <button class="btn" onclick={() => (confirmRemove = false)} disabled={removeBusy}>Cancel</button>
        <button class="btn danger-btn" onclick={remove} disabled={removeBusy}>{removeBusy ? "Removing…" : "Remove"}</button>
      </div>
    {:else}
      <div class="actions"><button class="btn danger-btn" onclick={() => (confirmRemove = true)}>Remove account…</button></div>
    {/if}
    {#if removeError}<p class="error" role="alert">{removeError}</p>{/if}
  </section>
</div>

<style>
  .settings { max-width: 760px; margin: 24px auto 48px; display: flex; flex-direction: column; gap: 16px; }
  header { display: flex; align-items: center; justify-content: space-between; }
  h2 { margin: 0; display: flex; align-items: center; gap: 8px; }
  h2 .muted { font-size: 14px; font-weight: normal; }
  h3 { margin: 0; }
  p { margin: 0; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: 16px; padding: 20px;
    display: flex; flex-direction: column; gap: 10px; }
  .ids { display: flex; flex-direction: column; gap: 6px; align-items: flex-start; }
  .id-row { display: flex; gap: 6px; width: 100%; }
  .id-row input { flex: 1; min-width: 0; }
  .alias-sig { width: 100%; display: flex; flex-direction: column; gap: 6px; padding: 8px 0 4px 12px; border-left: 2px solid var(--border); }
  .sig { border: 1px solid var(--border); border-radius: 10px; min-height: 120px; display: flex; flex-direction: column; padding: 0 10px; }
  select { align-self: flex-start; }
  .actions button { white-space: nowrap; }
  .actions span { flex: 1; text-align: right; }
  .actions { display: flex; justify-content: flex-end; align-items: center; gap: 8px; }
  .senders { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
  .senders li { display: flex; justify-content: space-between; align-items: center; padding: 6px 10px;
    border: 1px solid var(--border); border-radius: 8px; }
  .btn.small { padding: 4px 10px; font-size: 12px; }
  .ok { color: var(--accent); }
  .error { color: var(--danger, #d33); }
  .danger { border-color: color-mix(in srgb, var(--danger, #d33) 40%, var(--border)); }
  .danger-btn, .danger-btn:hover:not(:disabled) { background: var(--danger, #d33); color: white; border-color: transparent; }
  .danger-btn:hover:not(:disabled) { filter: brightness(0.92); }
  .danger-btn:disabled { opacity: 0.6; }
</style>
