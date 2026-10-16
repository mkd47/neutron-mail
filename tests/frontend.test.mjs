import assert from 'node:assert/strict';
import test from 'node:test';
import { componentScript, deferred } from './component-harness.mjs';

// The conversation list and the plain list share the same race handling.
const threads = (api) => ({ listThreads: api.listMessages, ...api });
const draft = { to: ['friend@example.test'], cc: [], bcc: [], subject: 'Hello', body: 'Body', html: null, references: [], in_reply_to: null, attachments: [] };
const account = { id: 1, email: 'me@example.test' };
const message = (id) => ({ id, seen: true, folder_id: 10, remote_images_failed: 0 });
const report = { failed_folders: [], new_messages: 0, rules_matched: 0, rule_errors: [] };

function composer(api) {
  return componentScript('src/lib/Composer.svelte', {
    $props: () => ({ account, draft, title: 'New message', onDone() {} }), api,
  }, '({send, saveDraft, onKey})');
}

test('repeated send shortcut cannot submit a second SMTP request', async () => {
  const request = deferred(); let calls = 0;
  const c = composer({ send() { calls++; return request.promise; } });
  const sending = c.send();
  c.onKey({ ctrlKey: true, key: 'Enter', preventDefault() {} });
  await c.send();
  assert.equal(calls, 1);
  request.resolve({ sent_copy: 'saved', draft_cleanup_error: null });
  await sending;
});

test('send shortcut is blocked while a draft save is in flight', async () => {
  const request = deferred(); let sends = 0;
  const c = composer({ saveDraft: () => request.promise, send: async () => { sends++; return { sent_copy: 'saved' }; } });
  const saving = c.saveDraft();
  await c.send();
  assert.equal(sends, 0);
  request.resolve(); await saving;
});

function app(api) {
  return componentScript('src/App.svelte', { api: threads(api), pimApi: {} }, `({
    open, reload, selectAccount, selectFolder, onSearch,
    setAccount(a){account=a}, setQuery(q){query=q},
    getState:()=>({detail, selectedId, account, folders, folder, hits})
  })`);
}

test('out-of-order message requests cannot replace the selected message', async () => {
  const one = deferred(), two = deferred();
  const c = app({ getMessage: (id) => id === 1 ? one.promise : two.promise });
  c.setAccount(account);
  const old = c.open(message(1)), latest = c.open(message(2));
  two.resolve(message(2)); await latest;
  one.resolve(message(1)); await old;
  assert.equal(c.getState().detail.id, 2);
  assert.equal(c.getState().selectedId, 2);
});

test('late image reload cannot resurrect a message after folder navigation', async () => {
  const reload = deferred(); let calls = 0;
  const c = app({ getMessage: () => ++calls === 1 ? Promise.resolve(message(1)) : reload.promise, listMessages: async () => [] });
  c.setAccount(account); await c.open(message(1));
  const loading = c.reload(true);
  await c.selectFolder({ id: 20 });
  reload.resolve(message(1)); await loading;
  assert.equal(c.getState().detail, null);
  assert.equal(c.getState().folder.id, 20);
});

test('late account folder response cannot install folders from the previous account', async () => {
  const old = deferred(), latest = deferred();
  const c = app({ listFolders: (id) => id === 1 ? old.promise : latest.promise, listMessages: async () => [], syncAccount: async () => report });
  const first = c.selectAccount(account), second = c.selectAccount({ id: 2 });
  latest.resolve([{ id: 20, account_id: 2, role: 'inbox' }]); await second;
  old.resolve([{ id: 10, account_id: 1, role: 'inbox' }]); await first;
  assert.equal(c.getState().account.id, 2);
  assert.equal(c.getState().folders[0].account_id, 2);
});

test('clearing a search invalidates its in-flight results', async () => {
  const search = deferred();
  const c = app({ search: () => search.promise });
  c.setAccount(account); c.setQuery('old'); c.onSearch();
  const timer = c.runTimers();
  c.setQuery(''); c.onSearch();
  search.resolve([message(1)]); await timer;
  assert.equal(c.getState().hits, null);
});

