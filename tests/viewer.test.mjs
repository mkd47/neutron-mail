import assert from 'node:assert/strict';
import test from 'node:test';
import { componentScript, deferred } from './component-harness.mjs';

const message = (id) => ({ id, subject: `Message ${id}`, from_addr: `sender${id}@example.test`, sender_trusted: false });
const draft = (subject) => ({ subject });

function viewer(api = {}, pimApi = {}) {
  const events = { statuses: [], reloads: [], notes: [], completed: [], pimChanges: 0 };
  const component = componentScript('src/lib/Viewer.svelte', {
    api: { conversation: async () => [], ...api }, pimApi, dayAt: (days) => days,
    $props: () => ({
      message: message(1), account: { id: 10, email: 'one@example.test' }, isDraft: false, hoverUrl: null,
      onStatus: (...args) => events.statuses.push(args),
      onReload: (...args) => events.reloads.push(args),
      onComposerDone: (...args) => events.completed.push(args),
      onOpenNote: (...args) => events.notes.push(args),
      onPimChanged: () => events.pimChanges++,
    }),
  }, `({
    startCompose, toggleSource, openAtt, confirmAttachment, openTodo, addTodo, saveToNotes, trustSender, done,
    openCard, trustChanged, setRecipientsOpen(value) { recipientsOpen = value; },
    setMessage(value) { message = value; }, setAccount(value) { account = value; },
    setTodo(value) { todo = value; },
    state: () => ({ reply, source, sourceLoading, confirmOpen, todo, todoBusy, notesBusy, trustBusy, card, recipientsOpen })
  })`);
  component.flushEffects();
  return { ...component, events };
}

function select(component, id) {
  component.setMessage(message(id));
  component.flushEffects();
}

test('late compose prefills cannot attach an old draft after switching away and back', async () => {
  const pending = deferred();
  const c = viewer({ composePrefill: () => pending.promise });
  const request = c.startCompose('draft');
  select(c, 2);
  select(c, 1);
  pending.resolve(draft('Old draft'));
  await request;
  assert.equal(c.state().reply, null);
});

test('latest compose request wins and preserves its source message and account', async () => {
  const first = deferred(), second = deferred();
  const c = viewer({ composePrefill: (_, mode) => mode === 'reply' ? first.promise : second.promise });
  const older = c.startCompose('reply');
  const newer = c.startCompose('draft');
  second.resolve(draft('Latest draft'));
  await newer;
  const active = c.state().reply;
  assert.equal(active.mode, 'draft');
  assert.equal(active.origin.messageId, 1);
  assert.equal(active.account.id, 10);
  first.resolve(draft('Stale reply'));
  await older;
  assert.equal(c.state().reply, active);
  // A same-message account transition must not reuse another sender's composer.
  c.setAccount({ id: 20, email: 'two@example.test' });
  c.flushEffects();
  assert.equal(c.state().reply, null);
});

test('stale composer completion cannot close a newer composer', async () => {
  const c = viewer({ composePrefill: async () => draft('Reply') });
  await c.startCompose('reply');
  const previous = c.state().reply;
  await c.startCompose('forward');
  const active = c.state().reply;
  c.done(previous.key, previous.origin, { kind: 'sent' });
  assert.equal(c.state().reply, active);
  assert.equal(c.events.completed.length, 0);
  c.done(active.key, active.origin, { kind: 'draft' });
  assert.equal(c.state().reply, null);
  assert.equal(c.events.completed.length, 1);
});

test('raw source results respect selection changes and cancellation', async () => {
  const pending = [deferred(), deferred(), deferred()];
  let index = 0;
  const c = viewer({ messageSource: () => pending[index++].promise });
  const first = c.toggleSource();
  select(c, 2);
  const second = c.toggleSource();
  pending[0].resolve('Source of message 1');
  await first;
  assert.equal(c.state().source, null);
  assert.equal(c.state().sourceLoading, true);
  pending[1].resolve('Source of message 2');
  await second;
  assert.equal(c.state().source, 'Source of message 2');
  await c.toggleSource();
  const third = c.toggleSource();
  await c.toggleSource();
  pending[2].resolve('Cancelled source');
  await third;
  assert.equal(c.state().source, null);
  assert.equal(c.state().sourceLoading, false);
});

