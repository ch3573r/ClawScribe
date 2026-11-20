import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

const jsx = (type, props) => ({ type, props: props ?? {} });
const expand = node => Array.isArray(node) ? node.flatMap(expand) : !node || typeof node !== 'object' ? [] : [node, ...expand(node.props?.children)];
const names = value => Object.fromEntries(value.split(' ').map(name => [name, name]));
const text = node => typeof node === 'string' ? node : Array.isArray(node) ? node.map(text).join('') : node?.props ? text(node.props.children) : '';

function setup({ learning }) {
  const hooks = createHookHarness();
  const invoked = [], offers = [];
  const vocabulary = loadTsModule('src/lib/vocabulary-learning.ts', { '@tauri-apps/api/core': { invoke: async () => [] } });
  const { TranscriptCorrections } = loadTsModule('src/components/MeetingDetails/TranscriptCorrections.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      invoked.push([command, args]);
      return command === 'api_get_transcript_edit_state' ? { can_undo: false, pending_file_sync: false, has_edits: false, can_restore_previous: false } : { changed: 1, file_warning: false };
    } },
    '@tauri-apps/api/event': { listen: async () => () => {} },
    sonner: { toast: { success() {}, error() {} } },
    '@/components/ui/button': { Button: 'button' }, '@/components/ui/textarea': { Textarea: 'textarea' },
    '@/components/ui/accordion': names('Accordion AccordionContent AccordionItem AccordionTrigger'),
    '@/components/ui/dialog': names('Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
    './TextReplaceDialog': { TextReplaceDialog: 'TextReplaceDialog' },
    '@/lib/vocabulary-learning': { ...vocabulary, readLearnVocabulary: () => learning },
    '@/lib/vocabulary-offer': { offerVocabularyWords: words => { offers.push(Array.from(words)); } },
  });
  const editing = { id: 'row-1', text: 'We use charge be for billing.' };
  const render = () => expand(hooks.render(() => TranscriptCorrections({ meetingId: 'meeting-1', editing, onClose() {}, replaceOpen: false, onReplaceOpenChange() {} })));
  return { render, invoked, offers, hooks };
}

test('a saved correction offers the names it introduced, only when learning is on', async () => {
  for (const learning of [true, false]) {
    const view = setup({ learning });
    view.render(); await flush();
    const nodes = view.render();
    nodes.find(node => node.type === 'textarea').props.onChange({ target: { value: 'We use ChargeBee for billing.' } });
    const save = view.render().find(node => node.type === 'button' && text(node) === 'Save correction');
    save.props.onClick(); await flush();
    assert.ok(view.invoked.some(([command]) => command === 'api_update_transcript_text'));
    assert.deepEqual(view.offers, learning ? [['ChargeBee']] : []);
    view.hooks.unmount();
  }
});

test('a failed save offers nothing', async () => {
  const hooks = createHookHarness();
  const offers = [];
  const { TranscriptCorrections } = loadTsModule('src/components/MeetingDetails/TranscriptCorrections.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/core': { invoke: async command => { if (command === 'api_update_transcript_text') throw 'The transcript changed.'; return { can_undo: false, pending_file_sync: false, has_edits: false, can_restore_previous: false }; } },
    '@tauri-apps/api/event': { listen: async () => () => {} },
    sonner: { toast: { success() {}, error() {} } },
    '@/components/ui/button': { Button: 'button' }, '@/components/ui/textarea': { Textarea: 'textarea' },
    '@/components/ui/accordion': names('Accordion AccordionContent AccordionItem AccordionTrigger'),
    '@/components/ui/dialog': names('Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
    './TextReplaceDialog': { TextReplaceDialog: 'TextReplaceDialog' },
    '@/lib/vocabulary-learning': { readLearnVocabulary: () => true, replacementSuggestion: () => ['ChargeBee'], vocabularySuggestions: () => ['ChargeBee'] },
    '@/lib/vocabulary-offer': { offerVocabularyWords: words => { offers.push(words); } },
  });
  const editing = { id: 'row-1', text: 'charge be' };
  const render = () => expand(hooks.render(() => TranscriptCorrections({ meetingId: 'meeting-1', editing, onClose() {}, replaceOpen: false, onReplaceOpenChange() {} })));
  render(); await flush();
  render().find(node => node.type === 'textarea').props.onChange({ target: { value: 'ChargeBee' } });
  render().find(node => node.type === 'button' && text(node) === 'Save correction').props.onClick(); await flush();
  assert.deepEqual(offers, []);
  hooks.unmount();
});

test('find and replace offers its replacement when it looks like a name or term', async () => {
  for (const [replacement, learning, expected] of [['ChargeBee', true, [['ChargeBee']]], ['billing', true, []], ['ChargeBee', false, []]]) {
    const view = setup({ learning });
    view.render(); await flush();
    const dialog = view.render().find(node => node.type === 'TextReplaceDialog');
    await dialog.props.apply({ query: 'charge be', replacement, matchCase: false }, 'token');
    await flush();
    assert.ok(view.invoked.some(([command]) => command === 'api_replace_transcript_text'));
    assert.deepEqual(view.offers.filter(words => words.length), expected, `${replacement} learning=${learning}`);
    view.hooks.unmount();
  }
});
