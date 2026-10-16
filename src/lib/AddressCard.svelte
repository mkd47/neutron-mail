<script lang="ts">
  // Popover for one address in the reading pane: who it is, and actions
  // (write to them, copy, add to GNOME Contacts, trust their remote images).
  import { onMount } from "svelte";
  import { Icon } from "@neutron/ui";
  import { initials } from "./format";
  import { api, errorText, type AddressInfo, type MailAddress } from "./api";

  let { address, accountId, left = 0, onClose, onCompose, onTrustChanged, onStatus }: {
    address: MailAddress; accountId: number; left?: number;
    onClose: () => void; onCompose: (a: MailAddress) => void;
    onTrustChanged: (email: string, trusted: boolean) => void; onStatus: (s: string, warn?: boolean) => void;
  } = $props();

  let info = $state<AddressInfo | null>(null);
  let busy = $state<"" | "contact" | "trust">("");
  let alive = true;
  let card: HTMLElement | undefined = $state();

  onMount(() => {
    api.addressInfo(accountId, address.email)
      .then((i) => { if (alive) info = i; })
      .catch(() => { if (alive) info = { contacts_available: false, in_contacts: false, trusted: false }; });
    card?.focus();
    const away = (e: PointerEvent) => { if (card && !card.contains(e.target as Node)) onClose(); };
    // Defer so the click that opened the card doesn't close it.
    const t = setTimeout(() => document.addEventListener("pointerdown", away), 0);
    return () => { alive = false; clearTimeout(t); document.removeEventListener("pointerdown", away); };
  });

  async function copy() {
    try { await navigator.clipboard.writeText(address.email); }
    catch {
      // Older WebKitGTK without async clipboard: fall back to a selection copy.
      const t = document.createElement("textarea");
      t.value = address.email; t.style.position = "fixed"; t.style.opacity = "0";
      document.body.appendChild(t); t.select(); document.execCommand("copy"); t.remove();
    }
    onStatus(`Copied ${address.email}`);
    onClose();
  }
  async function addContact() {
    if (busy) return;
    busy = "contact";
    try {
      await api.addContact(address.name, address.email);
      if (!alive) return;
      if (info) info.in_contacts = true;
      onStatus(`Added ${address.name || address.email} to Contacts`);
    } catch (e) { if (alive) onStatus(errorText(e), true); }
    finally { if (alive) busy = ""; }
  }
  async function toggleTrust() {
    if (busy || !info) return;
    const trusted = !info.trusted;
    busy = "trust";
    try {
      await api.setImageSenderTrust(accountId, address.email, trusted);
      if (!alive) return;
      info.trusted = trusted;
      onTrustChanged(address.email, trusted);
      onStatus(trusted ? `Remote images from ${address.email} will load automatically` : `Remote images from ${address.email} are blocked again`);
    } catch (e) { if (alive) onStatus(errorText(e), true); }
    finally { if (alive) busy = ""; }
  }
</script>

<div class="card" role="dialog" aria-label={`Contact card for ${address.email}`} tabindex="-1" bind:this={card}
  style:left={`${left}px`} onkeydown={(e) => { if (e.key === "Escape") { e.stopPropagation(); onClose(); } }}>
  <div class="who">
    <div class="avatar">{initials(address.name, address.email)}</div>
    <div class="text">
      {#if address.name}<div class="name">{address.name}</div>{/if}
      <div class="email" title={address.email}>{address.email}</div>
    </div>
  </div>
  <div class="actions">
    <button class="row" onclick={() => { onCompose(address); onClose(); }}><Icon name="compose" size={15} /> New message</button>
    <button class="row" onclick={copy}><Icon name="clip" size={15} /> Copy address</button>
    {#if info === null}
      <div class="row muted">Checking contacts…</div>
    {:else if info.in_contacts}
      <div class="row muted"><Icon name="check" size={15} /> In your contacts</div>
    {:else if info.contacts_available}
      <button class="row" onclick={addContact} disabled={busy !== ""}><Icon name="plus" size={15} /> {busy === "contact" ? "Adding…" : "Add to contacts"}</button>
    {:else}
      <div class="row muted" title="Install and start GNOME Contacts (evolution-data-server) to add contacts">Contacts unavailable</div>
    {/if}
    {#if info}
      <button class="row" onclick={toggleTrust} disabled={busy !== ""} aria-pressed={info.trusted}>
        <Icon name="shield" size={15} /> {info.trusted ? "Trusted: images load automatically · Stop" : "Trust sender (always load images)"}</button>
    {/if}
  </div>
</div>

<style>
  .card { position: absolute; top: calc(100% + 6px); z-index: 45; width: 300px; max-width: calc(100vw - 32px); background: var(--panel);
    border: 1px solid var(--border); border-radius: 12px; padding: 12px; box-shadow: 0 12px 32px #0006; outline: none; }
  .who { display: flex; gap: 10px; align-items: center; padding-bottom: 10px; border-bottom: 1px solid var(--border); }
  .avatar { width: 40px; height: 40px; border-radius: 50%; background: var(--accent); color: #fff; display: grid; place-items: center; font-weight: 600; flex: none; }
  .text { min-width: 0; }
  .name { font-weight: 600; }
  .email { font-size: 13px; color: var(--muted); word-break: break-all; user-select: text; }
  .actions { display: flex; flex-direction: column; gap: 2px; padding-top: 8px; }
  .row { display: flex; gap: 8px; align-items: center; text-align: left; border: 0; background: transparent; padding: 7px 8px; border-radius: 8px; font-size: 13px; color: var(--text); }
  button.row:hover:not(:disabled) { background: var(--hover); }
  .row[aria-pressed="true"] { color: var(--unread); }
  .muted { color: var(--muted); }
</style>