test('attachment confirmations remain bound to the original message', async () => {
  const pending = deferred();
  const calls = [];
  const c = viewer({ openAttachment: (...args) => { calls.push(args); return pending.promise; } });
  const opening = c.openAtt(3);
  select(c, 2);
  pending.resolve({ status: 'needs_confirmation', reason: 'Executable' });
  await opening;
  assert.equal(c.state().confirmOpen, null);
  assert.deepEqual(calls, [[1, 3, false]]);

  await c.openAtt(4);
  assert.equal(c.state().confirmOpen.origin.messageId, 2);
  // Props can change before Svelte's next effect runs. Even then, confirmation
  // must not apply the old attachment index to the new message.
  c.setMessage(message(3));
  c.confirmAttachment();
  assert.deepEqual(calls, [[1, 3, false], [2, 4, false]]);
  assert.equal(c.state().confirmOpen, null);
});

test('task submission snapshots title and settings before looking up its email link', async () => {
  const email = deferred(), saved = [];
  const c = viewer({}, {
    emailLink: (id) => { assert.equal(id, 1); return email.promise; },
    saveTask: async (value) => { saved.push(value); },
  });
  const original = { title: 'Original task', due: '1', list: 7, priority: 1 };
  c.setTodo(original);
  const adding = c.addTodo();
  await c.addTodo(); // A second click while busy must not submit twice.
  original.title = 'Changed while waiting';
  select(c, 2);
  const replacement = { title: 'Task for message 2', due: '', list: 8, priority: 0 };
  c.setTodo(replacement);
  email.resolve({ message_id: 'message-one' });
  await adding;
  assert.equal(saved.length, 1);
  assert.equal(saved[0].title, 'Original task');
  assert.equal(saved[0].list_id, 7);
  assert.equal(saved[0].email.message_id, 'message-one');
  assert.equal(c.state().todo, replacement);
  assert.equal(c.events.pimChanges, 1);
  assert.equal(c.events.statuses.length, 0);
});

test('late task-list and note requests do not reopen UI for another message', async () => {
  const lists = deferred(), note = deferred();
  const c = viewer({}, { taskLists: () => lists.promise, noteFromMessage: () => note.promise });
  const opening = c.openTodo();
  const saving = c.saveToNotes();
  select(c, 2);
  lists.resolve([{ id: 7 }]);
  note.resolve({ id: 9, title: 'Note from message 1' });
  await Promise.all([opening, saving]);
  assert.equal(c.state().todo, null);
  assert.equal(c.events.notes.length, 0);
  assert.equal(c.events.pimChanges, 1);
  assert.equal(c.events.statuses.length, 0);
});

test('sender-trust completion cannot load remote images for the next message', async () => {
  const pending = deferred(), calls = [];
  const c = viewer({ setImageSenderTrust: (...args) => { calls.push(args); return pending.promise; } });
  const trusting = c.trustSender();
  await c.trustSender();
  select(c, 2);
  pending.resolve();
  await trusting;
  assert.deepEqual(calls, [[10, 'sender1@example.test', true]]);
  assert.equal(c.events.reloads.length, 0);
  assert.equal(c.state().trustBusy, false);
});

test('unmounted viewers ignore outstanding compose and source completions', async () => {
  const compose = deferred(), source = deferred();
  const c = viewer({ composePrefill: () => compose.promise, messageSource: () => source.promise });
  const composing = c.startCompose('reply');
  const loading = c.toggleSource();
  c.destroy();
  compose.resolve(draft('Reply'));
  source.reject(new Error('Old failure'));
  await Promise.all([composing, loading]);
  assert.equal(c.state().reply, null);
  assert.equal(c.events.statuses.length, 0);
});

const chipClick = () => ({ currentTarget: { getBoundingClientRect: () => ({ left: 0 }) } });

test('clicking an address toggles its card, and switching messages closes it', () => {
  const c = viewer();
  const who = { name: 'Ayşe', email: 'ayse@example.test' };
  c.openCard(chipClick(), who);
  assert.equal(c.state().card.address.email, who.email);
  c.openCard(chipClick(), who);
  assert.equal(c.state().card, null);
  c.openCard(chipClick(), who); c.setRecipientsOpen(true);
  select(c, 2);
  assert.equal(c.state().card, null);
  assert.equal(c.state().recipientsOpen, false);
});

test('trusting the sender from its card re-renders the message; other addresses do not', () => {
  const c = viewer();
  c.trustChanged('someone-else@example.test', true);
  assert.deepEqual(c.events.reloads, []);
  c.trustChanged('SENDER1@example.test', true);
  c.trustChanged('sender1@example.test', false);
  assert.deepEqual(c.events.reloads, [[true], [false]]);
});
