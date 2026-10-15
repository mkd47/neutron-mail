<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { api, errorText, type AccountConfig, type Account, type Provider, type AuthMethod } from "./api";

  let { onAdded, onCancel, existing }:
    { onAdded: (a: Account) => void; onCancel?: () => void; existing?: Account } = $props();

  // Editing: a copy of the existing config (password left empty = keep).
  const start = untrack(() => existing);
  const init = start ? (({ id: _id, ...c }) => c)(start) : null;
  let cfg: AccountConfig = $state(init ? { auth: "password", ...init } : {
    email: "", display_name: "", username: "",
    imap_host: "", imap_port: 993, imap_security: "tls",
    smtp_host: "", smtp_port: 587, smtp_security: "starttls",
    extra_ca_pem: null, auth: "password",
  });
  let password = $state("");
  let caText = $state(init?.extra_ca_pem ?? "");
  let busy = $state(false);
  let error = $state("");
  let note = $state("");

  let providers: Provider[] = $state([]);
  // "" = no tile chosen yet; "other" = manual settings.
  let tile = $state(start ? "other" : "");
  let advanced = $state(!!start);
  let userTouchedTile = false;
  const provider = $derived(providers.find((p) => p.id === tile));
  const oauth = $derived(cfg.auth === "oauth_google" || cfg.auth === "oauth_microsoft");
  const manual = $derived(tile === "other" || advanced);

  onMount(async () => {
    try { providers = await api.listProviders(); } catch (e) { error = errorText(e); }
    if (existing) {
      const p = providers.find((p) => p.imap_host === cfg.imap_host && p.smtp_host === cfg.smtp_host);
      if (p) { tile = p.id; advanced = false; }
    }
  });

  function choose(id: string, byUser = true) {
    if (byUser) userTouchedTile = true;
    tile = id; error = ""; note = "";
    const p = providers.find((x) => x.id === id);
    if (!p) { advanced = true; cfg.auth = "password"; return; }
    advanced = false;
    Object.assign(cfg, {
      imap_host: p.imap_host, imap_port: p.imap_port, imap_security: p.imap_security,
      smtp_host: p.smtp_host, smtp_port: p.smtp_port, smtp_security: p.smtp_security,
    });
    // OAuth when this build has a client ID, else the password path if any.
    const preferred: AuthMethod = p.auth.find((m) => m !== "password" && p.oauth_ready)
      ?? (p.auth.includes("password") ? "password" : p.auth[0]);
    cfg.auth = existing ? (cfg.auth ?? "password") : preferred;
    applyConsumerSmtp(p);
    if (cfg.email) cfg.username = cfg.email;
  }

  // Outlook.com consumer addresses send via smtp-mail.outlook.com.
  function applyConsumerSmtp(p: Provider) {
    const domain = cfg.email.trim().toLowerCase().split("@")[1] ?? "";
    cfg.smtp_host = p.consumer_smtp_host && p.domains.includes(domain) ? p.consumer_smtp_host : p.smtp_host;
  }

  async function emailChanged() {
    const email = cfg.email.trim();
    if (!email.includes("@")) return;
    if (!cfg.username || tile !== "other") cfg.username = email;
    if (existing) return;
    if (provider && !advanced) applyConsumerSmtp(provider);
    const id = await api.detectProvider(email).catch(() => null);
    if (id && (!userTouchedTile || tile === "" || tile === "other")) { choose(id, false); note = `Detected ${providers.find((p) => p.id === id)?.name}.`; return; }
    if (!id && (tile === "" || tile === "other") && !cfg.imap_host) {
      note = "Looking up server settings…";
      try {
        const d = await api.autoconfigLookup(email);
        tile = "other"; advanced = true;
        Object.assign(cfg, {
          imap_host: d.imap_host, imap_port: d.imap_port, imap_security: d.imap_security,
          smtp_host: d.smtp_host, smtp_port: d.smtp_port, smtp_security: d.smtp_security,
          username: d.username,
        });
        note = `Settings found via ${new URL(d.source).host}. Check them before continuing.`;
      } catch {
        tile = "other"; advanced = true;
        const domain = email.split("@")[1];
        cfg.imap_host ||= `imap.${domain}`; cfg.smtp_host ||= `smtp.${domain}`;
        note = "No published settings for this domain; guessed host names below.";
      }
    }
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true; error = "";
    try {
      const c = { ...cfg, email: cfg.email.trim(), username: (cfg.username || cfg.email).trim(), extra_ca_pem: caText.trim() || null };
      let account: Account;
      if (oauth) {
        note = "Finish signing in in your browser…";
        account = await api.oauthSignIn(c, existing?.id ?? null);
      } else {
        account = existing ? await api.updateAccount(existing.id, c, password) : await api.addAccount(c, password);
      }
      password = ""; note = "";
      onAdded(account);
    } catch (err) {
      const t = errorText(err);
      note = "";
      error = /AUTHENTICATIONFAILED|authentication failed/i.test(t)
        ? `Login failed: the server rejected this username and ${provider?.password_label.toLowerCase() || "password"}. Nothing was saved.` : t;
    } finally {
      busy = false;
    }
  }

  function open(e: MouseEvent, url: string) {
    e.preventDefault();
    openUrl(url).catch((err) => (error = errorText(err)));
  }

  const authLabel = (m: AuthMethod) =>
    m === "password" ? (provider?.password_label || "Password")
      : m === "oauth_google" ? "Sign in with Google" : "Sign in with Microsoft";
</script>

