import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const whisper = loadTsModule('src/lib/whisper.ts', { '@tauri-apps/api/core': { invoke: async () => {} } });

// The model list the core sends comes from this catalogue. Its strings are data
// with their own spelling ("Very Fast"); the screen has to understand them.
const catalog = fs.readFileSync(new URL('../../src-tauri/src/config.rs', import.meta.url), 'utf8');
const entries = [...catalog.matchAll(/\(\s*"([^"]+)",\s*"ggml-[^"]+",\s*\d+,\s*"([^"]+)",\s*"([^"]+)",/g)]
  .map(([, name, accuracy, speed]) => ({ name, accuracy, speed }));

test('the core catalogue was found', () => {
  assert.ok(entries.length >= 8, `found ${entries.length} models`);
  assert.ok(entries.some(entry => entry.speed === 'Very Fast'), 'the core really spells it "Very Fast"');
});

test('every model the core lists gets a speed part in its tagline, without a stray bullet', () => {
  for (const { name, accuracy, speed } of entries) {
    const tagline = whisper.getModelTagline(name, speed, accuracy);
    assert.doesNotMatch(tagline, /^\s*•/, `${name}: ${tagline}`);
    assert.match(tagline, /^(Real time|Fast processing|Moderate speed|Slower processing)( • .+)?$/, `${name}: ${tagline}`);
  }
});

test('tiny and base keep "Real time" or "Fast processing" with the core\'s strings', () => {
  assert.equal(whisper.getModelTagline('tiny', 'Very Fast', 'Decent'), 'Real time • Fastest option');
  assert.equal(whisper.getModelTagline('tiny-q5_1', 'Very Fast', 'Decent'), 'Real time • Fastest option');
  assert.equal(whisper.getModelTagline('base-q5_1', 'Fast', 'Good'), 'Fast processing • Balanced quality');
});

test('the speed is matched without regard to case', () => {
  for (const spelling of ['Very Fast', 'Very fast', 'very fast', ' VERY FAST ']) {
    assert.equal(whisper.getModelTagline('tiny', spelling, 'Decent'), 'Real time • Fastest option', spelling);
  }
  assert.equal(whisper.getModelTagline('medium', 'Slow', 'high'), 'Slower processing • Professional quality');
});

test('an unknown speed leaves no leading bullet', () => {
  assert.equal(whisper.getModelTagline('tiny', 'Warp', 'Decent'), 'Fastest option');
});

test('the speed is shown in sentence case', () => {
  assert.equal(whisper.speedLabel('Very Fast'), 'Very fast');
  assert.equal(whisper.speedLabel('Fast'), 'Fast');
  assert.equal(whisper.speedLabel('Ultra Fast (v3)'), 'Ultra fast (v3)');
  assert.equal(whisper.speedLabel(undefined), '');
});
