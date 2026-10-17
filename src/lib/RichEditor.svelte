<script lang="ts">
  // Rich-text editor for the composer. Content is a contenteditable <div>
  // in the app page, so everything that enters it is sanitized by the Rust
  // compose sanitizer first: the initial HTML (quoted original, resumed
  // draft) comes sanitized from the backend, and paste/drop go through
  // `sanitize_compose_html`. The app CSP (no inline script, img-src 'self'
  // data:) is the backstop. The backend sanitizes once more before sending.
  import { untrack } from "svelte";
  import { api, errorText } from "./api";

  let { html, onInput, onError, focusStart = false }:
    { html: string; onInput: (html: string) => void; onError: (msg: string) => void; focusStart?: boolean } = $props();

  let el: HTMLDivElement | undefined = $state();
  let active = $state<Record<string, boolean>>({});

  // Set the initial content once. `html` is untracked: the parent updates it
  // on every keystroke, and re-assigning innerHTML would reset the caret.
  $effect(() => {
    if (!el) return;
    const [initial, top] = untrack(() => [html, focusStart] as const);
    el.innerHTML = initial;
    if (top) {
      el.focus();
      const r = document.createRange();
      r.setStart(el, 0); r.collapse(true);
      const sel = window.getSelection(); sel?.removeAllRanges(); sel?.addRange(r);
      el.scrollTop = 0;
    }
  });

  function changed() { if (el) onInput(el.innerHTML); refresh(); }
  // Where the caret last was, so text inserted from outside (templates)
  // lands there even after a button took the focus.
  let caret: Range | null = null;
  function remember() {
    const sel = window.getSelection();
    if (el && sel?.rangeCount && el.contains(sel.getRangeAt(0).startContainer)) caret = sel.getRangeAt(0).cloneRange();
  }
  /** Insert sanitized HTML at the caret (or at the start). */
  export function insertHtml(h: string) {
    if (!el) return;
    el.focus();
    const sel = window.getSelection();
    const r = caret ?? (() => { const x = document.createRange(); x.setStart(el!, 0); x.collapse(true); return x; })();
    sel?.removeAllRanges(); sel?.addRange(r);
    document.execCommand("insertHTML", false, h);
    changed(); remember();
  }
  function refresh() {
    remember();
    const q = (c: string) => { try { return document.queryCommandState(c); } catch { return false; } };
    active = { bold: q("bold"), italic: q("italic"), underline: q("underline"),
      insertUnorderedList: q("insertUnorderedList"), insertOrderedList: q("insertOrderedList") };
  }
  function exec(cmd: string, value?: string) {
    el?.focus();
    document.execCommand(cmd, false, value);
    changed();
  }
  function quote() {
    // formatBlock blockquote, then mark it as a citation like mail clients do.
    exec("formatBlock", "blockquote");
  }
  // Link bar (in-app; window.prompt shows a WebKit "JavaScript" dialog).
  let linkOpen = $state(false);
  let linkUrl = $state("https://");
  let linkInput: HTMLInputElement | undefined = $state();
  let savedRange: Range | null = null;
  function link() {
    const sel = window.getSelection();
    savedRange = sel && sel.rangeCount && el?.contains(sel.anchorNode) ? sel.getRangeAt(0).cloneRange() : null;
    const a = sel?.anchorNode?.parentElement?.closest("a");
    linkUrl = a?.getAttribute("href") ?? "https://";
    linkOpen = true;
    queueMicrotask(() => { linkInput?.focus(); linkInput?.select(); });
  }
  function applyLink() {
    const url = linkUrl.trim();
    let ok = false;
    try { ok = ["https:", "http:", "mailto:"].includes(new URL(url).protocol); } catch { ok = false; }
    if (!ok) { onError("Only https://, http:// and mailto: links can be added."); return; }
    linkOpen = false;
    onError("");
    el?.focus();
    const sel = window.getSelection();
    if (savedRange && sel) { sel.removeAllRanges(); sel.addRange(savedRange); }
    if (!savedRange || savedRange.collapsed) exec("insertHTML", `<a href="${escapeAttr(url)}">${escapeText(url.replace(/^mailto:/, ""))}</a>&nbsp;`);
    else exec("createLink", url);
  }
  function linkKey(e: KeyboardEvent) {
    if (e.key === "Enter") { e.preventDefault(); applyLink(); }
    else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); linkOpen = false; el?.focus(); }
  }
  const escapeText = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const escapeAttr = (s: string) => escapeText(s).replace(/"/g, "&quot;");

  async function insertTransfer(dt: DataTransfer | null) {
    if (!dt) return;
    const rich = dt.getData("text/html");
    const plain = dt.getData("text/plain");
    try {
      if (rich) {
        const clean = await api.sanitizeComposeHtml(rich);
        exec("insertHTML", clean);
      } else if (plain) {
        exec("insertText", plain);
      } else if (dt.files.length) {
        onError("Use Attach to add files.");
      }
    } catch (e) { onError(errorText(e)); }
  }
  function onPaste(e: ClipboardEvent) { e.preventDefault(); void insertTransfer(e.clipboardData); }
  function onDrop(e: DragEvent) { e.preventDefault(); void insertTransfer(e.dataTransfer); }
  function onKey(e: KeyboardEvent) {
    if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey) return;
    const k = e.key.toLowerCase();
    if (k === "b") { e.preventDefault(); exec("bold"); }
    else if (k === "i") { e.preventDefault(); exec("italic"); }
    else if (k === "u") { e.preventDefault(); exec("underline"); }
    else if (k === "k") { e.preventDefault(); link(); }
  }

  const tools: { cmd: string; label: string; title: string; run: () => void }[] = [
    { cmd: "bold", label: "B", title: "Bold (Ctrl+B)", run: () => exec("bold") },
    { cmd: "italic", label: "I", title: "Italic (Ctrl+I)", run: () => exec("italic") },
    { cmd: "underline", label: "U", title: "Underline (Ctrl+U)", run: () => exec("underline") },
    { cmd: "strikeThrough", label: "S", title: "Strikethrough", run: () => exec("strikeThrough") },
    { cmd: "insertUnorderedList", label: "•", title: "Bulleted list", run: () => exec("insertUnorderedList") },
    { cmd: "insertOrderedList", label: "1.", title: "Numbered list", run: () => exec("insertOrderedList") },
    { cmd: "quote", label: "❝", title: "Quote", run: quote },
    { cmd: "link", label: "🔗", title: "Link (Ctrl+K)", run: link },
    { cmd: "removeFormat", label: "T̸", title: "Clear formatting", run: () => exec("removeFormat") },
  ];
