<script lang="ts">
  // Rules editor: ordered rules with conditions/actions, local vs server
  // (Sieve via ManageSieve), a Sieve preview, test against a message and
  // "Run now" on a folder.
  import { onMount } from "svelte";
  import { Icon } from "@neutron/ui";
  import { errorText, rulesApi, type Account, type Action, type Condition, type Folder, type Rule,
    type RuleField, type RuleOp, type RuleTest, type SieveStatus } from "./api";

  let { account, folders, testMessage, onClose }:
    { account: Account; folders: Folder[]; testMessage: { id: number; subject: string } | null; onClose: () => void } = $props();

  let rules = $state<Rule[]>([]);
  let saved = $state("");
  let sel = $state(0);
  let msg = $state<{ text: string; error: boolean } | null>(null);
  let busy = $state(false);
  let sieve = $state<SieveStatus | null>(null);
  let sievePort = $state(4190);
  let showSieve = $state(false);
  let test = $state<RuleTest | null>(null);
  let runFolder = $state<number | null>(null);
  let pendingForward = $state<{ rule: Rule; to: string } | null>(null);

  const fields: [RuleField, string][] = [["from", "From"], ["to", "To"], ["cc", "Cc"], ["subject", "Subject"], ["body", "Body"],
    ["header", "Header…"], ["size", "Size"], ["has_attachment", "Has attachment"], ["account", "Account"]];
  const textOps: [RuleOp, string][] = [["contains", "contains"], ["equals", "equals"], ["starts_with", "starts with"], ["regex", "matches regex"]];
  const opsFor = (f: RuleField): [RuleOp, string][] =>
    f === "size" ? [["greater", "is larger than"], ["less", "is smaller than"]]
    : f === "has_attachment" ? [["is", "is yes"], ["is_not", "is no"]]
    : f === "account" ? [["is", "is"], ["is_not", "is not"]] : textOps;
  const actionLabels: Record<Action["type"], string> = { move: "Move to", copy: "Copy to", mark_read: "Mark as read", flag: "Star",
    label: "Add label", delete: "Delete", forward: "Forward to", stop: "Stop processing" };
  const targets = $derived(folders.filter((f) => f.role !== "inbox").map((f) => f.display).sort());
  const dirty = $derived(JSON.stringify(rules) !== saved);
  const rule = $derived(rules[sel] as Rule | undefined);

  onMount(async () => {
    try {
      rules = await rulesApi.list(); saved = JSON.stringify(rules);
      await loadSieve();
    } catch (e) { msg = { text: errorText(e), error: true }; }
  });
  async function loadSieve() {
    try { sieve = await rulesApi.sieveStatus(account.id); sievePort = sieve.settings.port; } catch (e) { msg = { text: errorText(e), error: true }; }
  }

  function addRule() {
    rules.push({ id: 0, name: "New rule", enabled: true, match_all: true, server_side: false,
      conditions: [{ field: "from", op: "contains", value: "", header: "" }], actions: [{ type: "move", folder: targets[0] ?? "" }] });
    sel = rules.length - 1; test = null;
  }
  function moveRule(i: number, d: number) {
    const j = i + d; if (j < 0 || j >= rules.length) return;
    [rules[i], rules[j]] = [rules[j], rules[i]]; sel = j;
  }
  function removeRule(i: number) { rules.splice(i, 1); sel = Math.max(0, Math.min(sel, rules.length - 1)); test = null; }
  function setField(c: Condition, f: RuleField) {
    c.field = f; c.op = opsFor(f)[0][0];
    if (f === "account") c.value = account.email;
    if (f === "has_attachment") c.value = "";
  }
  function newAction(t: Action["type"]): Action {
    switch (t) {
      case "move": case "copy": return { type: t, folder: targets[0] ?? "" };
      case "label": return { type: "label", keyword: "" };
      case "forward": return { type: "forward", to: "" };
      default: return { type: t } as Action;
    }
  }
  // Forwarding sends mail automatically: ask before a new address is kept.
  function confirmForward(r: Rule, a: Action) {
    if (a.type === "forward" && a.to.trim() && !JSON.parse(saved || "[]").some((x: Rule) => x.actions.some((y) => y.type === "forward" && y.to === a.to)))
      pendingForward = { rule: r, to: a.to };
  }

  async function save() {
    busy = true; msg = null;
    try {
      rules = await rulesApi.save(rules); saved = JSON.stringify(rules);
      msg = { text: sieve?.settings.active ? "Saved. Upload again to update the server script." : "Saved.", error: false };
      await loadSieve();
    } catch (e) { msg = { text: errorText(e), error: true }; } finally { busy = false; }
  }
  async function runTest() {
    if (!rule || !testMessage) return;
    try { test = await rulesApi.test($state.snapshot(rule), testMessage.id); msg = null; } catch (e) { test = null; msg = { text: errorText(e), error: true }; }
  }
  async function runNow() {
    if (runFolder === null) return;
    if (dirty) { msg = { text: "Save the rules first.", error: true }; return; }
    busy = true;
    try {
      const r = await rulesApi.runNow(runFolder);
      msg = { text: `Examined ${r.examined}, matched ${r.matched} (moved ${r.moved}, copied ${r.copied}, starred ${r.flagged}, forwarded ${r.forwarded})${r.errors.length ? ` · errors: ${r.errors.join("; ")}` : ""}`, error: r.errors.length > 0 };
    } catch (e) { msg = { text: errorText(e), error: true }; } finally { busy = false; }
  }
  async function upload() {
    if (dirty) { msg = { text: "Save the rules first.", error: true }; return; }
    busy = true;
    try {
      const ex = await rulesApi.sieveUpload(account.id, sievePort);
      msg = { text: `Sieve script with ${ex.exported.length} rule(s) uploaded and activated on ${account.imap_host}.`, error: false };
      await loadSieve();
    } catch (e) { msg = { text: `ManageSieve: ${errorText(e)}`, error: true }; } finally { busy = false; }
  }
  async function disable() {
    busy = true;
    try { await rulesApi.sieveDisable(account.id); msg = { text: "Server script removed; all rules run locally.", error: false }; await loadSieve(); }
    catch (e) { msg = { text: errorText(e), error: true }; } finally { busy = false; }
  }
  const localOnly = (id: number) => sieve?.preview.local_only.find(([i]) => i === id)?.[1];
