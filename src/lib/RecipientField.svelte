<script lang="ts">
  // Recipient chips with contact autocomplete (GNOME address books via EDS,
  // then addresses from the mail cache). Keyboard: Up/Down to move, Enter or
  // Tab to pick, Escape to close; typing "," ";" or Enter commits raw text.
  import { api, type ContactSuggestion } from "./api";
  import { onDestroy } from "svelte";

  let { label, accountId, values = $bindable([]), autofocus = false }:
    { label: string; accountId: number; values: string[]; autofocus?: boolean } = $props();

  let text = $state("");
  let suggestions = $state<ContactSuggestion[]>([]);
  let active = $state(0);
  let open = $state(false);
  let input: HTMLInputElement | undefined = $state();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let seq = 0;
  const listId = `ac-${Math.random().toString(36).slice(2)}`;

  $effect(() => { if (autofocus) input?.focus(); });
  onDestroy(() => { clearTimeout(timer); seq++; });

  function format(s: ContactSuggestion) {
    return s.name ? `${s.name} <${s.email}>` : s.email;
  }
  function add(v: string) {
    clearTimeout(timer); seq++; // drop a lookup still in flight
    const t = v.trim().replace(/[,;]+$/, "").trim();
    if (t && !values.includes(t)) values = [...values, t];
    text = ""; suggestions = []; open = false;
  }
  function remove(i: number) {
    values = values.filter((_, j) => j !== i);
    input?.focus();
  }
  function onInput() {
    if (/[,;]$/.test(text)) { add(text); return; }
    clearTimeout(timer);
    const mine = ++seq;
    const q = text.trim();
    if (q.length < 2) { suggestions = []; open = false; return; }
    timer = setTimeout(async () => {
      try {
        const r = await api.suggestContacts(accountId, q);
        if (mine !== seq) return; // a newer query is in flight
        const taken = new Set(values.map((v) => v.toLowerCase()));
        suggestions = r.filter((s) => !taken.has(format(s).toLowerCase()));
        active = 0; open = suggestions.length > 0;
      } catch { if (mine === seq) { suggestions = []; open = false; } }
    }, 150);
  }
  function onBlur() {
    // Blur runs before Send's click. Commit now, so a typed recipient is
    // included even when the user never pressed Enter or picked a chip.
    if (text.trim()) add(text);
    else { clearTimeout(timer); seq++; open = false; }
  }
  function onKey(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
      // Commit the typed address before the shortcut bubbles to Composer.
      if (text.trim()) add(text);
      return;
    }
    if (open && e.key === "ArrowDown") { e.preventDefault(); active = (active + 1) % suggestions.length; }
    else if (open && e.key === "ArrowUp") { e.preventDefault(); active = (active - 1 + suggestions.length) % suggestions.length; }
    else if (open && (e.key === "Enter" || e.key === "Tab") && suggestions[active]) { e.preventDefault(); add(format(suggestions[active])); }
    else if (e.key === "Enter" && text.trim()) { e.preventDefault(); add(text); }
    else if (e.key === "Escape" && open) { e.preventDefault(); e.stopPropagation(); open = false; }
    else if (e.key === "Backspace" && !text && values.length) { values = values.slice(0, -1); }
  }
</script>

<div class="field">
  <span class="label">{label}</span>
  <div class="chips">
    {#each values as v, i (v)}
      <span class="chip" title={v}>{v.replace(/\s*<[^>]+>$/, "") || v}
        <button type="button" aria-label={`Remove ${v}`} onclick={() => remove(i)}>×</button></span>
    {/each}
    <div class="wrap">
      <input bind:this={input} bind:value={text} oninput={onInput} onkeydown={onKey}
        onblur={onBlur}
        role="combobox" aria-expanded={open} aria-controls={listId} aria-autocomplete="list"
        aria-label={`${label} recipients`} autocomplete="off" spellcheck="false" />
      {#if open}
        <ul class="dropdown" id={listId} role="listbox">
          {#each suggestions as s, i (s.email)}
            <li role="option" aria-selected={i === active} class:active={i === active}
              onmousedown={(e) => { e.preventDefault(); add(format(s)); }} onmouseenter={() => (active = i)}>
              <span class="avatar">{(s.name || s.email)[0]?.toUpperCase()}</span>
              <span class="who"><span class="n">{s.name || s.email}</span><span class="e">{s.email}</span></span>
              <span class="src" title={s.source === "contacts" ? "GNOME Contacts" : "From your mail"}>{s.source === "contacts" ? "Contacts" : "Recent"}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </div>
</div>

<style>
  .field { display: flex; align-items: flex-start; gap: 10px; border-bottom: 1px solid var(--border); padding: 4px 0; }
  .label { width: 56px; color: var(--muted); padding-top: 8px; flex: none; }
  .chips { flex: 1; display: flex; flex-wrap: wrap; gap: 6px; align-items: center; min-width: 0; }
  .chip { display: inline-flex; align-items: center; gap: 4px; background: var(--accent-soft); color: var(--text);
    border: 1px solid var(--border); border-radius: 999px; padding: 2px 4px 2px 10px; font-size: 13px; max-width: 100%; }
  .chip button { border: 0; background: transparent; color: var(--muted); padding: 0 4px; font-size: 15px; line-height: 1; }
  .chip button:hover { color: var(--text); }
  .wrap { position: relative; flex: 1; min-width: 140px; }
  input { width: 100%; border: 0; background: transparent; padding: 7px 2px; }
  input:focus { outline: none; }
  .dropdown { position: absolute; top: 100%; left: 0; z-index: 30; min-width: 320px; max-width: 460px; margin: 2px 0 0; padding: 4px;
    list-style: none; background: var(--panel); border: 1px solid var(--border); border-radius: 10px; box-shadow: 0 12px 32px #0005; }
  li { display: flex; align-items: center; gap: 10px; padding: 7px 8px; border-radius: 8px; cursor: pointer; }
  li.active { background: var(--selected); }
  .avatar { width: 28px; height: 28px; border-radius: 50%; background: var(--accent); color: #fff; display: grid; place-items: center; font-size: 12px; font-weight: 600; flex: none; }
  .who { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .n { font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .e { color: var(--muted); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .src { font-size: 11px; color: var(--muted); border: 1px solid var(--border); border-radius: 6px; padding: 1px 6px; flex: none; }
</style>
