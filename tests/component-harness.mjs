// Exercise the actual component handlers with controlled API promises. Svelte's
// DOM/reactivity is deliberately outside this unit harness; check/build covers
// compilation, and callers explicitly run lifecycle effects when needed.
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';

export function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

export function componentScript(path, bindings = {}, expose = '({})') {
  let source = readFileSync(new URL(`../${path}`, import.meta.url), 'utf8').match(/<script lang="ts">([\s\S]*?)<\/script>/)[1];
  const ast = ts.createSourceFile(path, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  for (const statement of [...ast.statements].reverse()) {
    if (ts.isImportDeclaration(statement)) source = source.slice(0, statement.pos) + source.slice(statement.end);
  }
  const effects = [], beforeEffects = [], mounts = [], destroys = [];
  const effect = (fn) => effects.push(fn);
  effect.pre = (fn) => beforeEffects.push(fn);
  const derived = (value) => value;
  derived.by = (fn) => fn();
  const timers = new Map();
  let timerId = 0;
  const context = {
    console, Promise, Set, Map, Date,
    $state: (value) => value, $derived: derived, $effect: effect,
    $props: () => ({}), $bindable: (value) => value,
    onMount: (fn) => mounts.push(fn), onDestroy: (fn) => destroys.push(fn),
    untrack: (fn) => fn(), tick: () => Promise.resolve(),
    setTimeout: (fn) => { const id = ++timerId; timers.set(id, fn); return id; },
    clearTimeout: (id) => timers.delete(id),
    localStorage: { getItem: () => null, setItem: () => {} },
    matchMedia: () => ({ matches: false }),
    document: { documentElement: { dataset: {} } },
    errorText: (error) => String(error),
    ...bindings,
  };
  const { outputText } = ts.transpileModule(`${source}\n;globalThis.__handlers = ${expose};`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None },
  });
  runInNewContext(outputText, context, { filename: path });
  return {
    ...context.__handlers,
    flushEffects() { for (const fn of [...beforeEffects, ...effects]) fn(); },
    async mount() { for (const fn of mounts) await fn(); },
    destroy() { for (const fn of destroys) fn(); },
    async runTimers() { const queued = [...timers.values()]; timers.clear(); await Promise.all(queued.map((fn) => fn())); },
  };
}