const note = (id) => ({ id, title: 'Original', html: '<p>Original</p>', folder_id: null, tags: [], pinned: false, links: [] });
function notes(saveNote) {
  return componentScript('src/lib/Notes.svelte', {
    $props: () => ({ onOpenEmail() {}, onStatus() {} }),
    pimApi: { saveNote, notes: async () => [], noteFolders: async () => [], tags: async () => [] },
  }, `({changed, flush, setNote(n,e){note=n;title=n.title;editor=e},
    editTitle(t){title=t}, detachEditor(){editor=null},
    getState:()=>({note,title,dirty})})`);
}

test('leaving Notes before debounce retains the edited HTML after the editor detaches', async () => {
  const saved = [];
  const c = notes(async (input) => { saved.push(input); return input; });
  c.setNote(note(1), { innerHTML: '<p>Unsaved body</p>' }); c.changed();
  c.detachEditor(); c.destroy(); await c.flush();
  assert.equal(saved.length, 1);
  assert.equal(saved[0].html, '<p>Unsaved body</p>');
});

test('note saves are serialized and an old save response cannot erase later edits', async () => {
  const first = deferred(), second = deferred(); const sent = [];
  const c = notes((input) => { sent.push(input); return sent.length === 1 ? first.promise : second.promise; });
  const editor = { innerHTML: '<p>First</p>' };
  c.setNote(note(1), editor); c.changed(); const saving = c.flush();
  await Promise.resolve();
  editor.innerHTML = '<p>Second</p>'; c.editTitle('Second'); c.changed(); const newer = c.flush();
  assert.equal(sent.length, 1);
  first.resolve({ ...note(1), ...sent[0] }); await saving;
  assert.equal(c.getState().note.html, '<p>Second</p>');
  assert.equal(c.getState().title, 'Second');
  await Promise.resolve();
  second.resolve({ ...note(1), ...sent[1] }); await newer;
  assert.equal(sent[1].html, '<p>Second</p>');
  assert.equal(sent[1].title, 'Second');
});

const task = (id) => ({ id, title: 'Task '+id, notes: '', list_id: 1, parent_id: null, due: null, due_time: false, reminder: null, priority: 0, email: null, subtasks: [] });
function todo(saveTask) {
  return componentScript('src/lib/Todo.svelte', {
    $props: () => ({ onOpenEmail() {}, onOpenMessage() {}, onStatus() {} }),
    dayEnd: () => 100000, dayAt: () => 50000, taskInput: (t) => ({ ...t }),
    pimApi: { saveTask, taskLists: async () => [], taskCounts: async () => ({}), tasks: async () => [], flaggedEmails: async () => [] },
  }, '({edit,save,selectTask,getCurrent:()=>current})');
}

test('task autosave retains the original task when selection changes before debounce', async () => {
  const request = deferred(); const saved = [];
  const c = todo((input) => { saved.push(input); return request.promise; });
  const first = task(1); c.selectTask(first); first.title = 'Edited task 1'; c.edit();
  c.selectTask(task(2)); await Promise.resolve();
  assert.equal(saved[0].id, 1);
  assert.equal(saved[0].title, 'Edited task 1');
  request.resolve(saved[0]); await c.save();
  // The response must not reopen task 1 (a list refresh may close task 2).
  assert.notEqual(c.getCurrent()?.id, 1);
});

test('closing task details before debounce still saves the edit', async () => {
  const saved = [];
  const c = todo(async (input) => { saved.push(input); return input; });
  const first = task(1); c.selectTask(first); first.notes = 'Keep this'; c.edit();
  c.selectTask(null); await c.save();
  assert.equal(saved[0].notes, 'Keep this');
  assert.equal(c.getCurrent(), null);
});

test('folder discovery refreshes the sidebar while the account sync is still pending', async () => {
  let reads = 0;
  const c = componentScript('src/App.svelte', { api: threads({
    listFolders: async () => { reads++; return [{id:10,role:'inbox'}, {id:11,name:'已发送'}]; },
    listMessages: async () => [],
  }), pimApi: {} }, '({onSyncProgress,setAccount(a){account=a},state:()=>({folders,status})})');
  c.setAccount(account);
  await c.onSyncProgress({account_id:1,stage:'folders',total_folders:2});
  assert.equal(reads,1);
  assert.equal(c.state().folders.length,2);
  assert.match(c.state().status,/Found 2 folders/);
  await c.onSyncProgress({account_id:2,stage:'folders',total_folders:99});
  assert.equal(reads,1);
});

