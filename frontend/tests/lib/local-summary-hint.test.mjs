import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';

const modulePath = fileURLToPath(new URL('../../src/lib/meeting-summary.ts', import.meta.url));
const saved = (provider, extra = {}) => ({
  markdown: 'Notes',
  english_cache: { markdown: 'Notes', source: { model_provider: provider, model_name: 'synthetic-model', template_id: 'standard_meeting' } },
  ...extra,
});

test('local providers show the hint', () => {
  const { summaryDetails, LOCAL_SUMMARY_HINT } = loadTsModule(modulePath);
  for (const provider of ['builtin-ai', 'local-llama', 'ollama', ' Ollama ']) {
    assert.equal(summaryDetails(saved(provider)).localProvider, true, provider);
    assert.equal(summaryDetails(JSON.stringify(saved(provider))).localProvider, true, provider);
  }
  assert.equal(LOCAL_SUMMARY_HINT, 'Generated on this PC. Check tasks, owners and dates against the transcript.');
});

test('external providers do not show the hint', () => {
  const { summaryDetails } = loadTsModule(modulePath);
  for (const provider of ['openai', 'claude', 'claude-code', 'groq', 'openrouter', 'openai-compatible', 'custom-openai', 'openclaw', 'codex']) {
    assert.equal(summaryDetails(saved(provider)).localProvider, false, provider);
  }
});

test('missing or unusable metadata does not show the hint', () => {
  const { summaryDetails } = loadTsModule(modulePath);
  for (const input of [null, undefined, '', 'not json', {}, { markdown: 'Notes' }, { markdown: 'Notes', generation: { model: 'gemma' } },
    saved(undefined), saved(''), saved(42), { english_cache: { source: null } }]) {
    assert.equal(summaryDetails(input).localProvider, false);
  }
});
