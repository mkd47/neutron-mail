<script lang="ts">
  // Notes: folders, tags, search, a sanitized rich-text editor with
  // checklists, and links to emails. Stored locally (encrypted cache).
  import { onMount, tick } from "svelte";
  import { untrack } from "svelte";
  import Icon from "./Icon.svelte";
  import { api, errorText, pimApi, type Note, type NoteFolder, type NoteSummary, type Tag } from "./api";

  let { openId = null, onOpenEmail, onStatus }:
    { openId?: number | null; onOpenEmail: (link: Note["links"][number]) => void; onStatus: (t: string, warn?: boolean) => void } = $props();

  let folders = $state<NoteFolder[]>([]);
  let tags = $state<Tag[]>([]);
  let notes = $state<NoteSummary[]>([]);
  let filter = $state<{ folder: number | null; tag: string | null }>({ folder: null, tag: null });
  let query = $state("");
  let note = $state<Note | null>(null);
  let title = $state("");
  let editor = $state<HTMLDivElement | null>(null);
  let dirty = false;
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  let saving = $state<"" | "saving" | "saved">("");
  let tagInput = $state("");
  let menu = $state(false);
  let newFolder = $state<string | null>(null);
  let confirmDelete = $state(false);
  let block = $state("p");

  async function loadSide() {
    [folders, tags] = await Promise.all([pimApi.noteFolders(), pimApi.tags()]);
  }
  async function loadList() {
    notes = await pimApi.notes({ folder_id: filter.folder, tag: filter.tag, query });
  }
  onMount(async () => {
    try {
      await loadSide(); await loadList();
      if (openId !== null) await open(openId);
      else if (notes[0]) await open(notes[0].id);
    } catch (e) { onStatus(errorText(e), true); }
  });
  $effect(() => { const id = openId; if (id !== null) untrack(() => { if (note?.id !== id) void open(id); }); });

  async function open(id: number) {
    await flush();
    try {
      note = await pimApi.note(id); title = note.title; menu = false; confirmDelete = false;
      await tick();
      if (editor) editor.innerHTML = note.html;
    } catch (e) { onStatus(errorText(e), true); }
  }
  function changed() {
    dirty = true; saving = "saving";
    clearTimeout(saveTimer); saveTimer = setTimeout(() => void flush(), 600);
  }
  async function flush() {
    clearTimeout(saveTimer);
    if (!dirty || !note) return;
    dirty = false;
    const id = note.id;
    try {
      const saved = await pimApi.saveNote({ id, folder_id: note.folder_id, title, html: editor?.innerHTML ?? note.html, tags: note.tags, pinned: note.pinned });
      if (note?.id === id) { note = { ...saved }; saving = "saved"; }
      await loadList(); await loadSide();
    } catch (e) { onStatus(errorText(e), true); saving = ""; }
  }
  async function create() {
    await flush();
    try {
      const n = await pimApi.saveNote({ id: null, folder_id: filter.folder, title: "Untitled note", html: "<p></p>", tags: filter.tag ? [filter.tag] : [], pinned: false });
      await loadList(); await loadSide(); await open(n.id);
      document.querySelector<HTMLInputElement>(".ntitle")?.select();
    } catch (e) { onStatus(errorText(e), true); }
  }
  async function remove() {
    if (!note) return;
    try {
      await pimApi.deleteNote(note.id); dirty = false; note = null; confirmDelete = false;
      await loadList(); await loadSide();
      if (notes[0]) await open(notes[0].id);
    } catch (e) { onStatus(errorText(e), true); }
  }
  async function setFilter(f: { folder: number | null; tag: string | null }) {
    await flush(); filter = f; await loadList();
  }
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  function onSearch() { clearTimeout(searchTimer); searchTimer = setTimeout(() => void loadList(), 150); }
  async function addFolder() {
    if (!newFolder?.trim()) { newFolder = null; return; }
    try { await pimApi.createNoteFolder(newFolder); newFolder = null; await loadSide(); } catch (e) { onStatus(errorText(e), true); }
  }
  function addTag() {
    const t = tagInput.trim().replace(/^#/, "");
    if (!note || !t || note.tags.includes(t)) { tagInput = ""; return; }
    note.tags = [...note.tags, t]; tagInput = ""; changed();
  }
  function removeTag(t: string) { if (!note) return; note.tags = note.tags.filter((x) => x !== t); changed(); }
  async function moveTo(folder: number | null) { if (!note) return; note.folder_id = folder; menu = false; changed(); await flush(); }
  async function unlink(mid: string) {
    if (!note) return;
    try { note = await pimApi.unlinkNoteEmail(note.id, mid); await loadList(); } catch (e) { onStatus(errorText(e), true); }
  }

  // ---- editor ----
  function exec(cmd: string, value?: string) { editor?.focus(); document.execCommand(cmd, false, value); changed(); }
  function setBlock(tag: string) { exec("formatBlock", `<${tag}>`); block = tag; }
  function link() {
    const url = prompt("Link address (https://…)");
    if (url && /^(https?:|mailto:)/i.test(url.trim())) exec("createLink", url.trim());
    else if (url) onStatus("Only http, https and mailto links are allowed", true);
  }
  function checklist() {
    editor?.focus();
    const sel = window.getSelection();
    const li = sel?.anchorNode && (sel.anchorNode.nodeType === 1 ? sel.anchorNode as Element : sel.anchorNode.parentElement)?.closest("li");
    const ul = li?.parentElement;
    if (ul && ul.tagName === "UL") {
      // Toggle an existing list between bullets and a checklist.
      if (ul.hasAttribute("data-checklist")) { ul.removeAttribute("data-checklist"); ul.querySelectorAll("li").forEach((x) => x.removeAttribute("data-checked")); }
      else { ul.setAttribute("data-checklist", ""); ul.querySelectorAll(":scope > li").forEach((x) => x.setAttribute("data-checked", "false")); }
      changed(); return;
    }
    document.execCommand("insertHTML", false, '<ul data-checklist=""><li data-checked="false">&#8203;</li></ul>');
    changed();
  }
  function onEditorClick(e: MouseEvent) {
    const li = (e.target as HTMLElement).closest("ul[data-checklist] > li") as HTMLElement | null;
    if (!li) return;
    const r = li.getBoundingClientRect();
    if (e.clientX - r.left < 26) {
      e.preventDefault();
      li.setAttribute("data-checked", li.getAttribute("data-checked") === "true" ? "false" : "true");
      changed();
    }
  }
  function onKeydown(e: KeyboardEvent) {
    // New checklist items start unchecked.
    if (e.key === "Enter") setTimeout(() => {
      const sel = window.getSelection();
      const li = sel?.anchorNode && (sel.anchorNode.nodeType === 1 ? sel.anchorNode as Element : sel.anchorNode.parentElement)?.closest("ul[data-checklist] > li");
      if (li && li.getAttribute("data-checked") === "true" && !li.textContent?.trim()) li.setAttribute("data-checked", "false");
    });
    if ((e.ctrlKey || e.metaKey) && ["b", "i", "u"].includes(e.key.toLowerCase())) { e.preventDefault(); exec({ b: "bold", i: "italic", u: "underline" }[e.key.toLowerCase()]!); }
  }
  async function onPaste(e: ClipboardEvent) {
    e.preventDefault();
    const html = e.clipboardData?.getData("text/html"), text = e.clipboardData?.getData("text/plain") ?? "";
    if (html) {
      try { document.execCommand("insertHTML", false, (await api.sanitizeComposeHtml(html)).replace(/<img[^>]*>/gi, "")); }
      catch { document.execCommand("insertText", false, text); }
    } else document.execCommand("insertText", false, text);
    changed();
  }
  const date = (s: number) => new Date(s * 1000).toLocaleDateString([], { month: "short", day: "numeric" });
  const folderName = (id: number | null) => folders.find((f) => f.id === id)?.name ?? "No folder";
  const heading = $derived(filter.tag ? `#${filter.tag}` : filter.folder !== null ? folderName(filter.folder) : "All notes");
</script>

<div class="notes">
  <aside class="side">
    <div class="search"><Icon name="search" size={16} /><input placeholder="Search notes" bind:value={query} oninput={onSearch} aria-label="Search notes" /></div>
    <div class="sec">
      <div class="sechead"><span>Folders</span><button class="icon-btn sm" aria-label="New folder" title="New folder" onclick={() => (newFolder = "")}><Icon name="plus" size={14} /></button></div>
      <button class="row" class:active={filter.folder === null && filter.tag === null} onclick={() => setFilter({ folder: null, tag: null })}><Icon name="note" size={16} /> All notes</button>
      {#each folders as f (f.id)}
        <button class="row" class:active={filter.folder === f.id} onclick={() => setFilter({ folder: f.id, tag: null })}>
          <Icon name="folder" size={16} /> <span class="grow">{f.name}</span>{#if note?.folder_id === f.id}<span class="dot accent"></span>{/if}</button>
      {/each}
      {#if newFolder !== null}
        <input class="newin" placeholder="Folder name" bind:value={newFolder} maxlength="80" use:focusEl
          onkeydown={(e) => { if (e.key === "Enter") void addFolder(); if (e.key === "Escape") newFolder = null; }} onblur={addFolder} />
      {/if}
    </div>
    <div class="sec">
      <div class="sechead"><span>Tags</span></div>
      {#each tags as t (t.name)}
        <button class="row" class:active={filter.tag === t.name} onclick={() => setFilter({ folder: null, tag: t.name })}>
          <span class="dot" style:background={t.color}></span> <span class="grow">{t.name}</span><span class="muted small">{t.count}</span></button>
      {:else}<p class="muted small pad">Add tags to a note to see them here.</p>{/each}
    </div>
    <div class="sec list">
      <div class="sechead"><span>Notes</span><span class="muted small">Modified ↓</span></div>
      {#each notes as n (n.id)}
        <button class="nitem" class:active={note?.id === n.id} onclick={() => open(n.id)}>
          <span class="nt">{#if n.pinned}<Icon name="pin" size={12} /> {/if}{n.title || "Untitled note"}</span><span class="nd">{date(n.modified)}</span>
          <span class="ns">{n.snippet || "No text"}</span>
        </button>
      {:else}<p class="muted small pad">{query ? "No matching notes" : "No notes yet"}</p>{/each}
    </div>
  </aside>

  <section class="main">
    <div class="topbar">
      <span class="muted small">{heading}</span>
      <span class="grow"></span>
      {#if saving}<span class="muted small">{saving === "saving" ? "Saving…" : "Saved"}</span>{/if}
      <button class="btn primary" onclick={create}><Icon name="plus" size={15} /> New note</button>
      {#if note}
        <div class="more">
          <button class="btn" aria-label="More" aria-haspopup="menu" onclick={() => (menu = !menu)}>⋯</button>
          {#if menu}
            <div class="menu" role="menu">
              <button role="menuitem" onclick={() => { note!.pinned = !note!.pinned; menu = false; changed(); }}>{note.pinned ? "Unpin" : "Pin to top"}</button>
              <div class="muted small pad">Move to folder</div>
              {#each folders as f (f.id)}<button role="menuitem" onclick={() => moveTo(f.id)} disabled={note.folder_id === f.id}>{f.name}</button>{/each}
              <button role="menuitem" onclick={() => moveTo(null)} disabled={note.folder_id === null}>No folder</button>
              <hr /><button role="menuitem" class="danger" onclick={() => { menu = false; confirmDelete = true; }}>Delete note…</button>
            </div>
          {/if}
        </div>
      {/if}
    </div>
    {#if note}
      {#if confirmDelete}
        <div class="confirm"><span>Delete “{note.title || "Untitled note"}”? This can't be undone.</span>
          <button class="btn" onclick={() => (confirmDelete = false)}>Cancel</button><button class="btn danger" onclick={remove}>Delete</button></div>
      {/if}
      <input class="ntitle" bind:value={title} oninput={changed} maxlength="300" aria-label="Note title" placeholder="Title" />
      <div class="tools" role="toolbar" aria-label="Formatting">
        <select bind:value={block} onchange={() => setBlock(block)} aria-label="Text style">
          <option value="p">Paragraph</option><option value="h1">Heading 1</option><option value="h2">Heading 2</option><option value="h3">Heading 3</option><option value="pre">Code</option>
        </select>
        <button class="icon-btn" title="Bold (Ctrl+B)" aria-label="Bold" onclick={() => exec("bold")}><Icon name="bold" /></button>
        <button class="icon-btn" title="Italic (Ctrl+I)" aria-label="Italic" onclick={() => exec("italic")}><Icon name="italic" /></button>
        <button class="icon-btn" title="Underline (Ctrl+U)" aria-label="Underline" onclick={() => exec("underline")}><Icon name="underline" /></button>
        <button class="icon-btn" title="Strikethrough" aria-label="Strikethrough" onclick={() => exec("strikeThrough")}><Icon name="strike" /></button>
        <button class="icon-btn" title="Link" aria-label="Link" onclick={link}><Icon name="link" /></button>
        <span class="sep"></span>
        <button class="icon-btn" title="Bulleted list" aria-label="Bulleted list" onclick={() => exec("insertUnorderedList")}><Icon name="list" /></button>
        <button class="icon-btn" title="Checklist" aria-label="Checklist" onclick={checklist}><Icon name="checklist" /></button>
        <button class="icon-btn" title="Quote" aria-label="Quote" onclick={() => setBlock("blockquote")}><Icon name="quote" /></button>
        <button class="icon-btn" title="Divider" aria-label="Divider" onclick={() => exec("insertHorizontalRule")}><Icon name="hr" /></button>
      </div>
      <div class="scroll">
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div class="editor" bind:this={editor} contenteditable="true" role="textbox" aria-multiline="true" aria-label="Note text" tabindex="0"
          oninput={changed} onclick={onEditorClick} onkeydown={onKeydown} onpaste={onPaste}></div>
        <div class="meta">
          <div class="tags">
            {#each note.tags as t (t)}<span class="chip">#{t}<button aria-label="Remove tag {t}" onclick={() => removeTag(t)}>×</button></span>{/each}
            <input placeholder="+ tag" bind:value={tagInput} maxlength="80" onkeydown={(e) => { if (e.key === "Enter") addTag(); }} onblur={addTag} aria-label="Add tag" />
          </div>
          {#each note.links as l (l.message_id)}
            <div class="related"><Icon name="link" size={15} /> <b>Related email</b>
              <button class="echip" onclick={() => onOpenEmail(l)}><Icon name="mail" size={14} /> From: <span class="acc">{l.sender}</span> · <span class="acc">{l.subject}</span></button>
            </div>
            <div class="card">
              <Icon name="mail" size={22} />
              <div class="grow"><b>Linked email</b><div class="muted small">From: {l.sender} · <span class="acc">{l.subject}</span>{l.date ? ` · ${new Date(l.date * 1000).toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" })}` : ""}</div></div>
              <button class="icon-btn" title="Open email" aria-label="Open email" onclick={() => onOpenEmail(l)}><Icon name="external" /></button>
              <button class="icon-btn" title="Remove link" aria-label="Remove link" onclick={() => unlink(l.message_id)}><Icon name="close" size={14} /></button>
            </div>
          {/each}
        </div>
      </div>
    {:else}
      <div class="empty muted">No note selected. <button class="btn" onclick={create}>Create a note</button></div>
    {/if}
  </section>
</div>

<script module lang="ts">
  export function focusEl(el: HTMLElement) { el.focus(); }
</script>

<style>
  .notes { display: grid; grid-template-columns: 290px 1fr; height: 100%; min-height: 0; }
  .side { background: var(--panel-2); border-right: 1px solid var(--border); display: flex; flex-direction: column; min-height: 0; padding: 12px 10px; gap: 6px; }
  .search { display: flex; align-items: center; gap: 8px; padding: 0 10px; border: 1px solid var(--border); border-radius: 10px; background: var(--panel); color: var(--muted); margin-bottom: 6px; }
  .search input { border: 0; background: transparent; flex: 1; padding: 9px 0; outline: none; }
  .sec { display: flex; flex-direction: column; gap: 1px; border-bottom: 1px solid var(--border); padding-bottom: 8px; }
  .sec.list { flex: 1; min-height: 0; overflow: auto; border: 0; }
  .sechead { display: flex; justify-content: space-between; align-items: center; font-size: 11px; letter-spacing: .06em; text-transform: uppercase; color: var(--muted); padding: 6px 8px; }
  .row { display: flex; gap: 10px; align-items: center; border: 0; background: transparent; text-align: left; padding: 6px 8px; border-radius: 8px; font-size: 13px; }
  .row:hover { background: var(--hover); } .row.active { background: var(--selected); }
  .grow { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .dot { width: 9px; height: 9px; border-radius: 50%; flex: none; } .dot.accent { background: var(--accent); }
  .small { font-size: 12px; } .pad { padding: 4px 8px; margin: 0; }
  .sm { padding: 2px 4px; }
  .newin { margin: 4px 8px; font-size: 13px; }
  .nitem { display: grid; grid-template-columns: 1fr auto; gap: 2px 8px; border: 1px solid transparent; background: transparent; text-align: left; padding: 8px 10px; border-radius: 8px; }
  .nitem:hover { background: var(--hover); } .nitem.active { background: var(--selected); border-color: var(--accent); }
  .nt { font-weight: 600; font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .nd { font-size: 11px; color: var(--muted); }
  .ns { grid-column: 1 / -1; font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; }
  .main { display: flex; flex-direction: column; min-height: 0; min-width: 0; background: var(--panel); }
  .topbar { display: flex; align-items: center; gap: 8px; padding: 12px 22px; }
  .more { position: relative; }
  .menu { position: absolute; right: 0; top: calc(100% + 4px); z-index: 30; background: var(--panel); border: 1px solid var(--border); border-radius: 10px; padding: 4px; min-width: 190px; display: flex; flex-direction: column; box-shadow: 0 10px 30px #0006; }
  .menu button { border: 0; background: transparent; text-align: left; padding: 7px 12px; border-radius: 6px; }
  .menu button:hover:not(:disabled) { background: var(--hover); } .menu button:disabled { opacity: .45; }
  .menu hr { border: 0; border-top: 1px solid var(--border); margin: 4px 0; } .danger { color: #e5534b; }
  .confirm { display: flex; gap: 8px; align-items: center; margin: 0 22px 8px; padding: 8px 12px; border-radius: 10px; border: 1px solid #e5534b66; }
  .confirm span { flex: 1; font-size: 13px; }
  .ntitle { border: 0; background: transparent; font-size: 26px; font-weight: 700; padding: 4px 22px; outline: none; color: var(--text); }
  .tools { display: flex; align-items: center; gap: 2px; padding: 6px 22px; border-bottom: 1px solid var(--border); }
  .tools select { font-size: 13px; margin-right: 8px; }
  .sep { width: 1px; height: 20px; background: var(--border); margin: 0 6px; }
  .scroll { flex: 1; overflow: auto; padding: 8px 22px 24px; }
  .editor { outline: none; min-height: 200px; line-height: 1.55; font-size: 14px; }
  .editor :global(h1) { font-size: 22px; margin: 14px 0 6px; } .editor :global(h2) { font-size: 18px; margin: 14px 0 6px; } .editor :global(h3) { font-size: 15px; margin: 12px 0 4px; }
  .editor :global(p) { margin: 4px 0; } .editor :global(blockquote) { border-left: 3px solid var(--accent); margin: 6px 0; padding-left: 12px; color: var(--muted); }
  .editor :global(pre) { background: var(--panel-2); padding: 8px 10px; border-radius: 8px; }
  .editor :global(a) { color: var(--accent); }
  .editor :global(ul[data-checklist]) { list-style: none; padding-left: 4px; }
  .editor :global(ul[data-checklist] > li) { position: relative; padding-left: 28px; margin: 4px 0; }
  .editor :global(ul[data-checklist] > li::before) { content: ""; position: absolute; left: 0; top: 3px; width: 16px; height: 16px; border-radius: 4px; border: 1.5px solid var(--muted); cursor: pointer; }
  .editor :global(ul[data-checklist] > li[data-checked="true"]::before) { background: var(--accent) url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='white' stroke-width='3'%3E%3Cpath d='M5 12l5 5 9-10'/%3E%3C/svg%3E") center/12px no-repeat; border-color: var(--accent); }
  .editor :global(ul[data-checklist] > li[data-checked="true"]) { text-decoration: line-through; color: var(--muted); }
  .meta { margin-top: 18px; display: flex; flex-direction: column; gap: 10px; }
  .tags { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
  .tags input { width: 110px; font-size: 12px; padding: 4px 8px; }
  .chip { font-size: 12px; background: var(--accent-soft); border-radius: 12px; padding: 2px 4px 2px 10px; display: inline-flex; gap: 4px; align-items: center; }
  .chip button { border: 0; background: transparent; color: var(--muted); padding: 0 4px; }
  .related { display: flex; gap: 10px; align-items: center; font-size: 13px; }
  .echip { display: inline-flex; gap: 6px; align-items: center; border: 1px solid var(--border); background: var(--panel-2); border-radius: 8px; padding: 5px 10px; font-size: 13px; }
  .acc { color: var(--accent); }
  .card { display: flex; gap: 14px; align-items: center; border: 1px solid var(--border); border-radius: 10px; padding: 12px 14px; max-width: 420px; background: var(--panel-2); }
  .empty { margin: auto; display: flex; gap: 10px; align-items: center; }
</style>
