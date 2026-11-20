import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { flush } from './hook-harness.mjs';

const memoryStorage = (initial = {}) => {
  const values = new Map(Object.entries(initial));
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => { values.set(key, String(value)); }, removeItem: key => { values.delete(key); } };
};
function load(vocabulary = []) {
  const calls = [];
  let saved = [...vocabulary];
  const invoke = async (command, args) => {
    calls.push([command, args && JSON.parse(JSON.stringify(args))]);
    if (command === 'get_custom_vocabulary') return [...saved];
    if (command === 'set_custom_vocabulary') { saved = [...args.words]; return [...saved]; }
    throw new Error(`Unexpected command: ${command}`);
  };
  const learning = loadTsModule('src/lib/vocabulary-learning.ts', { '@tauri-apps/api/core': { invoke } });
  return { learning, calls, saved: () => saved };
}
const list = value => Array.from(value);

test('learning from corrections is off until the person turns it on', () => {
  const { learning } = load();
  const storage = memoryStorage();
  assert.equal(learning.readLearnVocabulary(storage), false);
  assert.equal(learning.writeLearnVocabulary(true, storage), true);
  assert.equal(learning.readLearnVocabulary(storage), true);
  assert.equal(learning.writeLearnVocabulary(false, storage), true);
  assert.equal(learning.readLearnVocabulary(storage), false);
  assert.equal(learning.readLearnVocabulary({ getItem() { throw new Error('blocked'); } }), false);
  assert.equal(learning.writeLearnVocabulary(true, null), false);
});

test('a correction that swaps a misheard word for a name or term is suggested', () => {
  const { learning } = load();
  const suggest = (before, after, known) => list(learning.vocabularySuggestions(before, after, known));
  assert.deepEqual(suggest('We use charge be for billing.', 'We use ChargeBee for billing.'), ['ChargeBee']);
  assert.deepEqual(suggest('Ask dear about it.', 'Ask Dave about it.'), ['Dave']);
  assert.deepEqual(suggest('Call Anna Meyer today', 'Call Anna Meier-Schmidt today'), ['Meier-Schmidt']);
  assert.deepEqual(suggest('the kubernetes cluster', 'the Kubernetes cluster'), ['Kubernetes'], 'a spelling change of a term');
  assert.deepEqual(suggest('Rollout of version two', 'Rollout of Version 2'), ['Version 2']);
  assert.deepEqual(suggest('We met at Nürnberg', 'We met in Nürnberg'), [], 'a plain lowercase word is not a term');
});

test('edits that are not new names or terms are ignored', () => {
  const { learning } = load();
  const suggest = (before, after, known) => list(learning.vocabularySuggestions(before, after, known));
  assert.deepEqual(suggest('we need this', 'We need this'), [], 'sentence capitalisation');
  assert.deepEqual(suggest('okay then. the plan', 'okay then. The plan'), [], 'capitalisation only');
  assert.deepEqual(suggest('We use charge be', 'We use charge be and ChargeBee'), [], 'a pure insertion');
  assert.deepEqual(suggest('We use charge be here', 'We use here'), [], 'a pure deletion');
  assert.deepEqual(suggest('A short note.', 'Completely Different Words Replace Everything Here'), [], 'a rewrite is too long to be a term');
  assert.deepEqual(suggest('We use charge be', 'We use ChargeBee', ['chargebee']), [], 'already in the vocabulary, any case');
  assert.deepEqual(suggest('', 'Dave'), []);
  assert.deepEqual(suggest('dear', ''), []);
  assert.deepEqual(suggest('a '.repeat(500), 'B '.repeat(500)), [], 'oversized text is skipped');
  assert.deepEqual(suggest('x dear y charge be z', 'x Dave y ChargeBee z').sort(), ['ChargeBee', 'Dave']);
});

test('a find-and-replace replacement is offered only when it looks like a term', () => {
  const { learning } = load();
  assert.deepEqual(list(learning.replacementSuggestion('  ChargeBee ')), ['ChargeBee']);
  assert.deepEqual(list(learning.replacementSuggestion('billing')), []);
  assert.deepEqual(list(learning.replacementSuggestion('Dave', ['dave'])), []);
  assert.deepEqual(list(learning.replacementSuggestion('A B C D E')), []);
  assert.deepEqual(list(learning.replacementSuggestion('')), []);
});