test('committed download pages become visible before the final sync response', async () => {
  let reads = 0;
  const c = componentScript('src/App.svelte', { api: threads({
    listFolders: async () => [{id:10,role:'inbox'}], listMessages: async () => {reads++;return []},
  }), pimApi: {} }, '({onSyncProgress,setAccount(a){account=a},state:()=>({status})})');
  c.setAccount(account);
  await c.onSyncProgress({account_id:1,stage:'messages',folder:'Inbox',completed_folders:0,total_folders:3,downloaded:50,total_messages:200});
  assert.equal(reads,1);
  assert.match(c.state().status,/50\/200 messages/);
});

test('download progress coalesces list refreshes instead of reloading per page', async () => {
  let reads = 0;
  const pending = [];
  const c = componentScript('src/App.svelte', { api: threads({
    listFolders: async () => [{id:10,role:'inbox'}],
    listMessages: () => { reads++; const d = deferred(); pending.push(d); return d.promise; },
  }), pimApi: {} }, '({onSyncProgress,setAccount(a){account=a}})');
  c.setAccount(account);
  const first = c.onSyncProgress({account_id:1,stage:'messages',folder:'Inbox',completed_folders:0,total_folders:1,downloaded:50,total_messages:500});
  await new Promise((r) => setImmediate(r));
  for (let n = 100; n <= 500; n += 50)
    await c.onSyncProgress({account_id:1,stage:'messages',folder:'Inbox',completed_folders:0,total_folders:1,downloaded:n,total_messages:500});
  assert.equal(reads, 1);
  pending[0].resolve([]);
  await new Promise((r) => setImmediate(r));
  assert.equal(reads, 1);
  void first;
});

function mailbox(api, folderRole = 'inbox') {
  const folders = [{ id: 10, role: 'inbox', name: 'INBOX', display: 'Inbox', unread: 1, total: 3 },
    { id: 11, role: 'trash', name: 'Trash', display: 'Trash' }, { id: 12, role: 'archive', name: 'Archive', display: 'Archive' },
    { id: 13, role: 'junk', name: 'Junk', display: 'Junk' }];
  const rows = [1, 2, 3].map((id) => ({ id, folder_id: 10, seen: id !== 3, flagged: id === 2, subject: `m${id}` }));
  const calls = [];
  const c = componentScript('src/App.svelte', { api: threads({
    listFolders: async () => folders, listMessages: async () => rows,
    moveMessages: async (ids, target, copy) => { calls.push(['move', ids, target, copy]); return { account_id: 1, target_folder_id: target, copy, items: [], untracked: 0, count: ids.length }; },
    deleteMessages: async (ids) => { calls.push(['delete', ids]); return ids.length; },
    setMessageFlag: async (ids, flag, on) => { calls.push(['flag', ids, flag, on]); return ids.length; },
    ...api,
  }), pimApi: {} }, `({ deleteSelected, purge, moveToRole, toggleRead, toggleStar, setFlag,
    setup(a, fs, f, ms) { account = a; folders = fs; folder = f; messages = ms; threaded = false; },
    pick(ids) { picked = ids; }, state: () => ({ messages, picked, confirmPurge, lastTransfer }) })`);
  c.setup(account, folders, folders.find((f) => f.role === folderRole), rows.map((r) => ({ ...r })));
  return { c, calls };
}

test('Delete moves to Trash with undo; in Trash it asks before deleting permanently', async () => {
  const { c, calls } = mailbox();
  c.pick([1, 2]);
  await c.deleteSelected();
  assert.equal(JSON.stringify(calls[0]), JSON.stringify(['move', [1, 2], 11, false]));
  assert.deepEqual(c.state().messages.map((m) => m.id), [3]);
  assert.ok(c.state().lastTransfer);

  const trash = mailbox({}, 'trash');
  trash.c.pick([3]);
  await trash.c.deleteSelected();
  assert.deepEqual(trash.calls, []);
  assert.equal(JSON.stringify(trash.c.state().confirmPurge), '{"ids":[3]}');
  await trash.c.purge();
  assert.deepEqual(trash.calls, [['delete', [3]]]);
  assert.deepEqual(trash.c.state().messages.map((m) => m.id), [1, 2]);
});