</script>

<div class="rte">
  <div class="toolbar" role="toolbar" aria-label="Formatting">
    {#each tools as t (t.cmd)}
      <button type="button" class="tool tool-{t.cmd}" class:on={active[t.cmd]} title={t.title} aria-label={t.title}
        aria-pressed={active[t.cmd] ?? false} onmousedown={(e) => e.preventDefault()} onclick={t.run}>{t.label}</button>
    {/each}
  </div>
  {#if linkOpen}
    <div class="linkbar" role="group" aria-label="Insert link">
      <input bind:this={linkInput} bind:value={linkUrl} onkeydown={linkKey} aria-label="Link address" spellcheck="false" placeholder="https://…" />
      <button type="button" class="btn small primary" onclick={applyLink}>Add link</button>
      <button type="button" class="btn small" onclick={() => { linkOpen = false; el?.focus(); }}>Cancel</button>
    </div>
  {/if}
  <div bind:this={el} class="editor" contenteditable="true" role="textbox" aria-multiline="true" tabindex="0"
    aria-label="Message body" spellcheck="true" oninput={changed} onpaste={onPaste} ondrop={onDrop}
    onkeydown={onKey} onkeyup={refresh} onmouseup={refresh}></div>
</div>

<style>
  .rte { flex: 1; display: flex; flex-direction: column; min-height: 140px; }
  .toolbar { display: flex; gap: 2px; padding: 4px 0; border-bottom: 1px solid var(--border); flex-wrap: wrap; }
  .tool { min-width: 28px; height: 26px; border: 0; border-radius: 6px; background: transparent; color: var(--muted); font-size: 13px; padding: 0 6px; }
  .tool:hover { background: var(--panel-2); color: var(--fg); }
  .tool.on { background: var(--panel-2); color: var(--accent); }
  .tool-bold { font-weight: 800; } .tool-italic { font-style: italic; font-family: Georgia, serif; }
  .tool-underline { text-decoration: underline; } .tool-strikeThrough { text-decoration: line-through; }
  /* contain: paint makes the editor the containing block of any fixed-position
     descendant, so content can never draw outside it (backstop to the CSS allow-list). */
  .linkbar { display: flex; gap: 6px; align-items: center; padding: 6px 0; border-bottom: 1px solid var(--border); }
  .linkbar input { flex: 1; border: 1px solid var(--border); border-radius: 8px; padding: 4px 8px; background: var(--panel-2); }
  .small { font-size: 12px; padding: 3px 10px; }
  .editor { flex: 1; min-height: 100px; overflow: auto; padding: 10px 2px; line-height: 1.5; outline: none;
    contain: paint; overflow-wrap: anywhere; }
  .editor :global(blockquote) { margin: 0 0 0 0.8ex; border-left: 2px solid #c9c4d8; padding-left: 1ex; color: var(--muted); }
  .editor :global(img) { max-width: 100%; height: auto; }
  .editor :global(a) { color: var(--accent); }
  .editor :global(p) { margin: 0 0 0.6em; }
</style>