test('words follow the same rules as the vocabulary settings', () => {
  const { learning } = load();
  assert.equal(learning.isValidVocabularyWord('ChargeBee'), true);
  assert.equal(learning.isValidVocabularyWord('Müller-Lüdenscheid'), true);
  assert.equal(learning.isValidVocabularyWord('x'.repeat(65)), false);
  assert.equal(learning.isValidVocabularyWord('...'), false);
  assert.equal(learning.isValidVocabularyWord('two\nlines'), false);
  assert.equal(learning.isValidVocabularyWord(' padded'), false);
});

test('adding appends only new words once, and undo removes only those', async () => {
  const { learning, calls, saved } = load(['Rismondo']);
  assert.deepEqual(list(await learning.addVocabularyWords(['ChargeBee', 'rismondo', 'ChargeBee', 'Dave'])), ['ChargeBee', 'Dave']);
  assert.deepEqual(saved(), ['Rismondo', 'ChargeBee', 'Dave']);
  assert.deepEqual(list(await learning.addVocabularyWords(['chargebee'])), []);
  assert.equal(calls.filter(([command]) => command === 'set_custom_vocabulary').length, 1, 'nothing is written when nothing is new');
  await learning.removeVocabularyWords(['chargebee', 'Dave']);
  assert.deepEqual(saved(), ['Rismondo']);
});

test('a full vocabulary refuses additions instead of truncating', async () => {
  const { learning, saved } = load(Array.from({ length: 500 }, (_, index) => `Word${index}`));
  await assert.rejects(learning.addVocabularyWords(['Extra']), /vocabulary is full/);
  assert.equal(saved().length, 500);
});

function offerSetup({ enabled = true, vocabulary = [] } = {}) {
  const { learning, calls, saved } = load(vocabulary);
  const toasts = [];
  const toast = Object.assign((message, options) => toasts.push({ kind: 'offer', message, options }), {
    success: (message, options) => toasts.push({ kind: 'success', message, options }),
    error: (message, options) => toasts.push({ kind: 'error', message, options }),
  });
  const previousWindow = globalThis.window;
  globalThis.window = { localStorage: memoryStorage(enabled ? { 'clawscribe.learnVocabularyFromCorrections': 'true' } : {}) };
  const offer = loadTsModule('src/lib/vocabulary-offer.ts', { sonner: { toast }, '@tauri-apps/api/core': { invoke: async (command, args) => {
    if (command === 'get_custom_vocabulary') return saved();
    return args ? undefined : undefined;
  } } });
  return { offer, toasts, calls, saved, restore: () => { globalThis.window = previousWindow; } };
}

test('no offer is made while learning is off, and none when the word is already known', async () => {
  for (const [enabled, vocabulary] of [[false, []], [true, ['ChargeBee']]]) {
    const view = offerSetup({ enabled, vocabulary });
    try {
      await view.offer.offerVocabularyWords(['ChargeBee']);
      assert.equal(view.toasts.length, 0);
    } finally { view.restore(); }
  }
});

test('an offer adds the word only on a click and can be undone', async () => {
  const written = [];
  let stored = ['Rismondo'];
  const toasts = [];
  const toast = Object.assign((message, options) => toasts.push({ kind: 'offer', message, options }), {
    success: (message, options) => toasts.push({ kind: 'success', message, options }),
    error: (message, options) => toasts.push({ kind: 'error', message, options }),
  });
  const previousWindow = globalThis.window;
  globalThis.window = { localStorage: memoryStorage({ 'clawscribe.learnVocabularyFromCorrections': 'true' }) };
  try {
    const learning = loadTsModule('src/lib/vocabulary-learning.ts', { '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'get_custom_vocabulary') return [...stored];
      written.push([...args.words]); stored = [...args.words]; return stored;
    } } });
    const { offerVocabularyWords } = loadTsModule('src/lib/vocabulary-offer.ts', { sonner: { toast }, '@/lib/vocabulary-learning': learning });
    await offerVocabularyWords(['ChargeBee', 'rismondo']);
    assert.equal(toasts.length, 1);
    assert.equal(toasts[0].message, 'Add “ChargeBee” to your vocabulary?');
    assert.equal(toasts[0].options.action.label, 'Add');
    assert.deepEqual(written, [], 'offering never writes');
    toasts[0].options.action.onClick(); await flush();
    assert.deepEqual(written, [['Rismondo', 'ChargeBee']]);
    const added = toasts.at(-1);
    assert.equal(added.kind, 'success');
    assert.equal(added.options.action.label, 'Undo');
    added.options.action.onClick(); await flush();
    assert.deepEqual(stored, ['Rismondo']);
  } finally { globalThis.window = previousWindow; }
});
