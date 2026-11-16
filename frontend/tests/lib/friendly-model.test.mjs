import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';

const modulePath = fileURLToPath(new URL('../../src/lib/friendly-model.ts', import.meta.url));

test('model labels name Claude Code aliases and retain known model families', () => {
  const { friendlyModel } = loadTsModule(modulePath);
  for (const [id, expected] of [
    ['sonnet', 'Claude Sonnet'], ['opus', 'Claude Opus'], ['haiku', 'Claude Haiku'],
    [' claude-sonnet-5 ', 'Claude Sonnet'], ['anthropic/claude-opus-5', 'Claude Opus'],
    ['gpt-5.6-sol', 'GPT-5.6'], ['local/qwen3.5:4b', 'Qwen 3.5'],
    [undefined, 'Choose a model'], ['', 'Choose a model'], ['   ', 'Choose a model'],
  ]) assert.equal(friendlyModel(id), expected, String(id));
});

test('unknown model ids stay out of the label', () => {
  const { friendlyModel } = loadTsModule(modulePath);
  assert.equal(friendlyModel(' vendor/custom-model '), 'Selected model');
  assert.equal(friendlyModel(`vendor/${'x'.repeat(60)}`), 'Selected model');
});
