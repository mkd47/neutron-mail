<script lang="ts">
  import { untrack } from "svelte";
  import { api, errorText, type AccountConfig, type Account } from "./api";

  let { onAdded, onCancel, existing }:
    { onAdded: (a: Account) => void; onCancel?: () => void; existing?: Account } = $props();

  // Editing: a copy of the existing config (password left empty = keep).
  const start = untrack(() => existing);
  const init = start ? (({ id: _id, ...c }) => c)(start) : null;
  let cfg: AccountConfig = $state(init ? { ...init } : {
    email: "", display_name: "", username: "",
    imap_host: "", imap_port: 993, imap_security: "tls",
    smtp_host: "", smtp_port: 587, smtp_security: "starttls",
    extra_ca_pem: null,
  });
  let password = $state("");
  let caText = $state(init?.extra_ca_pem ?? "");
  let busy = $state(false);
  let error = $state("");

  function guessFromEmail() {
    const domain = cfg.email.split("@")[1];
    if (!domain) return;
    if (!cfg.username) cfg.username = cfg.email;
    if (!cfg.imap_host) cfg.imap_host = `imap.${domain}`;
    if (!cfg.smtp_host) cfg.smtp_host = `smtp.${domain}`;
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true; error = "";
    try {
      const c = { ...cfg, extra_ca_pem: caText.trim() || null };
      const account = existing ? await api.updateAccount(existing.id, c, password) : await api.addAccount(c, password);
      password = "";
      onAdded(account);
    } catch (err) {
      const t = errorText(err);
      error = /AUTHENTICATIONFAILED|authentication failed/i.test(t)
        ? "Login failed: the server rejected this username and password. Nothing was saved." : t;
    } finally {
      busy = false;
    }
  }
</script>

<form class="setup" class:embedded={!!existing} onsubmit={submit}>
  {#if existing}
    <h3>Server and login</h3>
    <p class="muted">Changes are saved only after a successful login with the new settings.</p>
  {:else}
    <h2>Add an account</h2>
    <p class="muted">Connect Neutron Mail to your own IMAP/SMTP server. Connections are always encrypted
      and certificates are always verified.</p>
  {/if}
  <div class="grid">
    <label>Email<input type="email" required bind:value={cfg.email} onblur={guessFromEmail} autocomplete="email" /></label>
    <label>Display name<input bind:value={cfg.display_name} autocomplete="name" /></label>
    <label>Username<input required bind:value={cfg.username} autocomplete="username" /></label>
    <label>Password<input type="password" required={!existing} bind:value={password} autocomplete="current-password"
      placeholder={existing ? "Leave empty to keep" : ""} /></label>
  </div>
  <fieldset>
    <legend>Incoming (IMAP)</legend>
    <input class="host" required placeholder="imap.example.org" bind:value={cfg.imap_host} aria-label="IMAP host" />
    <input class="port" type="number" min="1" max="65535" required bind:value={cfg.imap_port} aria-label="IMAP port" />
    <select bind:value={cfg.imap_security} aria-label="IMAP security"
      onchange={() => (cfg.imap_port = cfg.imap_security === "tls" ? 993 : 143)}>
      <option value="tls">TLS</option><option value="starttls">STARTTLS</option>
    </select>
  </fieldset>
  <fieldset>
    <legend>Outgoing (SMTP)</legend>
    <input class="host" required placeholder="smtp.example.org" bind:value={cfg.smtp_host} aria-label="SMTP host" />
    <input class="port" type="number" min="1" max="65535" required bind:value={cfg.smtp_port} aria-label="SMTP port" />
    <select bind:value={cfg.smtp_security} aria-label="SMTP security"
      onchange={() => (cfg.smtp_port = cfg.smtp_security === "tls" ? 465 : 587)}>
      <option value="tls">TLS</option><option value="starttls">STARTTLS</option>
    </select>
  </fieldset>
  <details>
    <summary>Advanced: trust a private CA</summary>
    <p class="muted">For self-hosted servers with their own certificate authority. Paste the CA certificate (PEM).
      This adds trust; it never disables verification.</p>
    <textarea rows="4" bind:value={caText} placeholder="-----BEGIN CERTIFICATE-----"></textarea>
  </details>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <div class="actions">
    {#if onCancel}<button type="button" class="btn" onclick={onCancel}>Cancel</button>{/if}
    <button class="btn primary" disabled={busy}>{busy ? "Connecting…" : existing ? "Verify and save" : "Add account"}</button>
  </div>
</form>

<style>
  .setup { max-width: 640px; margin: 40px auto; background: var(--panel); border: 1px solid var(--border);
    border-radius: 16px; padding: 28px; display: flex; flex-direction: column; gap: 14px; }
  h2, h3 { margin: 0; }
  .setup.embedded { margin: 0; max-width: none; padding: 20px; }
  p { margin: 0; }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  label { display: flex; flex-direction: column; gap: 6px; font-size: 13px; color: var(--muted); }
  fieldset { border: 1px solid var(--border); border-radius: 10px; display: flex; gap: 8px; padding: 10px 12px 12px; }
  legend { color: var(--muted); font-size: 13px; padding: 0 6px; }
  .host { flex: 1; } .port { width: 90px; }
  textarea { width: 100%; margin-top: 8px; font-family: monospace; font-size: 12px; }
  summary { cursor: pointer; color: var(--muted); }
  .actions { display: flex; justify-content: flex-end; gap: 8px; }
</style>
