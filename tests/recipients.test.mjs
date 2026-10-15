import test from 'node:test';
import assert from 'node:assert/strict';
import { componentScript, deferred } from './component-harness.mjs';

function recipients(api = {}) {
  return componentScript('src/lib/RecipientField.svelte', {
    api, $props: () => ({ label: 'To', accountId: 1, values: ['first@example.test'] }),
  }, `({onInput, onKey, onBlur, setText: (v) => text = v,
    getValues: () => values, getSuggestions: () => suggestions,
    setSuggestions: (v) => { suggestions = v; open = true; active = 0; }})`);
}

test('blur commits a pending recipient synchronously before Send click', () => {
  const c = recipients();
  c.setText('second@example.test'); c.onBlur();
  assert.deepEqual([...c.getValues()], ['first@example.test', 'second@example.test']);
});

test('send shortcuts commit the typed address before bubbling', () => {
  for (const modifier of ['ctrlKey', 'metaKey']) {
    const c = recipients(); let prevented = false, stopped = false;
    c.setText('second@example.test');
    c.setSuggestions([{name: 'Different contact', email: 'different@example.test'}]);
    c.onKey({key: 'Enter', [modifier]: true,
      preventDefault: () => prevented = true, stopPropagation: () => stopped = true});
    assert.deepEqual([...c.getValues()], ['first@example.test', 'second@example.test']);
    assert.equal(prevented, false); assert.equal(stopped, false);
  }
});

test('ordinary Enter still commits the selected contact suggestion', () => {
  const c = recipients(); let prevented = false;
  c.setSuggestions([{name: 'Contact', email: 'contact@example.test'}]);
  c.onKey({key: 'Enter', preventDefault: () => prevented = true});
  assert.equal(c.getValues()[1], 'Contact <contact@example.test>');
  assert.equal(prevented, true);
});

test('clearing input invalidates a contact lookup already in flight', async () => {
  const request = deferred();
  const c = recipients({suggestContacts: () => request.promise});
  c.setText('co'); c.onInput(); const pending = c.runTimers();
  c.setText(''); c.onInput();
  request.resolve([{name: 'Contact', email: 'contact@example.test'}]); await pending;
  assert.equal(c.getSuggestions().length, 0);
});
