<script lang="ts">
  // Shared message editor: used inline in the preview pane (Reply / Reply all /
  // Forward / Edit draft) and inside the standalone Compose window.
  import { onDestroy, onMount } from "svelte";
  import { Icon } from "@neutron/ui";
  import RecipientField from "./RecipientField.svelte";
  import RichEditor from "./RichEditor.svelte";
  import { size } from "./format";
  import { undoSendSeconds } from "./prefs";
  import { fromLocalInput, laterChoices, toLocalInput, type LaterChoice } from "./sendLater";
  import { api, errorText, type Account, type ComposeDraft, type ComposerResult, type ForwardedAttachment, type MailAddress, type PickedAttachment, type Template } from "./api";

  let { account, draft, title, inline = false, replaceDraft = null, onDone }:
    { account: Account; draft: ComposeDraft; title: string; inline?: boolean; replaceDraft?: number | null;
      onDone: (r: ComposerResult) => void } = $props();

  // svelte-ignore state_referenced_locally
  const init = draft;
  let to = $state<string[]>([...init.to]);
  let cc = $state<string[]>([...init.cc]);
  let bcc = $state<string[]>([...init.bcc]);
  let showCcBcc = $state(init.cc.length > 0 || init.bcc.length > 0);
  let subject = $state(init.subject);
  let body = $state(init.body);
  // Rich text by default; the plain-text switch drops formatting.
  let rich = $state(true);
  let html = $state(init.html ?? textToHtml(init.body));
  let editorKey = $state(0);
  let carried = $state<ForwardedAttachment[]>([...init.attachments]);
  let picked = $state<PickedAttachment[]>([...(init.picked ?? [])]);
  let busy = $state<"" | "send" | "draft" | "pick">("");
  let error = $state("");
  let notice = $state("");
  let bodyEl: HTMLTextAreaElement | undefined = $state();
  // Sender: the account's address or one of its aliases (Account settings).
  let identities = $state<MailAddress[]>([]);
  // svelte-ignore state_referenced_locally
  let from = $state<string>(init.from ?? account.email);
  // Signatures by sender: switching From swaps the signature in the text.
  let signatures = { base: "", ids: {} as Record<string, string> };
  onMount(() => {
    api.getAccountSettings(account.id).then((s) => {
      identities = s.identities ?? [];
      signatures = { base: s.signature_html ?? "", ids: s.identity_signatures ?? {} };
    }).catch(() => {});
  });
  const sigFor = (addr: string) => {
    const own = signatures.ids[addr.trim().toLowerCase()];
    return own?.trim() ? own : signatures.base;
  };
  /** Replace the "-- " signature block (or add one); null if nothing to do. */
  function swapSignatureHtml(h: string, next: string): string | null {
    const doc = new DOMParser().parseFromString(h, "text/html"); // inert
    const kids = [...doc.body.children];
    const dash = kids.find((k) => k.textContent?.trim() === "--" && k.nextElementSibling);
    if (dash) {
      const sig = dash.nextElementSibling!;
      if (next.trim()) sig.innerHTML = next;
      else { sig.remove(); dash.remove(); }
      return doc.body.innerHTML;
    }
    if (!next.trim()) return null;
    const first = kids[0];
    const block = `<div>-- </div><div>${next}</div>`;
    if (first && first.tagName === "P" && !first.textContent?.trim()) {
      first.insertAdjacentHTML("afterend", block + (kids.length > 1 ? "<p><br></p>" : ""));
      return doc.body.innerHTML;
    }
    return `<p><br></p>${block}${h.trim() ? `<p><br></p>${h}` : ""}`;
  }
  function swapSignatureText(t: string, prev: string, next: string): string | null {
    const prevText = prev.trim() ? htmlToText(prev).trim() : "";
    const nextText = next.trim() ? htmlToText(next).trim() : "";
    if (prevText) {
      const at = t.indexOf(`-- \n${prevText}`);
      if (at < 0) return null;
      const end = at + 4 + prevText.length;
      return t.slice(0, at) + (nextText ? `-- \n${nextText}` : "") + t.slice(end + (nextText ? 0 : 1));
    }
    return nextText ? `\n\n-- \n${nextText}\n${t.replace(/^\n+/, "")}` : null;
  }
  // ---- templates ----
  let editor: RichEditor | undefined = $state();
  let templates = $state<Template[] | null>(null);
  let templatesOpen = $state(false);
  let templateName = $state<string | null>(null);
  let templateError = $state("");
  async function openTemplates() {
    templatesOpen = !templatesOpen; templateName = null; templateError = "";
    if (templatesOpen) {
      try { templates = await api.listTemplates(); } catch (e) { templateError = errorText(e); }
    }
  }
  function useTemplate(t: Template) {
    templatesOpen = false;
    if (rich && editor) { editor.insertHtml(t.html); return; }
    const text = htmlToText(t.html).trim();
    const at = bodyEl?.selectionStart ?? 0;
    body = body.slice(0, at) + text + body.slice(at);
  }
  async function saveTemplate() {
    const name = templateName?.trim() ?? "";
    if (!name) { templateError = "Give the template a name."; return; }
    try {
      const t = await api.saveTemplate(null, name, rich ? html : textToHtml(body));
      templates = [...(templates ?? []), t].sort((a, b) => a.name.localeCompare(b.name));
      templateName = null; templateError = "";
      notice = `Saved template “${t.name}”.`;
    } catch (e) { templateError = errorText(e); }
  }
  async function deleteTemplate(t: Template) {
    try { await api.deleteTemplate(t.id); templates = (templates ?? []).filter((x) => x.id !== t.id); }
    catch (e) { templateError = errorText(e); }
  }

  function changeFrom(next: string) {
    const prev = sigFor(from);
    const sig = sigFor(next);
    from = next;
    if (prev === sig) return;
    if (rich) {
      const h = swapSignatureHtml(html, sig);
      if (h !== null) { html = h; editorKey++; }
    } else {
      const t = swapSignatureText(body, prev, sig);
      if (t !== null) body = t;
    }
  }

  // ---- automatic draft saving ----
  // Every 30 s, if anything changed, the draft is stored in Drafts; each save
  // replaces the previous one (by Message-ID), and the resumed draft this
  // composer was opened from is replaced by the first save.
  const AUTOSAVE_MS = 30_000;
  // svelte-ignore state_referenced_locally
  let resumedId = $state<number | null>(replaceDraft);
  let draftMid = $state<string | null>(init.draft_message_id ?? null);
  let autosave = $state<"" | "saving" | "off">("");
  let savedAt = $state<Date | null>(null);
  let saveNote = $state("");
  let saving: Promise<void> | null = null;
  let closed = false;
  const content = () => JSON.stringify(message());
  // svelte-ignore state_referenced_locally
  let lastSaved = content();
  function hasContent() {
    return to.length + cc.length + bcc.length > 0 || subject.trim() !== "" || picked.length > 0
      || (rich ? html.replace(/<[^>]*>|&nbsp;/g, "").trim() !== "" : body.trim() !== "");
  }
  async function storeDraft(): Promise<void> {
    const snapshot = content();
    const r = await api.saveDraft(account.id, message(), resumedId, draftMid);
    draftMid = r.message_id; resumedId = null; lastSaved = snapshot; savedAt = new Date(); saveNote = "";
    if (r.replace_error) {
      autosave = "off";
      saveNote = `Autosave paused: the earlier copy couldn't be replaced (${r.replace_error})`;
    }
  }
  function autoSave(): Promise<void> {
    if (closed || busy || saving || autosave === "off" || content() === lastSaved || !hasContent()) return saving ?? Promise.resolve();
    autosave = "saving";
    saving = storeDraft()
      .catch((e) => { saveNote = `Not saved: ${errorText(e)}`; })
      .finally(() => { saving = null; if (autosave === "saving") autosave = ""; });
    return saving;
  }
  $effect(() => {
    const t = setInterval(() => void autoSave(), AUTOSAVE_MS);
    return () => clearInterval(t);
  });
  // Closed without Send / Save / Discard (e.g. another message was opened):
  // keep what was typed.
  onDestroy(() => { void autoSave(); closed = true; });

  $effect(() => {
    // Replies: cursor at the top, above the quote.
    if (!rich && inline && bodyEl && init.to.length) { bodyEl.focus(); bodyEl.setSelectionRange(0, 0); bodyEl.scrollTop = 0; }
  });

  function textToHtml(t: string) {
    const e = t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    return e.trim() ? e.replace(/\r?\n/g, "<br>") : "<p><br></p>";
  }
  // Rough plain text of the editor HTML (for switching to plain text; the
  // text/plain part of rich mail is generated by the backend instead).
  function htmlToText(h: string) {
    const doc = new DOMParser().parseFromString(h, "text/html"); // inert: no scripts, no loads
    doc.querySelectorAll("br").forEach((b) => b.replaceWith("\n"));
    doc.querySelectorAll("p,div,li,h1,h2,h3,h4,h5,h6,tr,blockquote").forEach((b) => b.append("\n"));
    doc.querySelectorAll("blockquote").forEach((q) => {
      q.textContent = (q.textContent ?? "").trimEnd().split("\n").map((l) => `> ${l}`).join("\n") + "\n";
    });
    return (doc.body.textContent ?? "").replace(/\n{3,}/g, "\n\n");
  }
  function toggleRich() {
    if (rich) {
      if (!confirm("Switch to plain text? Formatting and inline images will be removed.")) return;
      body = htmlToText(html); rich = false;
    } else {
      html = textToHtml(body); editorKey++; rich = true;
    }
  }

  function message() {
    return {
      from: from.toLowerCase() === account.email.toLowerCase() ? null : from,
      to, cc, bcc, subject,
      body: rich ? "" : body,
      html_body: rich ? html : null,
      in_reply_to: init.in_reply_to, references: init.references,
      attachments: [
        ...carried.map((a) => ({ kind: "message" as const, message_id: a.message_id, index: a.index })),
        ...picked.map((p) => ({ kind: "file" as const, token: p.token })),
      ],
    };
  }
  const total = $derived(carried.reduce((n, a) => n + a.size, 0) + picked.reduce((n, a) => n + a.size, 0));

  async function pick() {
    if (busy) return;
    busy = "pick"; error = ""; notice = "";
    try {
      const r = await api.pickAttachments();
      picked = [...picked, ...r.files];
      if (r.rejected.length) notice = `Not attached: ${r.rejected.join("; ")}`;
    } catch (e) { error = errorText(e); } finally { busy = ""; }
  }
  /** Everything needed to reopen this message exactly (undo send, failed send). */
  function snapshot(): ComposeDraft {
    return {
      to: [...to], cc: [...cc], bcc: [...bcc], subject,
      body: rich ? "" : body, html: rich ? html : null,
      in_reply_to: init.in_reply_to, references: init.references,
      attachments: [...carried], from, picked: [...picked], draft_message_id: draftMid,
    };
  }
  async function send() {
    if (busy) return;
    if (!to.length && !cc.length && !bcc.length) { error = "Add at least one recipient."; return; }
    busy = "send"; error = "";
    try {
      if (saving) await saving;
      const delay = undoSendSeconds();
      const id = await api.queueSend(account.id, message(), resumedId, draftMid, delay);
      closed = true;
      onDone({ kind: "queued", id, account, draft: snapshot(), replaceDraft: resumedId, delay });
    } catch (e) { error = errorText(e); } finally { busy = ""; }
  }
  // ---- send later ----
  let laterOpen = $state(false);
  let laterList = $state<LaterChoice[]>([]);
  let laterCustom = $state("");
  function openLater() {
    laterOpen = !laterOpen;
    if (laterOpen) {
      laterList = laterChoices();
      laterCustom = toLocalInput(laterList.find((c) => c.label.startsWith("Tomorrow morning"))?.at ?? new Date(Date.now() + 3_600_000));
    }
  }
  async function sendLater(when: Date | null) {
    if (busy) return;
    if (!when || when.getTime() < Date.now() + 30_000) { error = "Pick a time in the future."; return; }
    if (!to.length && !cc.length && !bcc.length) { error = "Add at least one recipient."; return; }
    laterOpen = false; busy = "send"; error = "";
    try {
      if (saving) await saving;
      const item = await api.scheduleSend(account.id, message(), resumedId, draftMid, Math.floor(when.getTime() / 1000));
      closed = true;
      onDone({ kind: "scheduled", item });
    } catch (e) { error = errorText(e); } finally { busy = ""; }
  }
  async function saveDraft() {
    if (busy) return;
    busy = "draft"; error = "";
    try {
      if (saving) await saving;
      await storeDraft();
      closed = true;
      onDone({ kind: "draft" });
    } catch (e) { error = errorText(e); } finally { busy = ""; }
  }
  async function discard() {
    if (busy) return;
    closed = true;
    if (saving) await saving.catch(() => {});
    // Remove the autosaved copy too; a failure is reported but doesn't block.
    if (draftMid) api.discardDraft(account.id, draftMid).catch(() => {});
    onDone({ kind: "discarded" });
  }
  function onKey(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") { e.preventDefault(); void send(); }
  }
