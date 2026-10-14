<script lang="ts">
  import { api, errorText, type AccountConfig, type Account } from "./api";

  let { onAdded, onCancel }: { onAdded: (a: Account) => void; onCancel?: () => void } = $props();

  let cfg: AccountConfig = $state({
    email: "", display_name: "", username: "",
    imap_host: "", imap_port: 993, imap_security: "tls",
    smtp_host: "", smtp_port: 587, smtp_security: "starttls",
    extra_ca_pem: null,
  });
  let password = $state("");
  let caText = $state("");
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
      const account = await api.addAccount({ ...cfg, extra_ca_pem: caText.trim() || null }, password);
      password = "";
      onAdded(account);
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }
</script>

<form class="setup" onsubmit={submit}>
  <h2>Add an account</h2>
  <p class="muted">Connect Neutron Mail to your own IMAP/SMTP server. Connections are always encrypted
    and certificates are always verified.</p>
  <div class="grid">
    <label>Email<input type="email" required bind:value={cfg.email} onblur={guessFromEmail} autocomplete="email" /></label>
    <label>Display name<input bind:value={cfg.display_name} autocomplete="name" /></label>
    <label>Username<input required bind:value={cfg.username} autocomplete="username" /></label>
    <label>Password<input type="password" required bind:value={password} autocomplete="current-password" /></label>
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
    <button class="btn primary" disabled={busy}>{busy ? "Connecting…" : "Add account"}</button>
  </div>
</form>

<style>
  .setup { max-width: 640px; margin: 40px auto; background: var(--panel); border: 1px solid var(--border);
    border-radius: 16px; padding: 28px; display: flex; flex-direction: column; gap: 14px; }
  h2 { margin: 0; }
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