<form class="setup" class:embedded={!!existing} onsubmit={submit}>
  {#if existing}
    <h3>Server and login</h3>
    <p class="muted">Changes are saved only after a successful login with the new settings.</p>
  {:else}
    <h2>Add an account</h2>
    <p class="muted">Pick your provider, or enter your address and Neutron Mail will look it up.
      Connections are always encrypted and certificates are always verified.</p>
  {/if}

  {#if !existing}
    <div class="tiles" role="radiogroup" aria-label="Provider">
      {#each providers as p (p.id)}
        <button type="button" role="radio" aria-checked={tile === p.id} class="tile" class:on={tile === p.id}
          onclick={() => choose(p.id)}>{p.name}</button>
      {/each}
      <button type="button" role="radio" aria-checked={tile === "other"} class="tile" class:on={tile === "other"}
        onclick={() => choose("other")}>Other / manual</button>
    </div>
  {/if}

  <div class="grid">
    <label>Email<input type="email" required bind:value={cfg.email} onblur={emailChanged} autocomplete="email" /></label>
    <label>Display name<input bind:value={cfg.display_name} autocomplete="name" /></label>
    {#if manual}
      <label>Username<input required bind:value={cfg.username} autocomplete="username" /></label>
    {/if}
    {#if !oauth}
      <label>{provider?.password_label || "Password"}<input type="password" required={!existing} bind:value={password}
        autocomplete="current-password" placeholder={existing ? "Leave empty to keep" : ""} /></label>
    {/if}
  </div>

  {#if provider}
    {#if provider.auth.length > 1}
      <div class="auth" role="radiogroup" aria-label="Sign-in method">
        {#each provider.auth as m (m)}
          <label class="inline"><input type="radio" bind:group={cfg.auth} value={m} disabled={!!existing} /> {authLabel(m)}</label>
        {/each}
      </div>
    {/if}
    <p class="hint">{provider.hint}
      {#if provider.help_url !== provider.source}<a href={provider.help_url} onclick={(e) => open(e, provider.help_url)}>
        {provider.auth.includes("password") ? `Where to get your ${provider.password_label.toLowerCase()}` : "How to turn on IMAP"}</a>.{/if}
      <a href={provider.source} onclick={(e) => open(e, provider.source)}>Official server settings</a>.</p>
    {#if oauth && !provider.oauth_ready}
      <p class="error" role="alert">Sign-in with {provider.name} is not configured in this build (no OAuth client ID).
        {#if provider.auth.includes("password")}Use an app password instead, or see{:else}See{/if} the README, "OAuth setup".</p>
    {/if}
  {/if}
  {#if note}<p class="muted" aria-live="polite">{note}</p>{/if}

  {#if tile !== ""}
    {#if provider}
      <label class="inline"><input type="checkbox" bind:checked={advanced} /> Advanced: edit server settings</label>
    {/if}
    {#if manual}
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
        <summary>Trust a private CA</summary>
        <p class="muted">For self-hosted servers with their own certificate authority. Paste the CA certificate (PEM).
          This adds trust; it never disables verification.</p>
        <textarea rows="4" bind:value={caText} placeholder="-----BEGIN CERTIFICATE-----"></textarea>
      </details>
    {:else if provider}
      <p class="muted servers">IMAP {cfg.imap_host}:{cfg.imap_port} ({cfg.imap_security.toUpperCase()}) ·
        SMTP {cfg.smtp_host}:{cfg.smtp_port} ({cfg.smtp_security.toUpperCase()})</p>
    {/if}
  {/if}

  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <div class="actions">
    {#if onCancel}<button type="button" class="btn" onclick={onCancel}>Cancel</button>{/if}
    <button class="btn primary" disabled={busy || tile === "" || (oauth && !provider?.oauth_ready)}>
      {busy ? (oauth ? "Waiting for browser…" : "Connecting…") : oauth ? authLabel(cfg.auth ?? "password") : existing ? "Verify and save" : "Add account"}</button>
  </div>
</form>

<style>
  .setup { max-width: 680px; margin: 40px auto; background: var(--panel); border: 1px solid var(--border);
    border-radius: 16px; padding: 28px; display: flex; flex-direction: column; gap: 14px; }
  h2, h3 { margin: 0; }
  .setup.embedded { margin: 0; max-width: none; padding: 20px; }
  p { margin: 0; }
  .tiles { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
  .tile { padding: 12px 10px; border-radius: 10px; border: 1px solid var(--border); background: transparent;
    color: inherit; cursor: pointer; font: inherit; text-align: center; }
  .tile.on { border-color: var(--accent, #3584e4); box-shadow: 0 0 0 1px var(--accent, #3584e4) inset; font-weight: 600; }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  label { display: flex; flex-direction: column; gap: 6px; font-size: 13px; color: var(--muted); }
  label.inline { flex-direction: row; align-items: center; gap: 6px; }
  .auth { display: flex; gap: 18px; }
  .hint { font-size: 13px; line-height: 1.45; }
  .servers { font-family: monospace; font-size: 12px; }
  fieldset { border: 1px solid var(--border); border-radius: 10px; display: flex; gap: 8px; padding: 10px 12px 12px; }
  legend { color: var(--muted); font-size: 13px; padding: 0 6px; }
  .host { flex: 1; } .port { width: 90px; }
  textarea { width: 100%; margin-top: 8px; font-family: monospace; font-size: 12px; }
  summary { cursor: pointer; color: var(--muted); }
  .actions { display: flex; justify-content: flex-end; gap: 8px; }
</style>