</script>

<svelte:window onclick={(e) => {
  const t = e.target as Element;
  if (templatesOpen && !t.closest?.(".tpl-wrap")) templatesOpen = false;
  if (laterOpen && !t.closest?.(".send-split")) laterOpen = false;
}} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section class="composer" class:inline aria-label={title} onkeydown={onKey} role="form">
  <header>
    <strong>{title}</strong>
    {#if identities.length}
      <label class="from muted">from
        <select value={from} onchange={(e) => changeFrom((e.currentTarget as HTMLSelectElement).value)} aria-label="From">
          <option value={account.email}>{account.display_name || account.email} &lt;{account.email}&gt;</option>
          {#each identities as i (i.email)}<option value={i.email}>{i.name || account.display_name || i.email} &lt;{i.email}&gt;</option>{/each}
          {#if from.toLowerCase() !== account.email.toLowerCase() && !identities.some((i) => i.email.toLowerCase() === from.toLowerCase())}
            <option value={from}>{from} (not configured)</option>
          {/if}
        </select></label>
    {:else}
      <span class="from muted">from {account.display_name || account.email} &lt;{account.email}&gt;</span>
    {/if}
    {#if !showCcBcc}<button type="button" class="link" onclick={() => (showCcBcc = true)}>Cc / Bcc</button>{/if}
  </header>
  <RecipientField label="To" accountId={account.id} bind:values={to} autofocus={!init.to.length} />
  {#if showCcBcc}
    <RecipientField label="Cc" accountId={account.id} bind:values={cc} />
    <RecipientField label="Bcc" accountId={account.id} bind:values={bcc} />
  {/if}
  <label class="subject"><span>Subject</span><input bind:value={subject} aria-label="Subject" /></label>
  {#if rich}
    {#key editorKey}
      <RichEditor bind:this={editor} {html} focusStart={inline && init.to.length > 0} onInput={(h) => (html = h)} onError={(m) => (notice = m)} />
    {/key}
  {:else}
    <textarea bind:this={bodyEl} bind:value={body} placeholder="Write your message…" aria-label="Message body"></textarea>
  {/if}
  {#if carried.length || picked.length}
    <div class="atts" aria-label="Attachments">
      {#each carried as a, i (`m${a.message_id}-${a.index}`)}
        <span class="att" title="Forwarded from the original message"><Icon name="clip" size={14} />{a.name}<em>{size(a.size)}</em>
          <button type="button" aria-label={`Remove ${a.name}`} onclick={() => (carried = carried.filter((_, j) => j !== i))}>×</button></span>
      {/each}
      {#each picked as p, i (p.token)}
        <span class="att"><Icon name="clip" size={14} />{p.name}<em>{size(p.size)}</em>
          <button type="button" aria-label={`Remove ${p.name}`} onclick={() => (picked = picked.filter((_, j) => j !== i))}>×</button></span>
      {/each}
      <span class="muted total">{size(total)} total · max 25 MB</span>
    </div>
  {/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <footer>
    <span class="send-split">
      <button type="button" class="btn primary send-main" onclick={send} disabled={busy !== ""}>{busy === "send" ? "Sending…" : "Send"}</button>
      <button type="button" class="btn primary send-more" onclick={openLater} disabled={busy !== ""} aria-expanded={laterOpen}
        aria-label="Send later" title="Send later"><Icon name="chevron" size={12} /></button>
      {#if laterOpen}
        <div class="later-pop" role="menu" tabindex="-1" onkeydown={(e) => { if (e.key === "Escape") { e.stopPropagation(); laterOpen = false; } }}>
          <p class="muted">Send later</p>
          {#each laterList as c (c.label)}
            <button type="button" role="menuitem" onclick={() => sendLater(c.at)}>{c.label}</button>
          {/each}
          <div class="later-custom">
            <input type="datetime-local" bind:value={laterCustom} aria-label="Send at" />
            <button type="button" class="btn small primary" onclick={() => sendLater(fromLocalInput(laterCustom))}>Schedule</button>
          </div>
          <p class="muted hint">Sent by Neutron Mail: if it's closed at that time, the message goes out when you next open it.</p>
        </div>
      {/if}
    </span>
    <button type="button" class="btn" onclick={pick} disabled={busy !== ""} title="Attach files"><Icon name="clip" size={15} /> Attach</button>
    <button type="button" class="btn ghost small" onclick={toggleRich} disabled={busy !== ""}
      title={rich ? "Send as plain text only" : "Use rich text (HTML)"}>{rich ? "Plain text" : "Rich text"}</button>
    <button type="button" class="btn" onclick={saveDraft} disabled={busy !== ""}>{busy === "draft" ? "Saving…" : "Save draft"}</button>
    <span class="tpl-wrap">
      <button type="button" class="btn ghost small" onclick={openTemplates} aria-expanded={templatesOpen} disabled={busy !== ""}
        title="Insert a saved text, or save this one">Templates</button>
      {#if templatesOpen}
        <div class="tpl-pop" role="dialog" aria-label="Templates" tabindex="-1" onkeydown={(e) => { if (e.key === "Escape") { e.stopPropagation(); templatesOpen = false; } }}>
          {#if templates === null}<p class="muted">Loading…</p>
          {:else if !templates.length}<p class="muted">No templates yet. Write a text you use often, then save it here.</p>
          {:else}
            <ul>
              {#each templates as t (t.id)}
                <li><button type="button" class="tpl-use" onclick={() => useTemplate(t)} title={htmlToText(t.html).slice(0, 200)}>{t.name}</button>
                  <button type="button" class="tpl-del" onclick={() => deleteTemplate(t)} aria-label={`Delete template ${t.name}`}>×</button></li>
              {/each}
            </ul>
          {/if}
          {#if templateName === null}
            <button type="button" class="link" onclick={() => (templateName = subject.trim().slice(0, 100))}>Save this message as a template…</button>
          {:else}
            <div class="tpl-save">
              <input bind:value={templateName} maxlength="100" placeholder="Template name" aria-label="Template name"
                onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); void saveTemplate(); } }} />
              <button type="button" class="btn small primary" onclick={saveTemplate}>Save</button>
            </div>
          {/if}
          {#if templateError}<p class="error" role="alert">{templateError}</p>{/if}
        </div>
      {/if}
    </span>
    <span class="spacer"></span>
    <span class="saved muted" role="status" aria-live="polite" title={saveNote}>
      {#if autosave === "saving"}Saving…{:else if saveNote}{saveNote.startsWith("Autosave paused") ? "Autosave paused" : "Not saved"}{:else if savedAt}Saved {savedAt.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}{/if}
    </span>
    <button type="button" class="btn ghost" onclick={discard} disabled={busy !== ""}>Discard</button>
  </footer>
</section>

<style>
  .send-split { position: relative; display: inline-flex; }
  .send-main { border-top-right-radius: 0; border-bottom-right-radius: 0; }
  .send-more { border-top-left-radius: 0; border-bottom-left-radius: 0; padding-left: 6px; padding-right: 6px;
    border-left: 1px solid color-mix(in srgb, #fff 35%, transparent); display: flex; align-items: center; }
  .send-more :global(svg) { transform: rotate(90deg); }
  .later-pop { position: absolute; bottom: calc(100% + 6px); left: 0; z-index: 20; width: 290px; background: var(--panel);
    border: 1px solid var(--border); border-radius: 10px; padding: 8px; box-shadow: 0 8px 24px rgb(0 0 0 / 0.2);
    display: flex; flex-direction: column; gap: 2px; }
  .later-pop p { margin: 2px 6px 4px; font-size: 12px; }
  .later-pop .hint { margin-top: 6px; font-size: 11px; }
  .later-pop > button { border: 0; background: transparent; text-align: left; padding: 7px 8px; border-radius: 6px; cursor: pointer; color: inherit; }
  .later-pop > button:hover { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .later-custom { display: flex; gap: 6px; padding: 6px 4px 0; border-top: 1px solid var(--border); margin-top: 4px; }
  .later-custom input { flex: 1; min-width: 0; }
  /* Anchored to the footer, so it stays inside a narrow inline reply. */
  footer { position: relative; }
  .tpl-wrap { position: static; }
  .tpl-pop { position: absolute; bottom: calc(100% + 6px); left: 0; z-index: 20; width: min(300px, 100%); max-height: 320px; overflow: auto;
    background: var(--panel); border: 1px solid var(--border); border-radius: 10px; padding: 8px; box-shadow: 0 8px 24px rgb(0 0 0 / 0.2);
    display: flex; flex-direction: column; gap: 6px; }
  .tpl-pop p { margin: 2px 4px; font-size: 12px; }
  .tpl-pop ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
  .tpl-pop li { display: flex; align-items: center; }
  .tpl-use { flex: 1; text-align: left; border: 0; background: transparent; padding: 6px 8px; border-radius: 6px; cursor: pointer; color: inherit;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tpl-use:hover { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .tpl-del { border: 0; background: transparent; color: var(--muted); cursor: pointer; padding: 2px 8px; font-size: 16px; }
  .tpl-save { display: flex; gap: 6px; }
  .tpl-save input { flex: 1; min-width: 0; }
  .tpl-pop .link { align-self: flex-start; }
  .composer { display: flex; flex-direction: column; gap: 4px; background: var(--panel); min-height: 0; height: 100%; }
  .composer.inline { border: 1px solid var(--border); border-radius: 14px; padding: 12px 16px; box-shadow: 0 -6px 24px #0003; }
  header { display: flex; align-items: baseline; gap: 10px; padding-bottom: 4px; }
  .from { font-size: 12px; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  label.from { display: flex; align-items: center; gap: 4px; min-width: 0; overflow: visible; }
  label.from select { font-size: 12px; flex: 1 1 auto; min-width: 0; max-width: 100%; padding: 2px 4px; color: var(--text); text-overflow: ellipsis; }
  .link { border: 0; background: transparent; color: var(--unread); padding: 0; font-size: 13px; }
  .subject { display: flex; align-items: center; gap: 10px; border-bottom: 1px solid var(--border); padding: 2px 0; }
  .subject span { width: 56px; color: var(--muted); flex: none; }
  .subject input { flex: 1; border: 0; background: transparent; }
  .subject input:focus { outline: none; }
  textarea { flex: 1; min-height: 120px; resize: none; border: 0; background: transparent; padding: 10px 2px; line-height: 1.5; font-family: inherit; }
  textarea:focus { outline: none; }
  .atts { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; padding: 4px 0; }
  .att { display: inline-flex; align-items: center; gap: 6px; border: 1px solid var(--border); border-radius: 8px; padding: 4px 6px 4px 8px; font-size: 13px; }
  .att em { color: var(--muted); font-style: normal; font-size: 12px; }
  .att button { border: 0; background: transparent; color: var(--muted); padding: 0 4px; }
  .total { font-size: 12px; }
  .notice { margin: 0; color: #f5b14c; font-size: 13px; }
  .error { margin: 0; font-size: 13px; }
  footer { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; padding-top: 6px; }
  .saved { font-size: 12px; white-space: nowrap; }
  footer .btn { display: inline-flex; gap: 6px; align-items: center; }
  .spacer { flex: 1; }
  .ghost { background: transparent; }
  .small { font-size: 12px; }
</style>