</script>

<div class="rules">
  <header>
    <h2>Rules</h2>
    <span class="muted">Rules run in order on new Inbox mail. Server-side rules are filtered by your mail server (Sieve) even when Neutron is closed.</span>
    <button class="icon-btn" onclick={onClose} aria-label="Close"><Icon name="close" /></button>
  </header>
  <div class="cols">
    <aside>
      <ol>
        {#each rules as r, i (i)}
          <li class:active={i === sel}>
            <button class="pick" onclick={() => { sel = i; test = null; }}>
              <span class="num">{i + 1}</span>
              <span class="rname" class:off={!r.enabled}>{r.name || "(unnamed)"}</span>
              {#if r.server_side}<span class="chip" title={localOnly(r.id) ? `Runs locally: ${localOnly(r.id)}` : "Runs on the server"}>{localOnly(r.id) ? "local*" : "server"}</span>{/if}
            </button>
            <button class="icon-btn sm" aria-label="Move up" onclick={() => moveRule(i, -1)} disabled={i === 0}>↑</button>
            <button class="icon-btn sm" aria-label="Move down" onclick={() => moveRule(i, 1)} disabled={i === rules.length - 1}>↓</button>
          </li>
        {/each}
      </ol>
      <button class="btn" onclick={addRule}><Icon name="plus" size={14} /> New rule</button>
      <div class="sieve">
        <h3>Server filtering (ManageSieve)</h3>
        {#if sieve?.oauth}<p class="muted small">Not available for OAuth accounts yet; server-side rules run locally.</p>
        {:else}
          <p class="small">Status: <b>{sieve?.settings.active ? "active on server" : "not uploaded"}</b></p>
          <label class="small">Port <input type="number" min="1" max="65535" bind:value={sievePort} style="width:90px" /></label>
          <div class="row">
            <button class="btn" onclick={() => (showSieve = !showSieve)}>{showSieve ? "Hide" : "Preview"} Sieve</button>
            <button class="btn primary" onclick={upload} disabled={busy}>Upload &amp; activate</button>
            {#if sieve?.settings.active}<button class="btn" onclick={disable} disabled={busy}>Disable</button>{/if}
          </div>
        {/if}
      </div>
    </aside>

    <section class="edit">
      {#if showSieve && sieve}
        <div class="preview">
          <h3>Sieve preview <span class="muted small">({sieve.preview.exported.length} rule(s) exported{sieve.preview.local_only.length ? `, ${sieve.preview.local_only.length} kept local` : ""})</span></h3>
          <pre>{sieve.preview.script}</pre>
          {#each sieve.preview.local_only as [id, why] (id)}<p class="small warn">“{rules.find((r) => r.id === id)?.name}” stays local: {why}</p>{/each}
        </div>
      {/if}
      {#if rule}
        <div class="row">
          <input class="title" bind:value={rule.name} aria-label="Rule name" maxlength="100" />
          <label><input type="checkbox" bind:checked={rule.enabled} /> Enabled</label>
          <label title="Export to the server as Sieve when possible"><input type="checkbox" bind:checked={rule.server_side} /> Run on server</label>
          <button class="btn danger" onclick={() => removeRule(sel)}>Delete rule</button>
        </div>
        <div class="row">If
          <select bind:value={rule.match_all} aria-label="Match">
            <option value={true}>all</option><option value={false}>any</option>
          </select> of these conditions match:</div>
        {#each rule.conditions as c, ci (ci)}
          <div class="row cond" class:hit={test && test.conditions[ci]} class:miss={test && !test.conditions[ci]}>
            <select value={c.field} onchange={(e) => setField(c, (e.currentTarget as HTMLSelectElement).value as RuleField)} aria-label="Field">
              {#each fields as [v, l] (v)}<option value={v}>{l}</option>{/each}
            </select>
            {#if c.field === "header"}<input placeholder="Header name" bind:value={c.header} style="width:130px" />{/if}
            <select bind:value={c.op} aria-label="Operator">
              {#each opsFor(c.field) as [v, l] (v)}<option value={v}>{l}</option>{/each}
            </select>
            {#if c.field !== "has_attachment"}
              <input class="grow" bind:value={c.value} placeholder={c.field === "size" ? "e.g. 5M" : c.op === "regex" ? "regular expression" : "text"} aria-label="Value" />
            {/if}
            <button class="icon-btn sm" aria-label="Remove condition" onclick={() => rule.conditions.splice(ci, 1)} disabled={rule.conditions.length === 1}><Icon name="close" size={13} /></button>
          </div>
        {/each}
        <button class="btn sm" onclick={() => rule.conditions.push({ field: "subject", op: "contains", value: "", header: "" })}><Icon name="plus" size={13} /> Condition</button>

        <div class="row">Then:</div>
        {#each rule.actions as a, ai (ai)}
          <div class="row">
            <select value={a.type} aria-label="Action" onchange={(e) => (rule.actions[ai] = newAction((e.currentTarget as HTMLSelectElement).value as Action["type"]))}>
              {#each Object.entries(actionLabels) as [v, l] (v)}<option value={v}>{l}</option>{/each}
            </select>
            {#if a.type === "move" || a.type === "copy"}
              <select bind:value={a.folder} aria-label="Folder">{#each targets as t (t)}<option value={t}>{t}</option>{/each}</select>
            {:else if a.type === "label"}<input bind:value={a.keyword} placeholder="keyword, e.g. $work" />
            {:else if a.type === "forward"}<input type="email" bind:value={a.to} placeholder="address" onchange={() => confirmForward(rule, a)} />{/if}
            <button class="icon-btn sm" aria-label="Remove action" onclick={() => rule.actions.splice(ai, 1)} disabled={rule.actions.length === 1}><Icon name="close" size={13} /></button>
          </div>
        {/each}
        <button class="btn sm" onclick={() => rule.actions.push({ type: "mark_read" })}><Icon name="plus" size={13} /> Action</button>

        <div class="testbox">
          <button class="btn" onclick={runTest} disabled={!testMessage}>Test against message</button>
          <span class="muted small">{testMessage ? `“${testMessage.subject || "(no subject)"}”` : "Open a message first to test against it."}</span>
          {#if test}
            <div class="result" class:ok={test.matched}>
              {#if test.matched}
                <b>Matches.</b> Would:
                {[test.plan.mark_read && "mark read", test.plan.flag && "star", ...test.plan.keywords.map((k) => `label ${k}`),
                  ...test.plan.copies.map((f) => `copy to ${f}`), test.plan.delete ? "delete (to Trash)" : test.plan.move_to && `move to ${test.plan.move_to}`,
                  ...test.plan.forwards.map((f) => `forward to ${f}`)].filter(Boolean).join(", ") || "nothing"}
                {#each test.plan.skipped as s, i (i)}<div class="small warn">Skipped: {s}</div>{/each}
              {:else}<b>Doesn't match</b> (failing conditions are marked red).{/if}
            </div>
          {/if}
        </div>
      {:else}
        <p class="muted">No rules yet. Create one to sort, star, label or forward incoming mail.</p>
      {/if}
    </section>
  </div>
  <footer>
    {#if msg}<span class:error={msg.error} class="msg">{msg.text}</span>{/if}
    <span class="spacer"></span>
    <select bind:value={runFolder} aria-label="Folder to run rules on">
      <option value={null}>Run now on folder…</option>
      {#each folders as f (f.id)}<option value={f.id}>{f.role === "inbox" ? "Inbox" : f.display}</option>{/each}
    </select>
    <button class="btn" onclick={runNow} disabled={runFolder === null || busy}>Run now</button>
    <button class="btn primary" onclick={save} disabled={!dirty || busy}>Save rules</button>
  </footer>
  {#if pendingForward}
    <div class="modal" role="dialog" aria-modal="true">
      <div class="card">
        <h3>Forward mail automatically?</h3>
        <p>Messages matching “{pendingForward.rule.name}” will be forwarded to <b>{pendingForward.to}</b> without asking. Only do this for an address you control or trust.
          Automatic forwards are marked <code>Auto-Submitted</code>, never forward automated mail, and are limited to 20 per hour.</p>
        <div class="row end">
          <button class="btn" onclick={() => { const p = pendingForward!; p.rule.actions = p.rule.actions.filter((a) => !(a.type === "forward" && a.to === p.to)); if (!p.rule.actions.length) p.rule.actions.push({ type: "mark_read" }); pendingForward = null; }}>Remove forward</button>
          <button class="btn primary" onclick={() => (pendingForward = null)}>Forward to {pendingForward.to}</button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .rules { display: flex; flex-direction: column; height: 100%; max-width: 1150px; margin: 0 auto; padding: 18px 24px; gap: 12px; }
  header { display: flex; align-items: baseline; gap: 14px; }
  header h2 { margin: 0; } header .muted { flex: 1; font-size: 13px; }
  .cols { display: grid; grid-template-columns: 300px 1fr; gap: 18px; flex: 1; min-height: 0; }
  aside { display: flex; flex-direction: column; gap: 10px; overflow: auto; }
  ol { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
  li { display: flex; align-items: center; border-radius: 8px; } li.active { background: var(--selected); }
  .pick { flex: 1; display: flex; gap: 8px; align-items: center; border: 0; background: transparent; padding: 8px; text-align: left; min-width: 0; }
  .num { color: var(--muted); font-size: 12px; width: 18px; }
  .rname { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; } .off { opacity: .5; text-decoration: line-through; }
  .chip { font-size: 11px; border: 1px solid var(--accent); color: var(--accent); border-radius: 8px; padding: 0 6px; }
  .sm { padding: 2px 6px; font-size: 12px; }
  .sieve { border-top: 1px solid var(--border); padding-top: 10px; display: flex; flex-direction: column; gap: 6px; }
  h3 { margin: 4px 0; font-size: 14px; }
  .edit { overflow: auto; display: flex; flex-direction: column; gap: 8px; padding-right: 6px; }
  .row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; } .row.end { justify-content: flex-end; }
  .title { font-size: 16px; font-weight: 600; flex: 1; min-width: 200px; }
  .grow { flex: 1; min-width: 160px; }
  .cond { border-left: 3px solid transparent; padding-left: 6px; }
  .cond.hit { border-left-color: #3fb27f; } .cond.miss { border-left-color: #e5534b; }
  .preview pre { background: var(--panel-2); border: 1px solid var(--border); border-radius: 8px; padding: 10px; font-size: 12px; max-height: 260px; overflow: auto; white-space: pre-wrap; }
  .testbox { border-top: 1px solid var(--border); margin-top: 8px; padding-top: 10px; display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
  .result { width: 100%; padding: 8px 10px; border-radius: 8px; background: var(--panel-2); border-left: 3px solid #e5534b; font-size: 13px; }
  .result.ok { border-left-color: #3fb27f; }
  .small { font-size: 12px; } .warn { color: #f5b14c; } .error { color: #e5534b; }
  footer { display: flex; gap: 8px; align-items: center; border-top: 1px solid var(--border); padding-top: 10px; }
  .msg { font-size: 13px; max-width: 55%; } .spacer { flex: 1; }
  .modal { position: fixed; inset: 0; background: #0008; display: grid; place-items: center; z-index: 80; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: 12px; padding: 18px 20px; max-width: 480px; }
</style>