test('Shift+Delete always asks first, even outside Trash', async () => {
  const { c, calls } = mailbox();
  await c.deleteSelected([1], true);
  assert.deepEqual(calls, []);
  assert.equal(JSON.stringify(c.state().confirmPurge), '{"ids":[1]}');
});

test('archive and junk move to the role folders', async () => {
  const { c, calls } = mailbox();
  await c.moveToRole('archive', [1]);
  await c.moveToRole('junk', [2]);
  assert.deepEqual(calls.map((x) => x[2]), [12, 13]);
});

test('read and star toggles follow the selection and update the list', async () => {
  const { c, calls } = mailbox();
  await c.toggleRead([1, 3]);           // one unread -> mark all read
  await c.toggleRead([1]);              // all read -> mark unread
  await c.toggleStar([2]);              // starred -> unstar
  await c.toggleStar([1, 2]);           // not all starred -> star
  assert.deepEqual(calls.map((x) => x.slice(1)), [
    [[1, 3], 'seen', true], [[1], 'seen', false], [[2], 'flagged', false], [[1, 2], 'flagged', true],
  ]);
  const byId = Object.fromEntries(c.state().messages.map((m) => [m.id, m]));
  assert.equal(byId[1].seen, false);
  assert.equal(byId[3].seen, true);
  assert.ok(byId[1].flagged && byId[2].flagged);
});

test('the composer sends from the primary address unless an alias is chosen', async () => {
  const sent = [];
  const make = (d) => componentScript('src/lib/Composer.svelte', {
    $props: () => ({ account, draft: d, title: 'Reply', onDone() {} }),
    api: { send: async (_id, m) => { sent.push(m.from); return { sent_copy: 'saved' }; } },
  }, '({ send, setFrom(v) { from = v; } })');
  await make(draft).send();
  const reply = make({ ...draft, from: 'sales@example.test' });
  await reply.send();
  reply.setFrom('ME@example.test');
  await reply.send();
  assert.deepEqual(sent, [null, 'sales@example.test', null]);
});

test('in conversation view, actions on a row apply to every message of the conversation', async () => {
  const folders = [{ id: 10, role: 'inbox', name: 'INBOX', display: 'Inbox', total: 3 }, { id: 11, role: 'trash', name: 'Trash', display: 'Trash' }];
  const rows = [{ id: 5, folder_id: 10, seen: true, flagged: false, thread_size: 3, thread_unread: 1, thread_flagged: false, thread_ids: [5, 3, 1] },
    { id: 4, folder_id: 10, seen: true, flagged: true, thread_size: 1, thread_unread: 0, thread_flagged: true, thread_ids: [4] }];
  const calls = [];
  const c = componentScript('src/App.svelte', { api: {
    listFolders: async () => folders, listThreads: async () => rows.map((r) => ({ ...r })),
    moveMessages: async (ids, target) => { calls.push(['move', [...ids], target]); return { count: ids.length, items: [], copy: false }; },
    setMessageFlag: async (ids, flag, on) => { calls.push(['flag', [...ids], flag, on]); return ids.length; },
  }, pimApi: {} }, `({ deleteSelected, toggleRead, toggleStar, selection,
    setup(a, fs, f, ms) { account = a; folders = fs; folder = f; messages = ms; }, pick(ids) { picked = ids; } })`);
  c.setup(account, folders, folders[0], rows.map((r) => ({ ...r })));
  c.pick([5]);
  assert.deepEqual([...c.selection()].sort(), [1, 3, 5]);
  await c.toggleRead();                 // the conversation has an unread message -> read
  await c.toggleStar();                 // not starred -> star all
  c.pick([4]);
  await c.toggleStar();                 // starred -> unstar
  c.pick([5]);
  await c.deleteSelected();
  assert.equal(JSON.stringify(calls), JSON.stringify([
    ['flag', [5, 3, 1], 'seen', true], ['flag', [5, 3, 1], 'flagged', true], ['flag', [4], 'flagged', false], ['move', [5, 3, 1], 11],
  ]));
});
