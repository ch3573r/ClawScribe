import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const language = loadTsModule('src/lib/transcription-language.ts');

test('each engine maps the way the core chooses its live engine', () => {
  assert.equal(language.languageSupport('localWhisper'), 'whisper');
  assert.equal(language.languageSupport('nemotron'), 'nemotron');
  assert.equal(language.languageSupport('parakeet'), 'detects');
  // No saved engine config at all: the core falls back to Parakeet.
  for (const none of [undefined, null]) assert.equal(language.languageSupport(none), 'detects', String(none));
  // A saved config with a blank provider is not "none": the core's `localWhisper | _` branch runs Whisper.
  assert.equal(language.languageSupport(''), 'whisper');
  // Only the two hosted engines are batch only (is_cloud_provider).
  for (const hosted of ['cloud-whisper', 'mai-transcribe']) assert.equal(language.languageSupport(hosted), 'hosted', hosted);
  // The old ids are not hosted engines; the core runs them through Whisper.
  for (const legacy of ['deepgram', 'elevenLabs', 'groq', 'openai']) assert.equal(language.languageSupport(legacy), 'whisper', legacy);
});

test('Whisper offers Auto detect and translation', () => {
  const whisper = language.languageOptions('whisper').map(option => option.code);
  assert.ok(whisper.includes('auto') && whisper.includes('auto-translate') && whisper.includes('de'));
});

test('Nemotron offers the system language and concrete languages, never translation', () => {
  const nemotron = language.languageOptions('nemotron');
  const codes = nemotron.map(option => option.code);
  assert.ok(codes.includes('auto') && codes.includes('de'));
  assert.ok(!codes.includes('auto-translate'));
  assert.equal(nemotron.find(option => option.code === 'auto').label, 'System language');
});

test('engines that detect the language or run in the cloud show no control', () => {
  assert.equal(language.languageOptions('detects').length, 0);
  assert.equal(language.languageOptions('hosted').length, 0);
});

test('labels are sentence case, including a stored value no control offers', () => {
  assert.equal(language.languageLabel('auto', 'Auto Detect (Original Language)'), 'Auto detect');
  assert.equal(language.languageLabel('auto-translate', 'x'), 'Auto detect, translate to English');
  assert.equal(language.languageLabel('de', 'German'), 'German');
  assert.equal(language.storedLanguageLabel('de'), 'German');
  assert.equal(language.storedLanguageLabel('auto-translate'), 'Auto detect, translate to English');
  assert.equal(language.storedLanguageLabel('xx'), 'xx');
});
