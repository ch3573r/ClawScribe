import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const names = list => Object.fromEntries(list.split(' ').map(name => [name, name]));
const jsx = (type, props) => ({ type, props });
const slots = props => [props?.title, props?.description, props?.children];
const nodes = root => Array.isArray(root) ? root.flatMap(nodes)
  : root && typeof root === 'object' ? [root, ...slots(root.props).flatMap(nodes)] : [];
const text = root => typeof root === 'string' ? root : Array.isArray(root) ? root.map(text).join('')
  : root && typeof root === 'object' ? slots(root.props).map(text).join('') : '';

function card(t, { provider, language, recording = false }) {
  const hooks = createHookHarness();
  const changes = [];
  const toasts = [];
  const { TranscriptionLanguage } = loadTsModule('src/components/settings/TranscriptionLanguage.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx, jsxs: jsx },
    'lucide-react': names('Globe'),
    sonner: { toast: { info: title => toasts.push(title) } },
    '@/components/settings/settings-parts': { ROW_SELECT: 'row-select' },
    '@/components/ui/button': names('Button'),
    '@/components/ui/select': names('Select SelectContent SelectItem SelectTrigger SelectValue'),
    '@/components/ui/settings-layout': names('Meta SettingsCard SettingsRow'),
    '@/contexts/ConfigContext': { useTranscriptionConfig: () => ({
      transcriptModelConfig: { provider }, selectedLanguage: language, setSelectedLanguage: value => changes.push(value),
    }) },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ isRecording: recording, isStarting: false, isStopping: false }) },
  });
  t.after(() => hooks.unmount());
  const render = () => hooks.render(() => TranscriptionLanguage());
  const find = (type, predicate = () => true) => nodes(render()).find(node => node.type === type && predicate(node));
  return { find, render, changes, toasts, text: () => text(render()) };
}

test('Parakeet with an automatic setting shows only that it detects the language', t => {
  const ui = card(t, { provider: 'parakeet', language: 'auto' });
  assert.equal(ui.find('Select'), undefined);
  assert.equal(ui.find('Button'), undefined);
  assert.match(ui.text(), /detects the spoken language itself/);
});

test('a stored language is shown for Parakeet and hosted engines, with a way to reset it', t => {
  for (const provider of ['parakeet', 'cloud-whisper', 'mai-transcribe']) {
    const ui = card(t, { provider, language: 'de' });
    assert.equal(ui.find('Select'), undefined, provider);
    assert.match(ui.text(), /German/);
    assert.match(ui.text(), /still saved with new recordings/);
    const reset = ui.find('Button');
    assert.equal(text(reset), 'Reset to automatic');
    reset.props.onClick();
    assert.deepEqual(ui.changes, ['auto'], 'only on request');
  }
});

test('the stored value is never reset silently', t => {
  const ui = card(t, { provider: 'parakeet', language: 'de' });
  ui.render();
  assert.deepEqual(ui.changes, []);
});

test('the choice is locked while a recording is running', t => {
  const idle = card(t, { provider: 'localWhisper', language: 'de' });
  assert.equal(idle.find('Select').props.disabled, false);
  const busy = card(t, { provider: 'localWhisper', language: 'de', recording: true });
  assert.equal(busy.find('Select').props.disabled, true);
  assert.match(busy.text(), /Not available while a recording is running/);
});

test('Nemotron keeps Auto as the system language and only moves off translation', t => {
  const auto = card(t, { provider: 'nemotron', language: 'auto' });
  auto.render();
  assert.deepEqual(auto.changes, [], 'no switch away from Auto when the page opens');
  assert.deepEqual(auto.toasts, []);
  assert.equal(auto.find('Select').props.value, 'auto');

  const translate = card(t, { provider: 'nemotron', language: 'auto-translate' });
  translate.render();
  assert.deepEqual(translate.changes, ['auto']);
  assert.deepEqual(translate.toasts, ['Nemotron cannot translate']);
});

test('Nemotron does not change the setting during a recording', t => {
  const ui = card(t, { provider: 'nemotron', language: 'auto-translate', recording: true });
  ui.render();
  assert.deepEqual(ui.changes, []);
});
