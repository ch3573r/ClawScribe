import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';

const { EmptyStateSummary } = loadTsModule(fileURLToPath(new URL('../../src/components/EmptyStateSummary.tsx', import.meta.url)), {
  'framer-motion': { motion: { div: 'div' } },
  'lucide-react': { FileQuestion: 'FileQuestion', Sparkles: 'Sparkles' },
  '@/components/ui/button': { Button: 'button' },
  '@/components/ui/tooltip': Object.fromEntries(['Tooltip', 'TooltipContent', 'TooltipProvider', 'TooltipTrigger'].map(name => [name, name])),
});
function nodes(node) {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (!node || typeof node !== 'object') return [];
  return [node, ...nodes(node.props?.children)];
}
function text(node) {
  if (Array.isArray(node)) return node.map(text).join('');
  if (typeof node === 'string') return node;
  return node ? text(node.props?.children) : '';
}

for (const hasModel of [false, true]) {
  test(`no transcript hides Generate and model hints with hasModel=${hasModel}`, () => {
    const tree = EmptyStateSummary({ hasTranscript: false, hasModel, onGenerate() {} });
    assert.match(text(tree), /No transcript yet/);
    assert.match(text(tree), /Transcribe the recording first, then generate a summary\./);
    assert.doesNotMatch(text(tree), /Generate Summary|select a model/);
    assert.equal(nodes(tree).some(node => node.type === 'button'), false);
  });
}

test('a transcript with a model retains the working Generate action', () => {
  let generated = 0;
  const tree = EmptyStateSummary({ hasTranscript: true, hasModel: true, onGenerate: () => { generated++; } });
  assert.match(text(tree), /No Summary Generated Yet/);
  const button = nodes(tree).find(node => node.type === 'button');
  assert.equal(text(button), 'Generate Summary');
  assert.equal(button.props.disabled, false);
  button.props.onClick();
  assert.equal(generated, 1);
});

test('an existing transcript keeps the model hint and disabled Generate action without a model', () => {
  const tree = EmptyStateSummary({ hasTranscript: true, hasModel: false, onGenerate() {} });
  assert.match(text(tree), /Please select a model in Settings first/);
  assert.equal(nodes(tree).find(node => node.type === 'button').props.disabled, true);
});

test('generation in flight stays disabled', () => {
  const tree = EmptyStateSummary({ hasTranscript: true, hasModel: true, isGenerating: true, onGenerate() {} });
  const button = nodes(tree).find(node => node.type === 'button');
  assert.equal(text(button), 'Generating...');
  assert.equal(button.props.disabled, true);
});
