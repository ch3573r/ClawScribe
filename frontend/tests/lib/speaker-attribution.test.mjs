import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';

const modulePath = fileURLToPath(new URL('../../src/lib/speaker-attribution.ts', import.meta.url));

test('saved labels enable attribution with either Beta setting and without audio', () => {
  const { shouldShowSpeakerAttribution } = loadTsModule(modulePath);
  for (const enabled of [false, true]) for (const folder of [false, true]) {
    assert.equal(shouldShowSpeakerAttribution(enabled, folder, [{ speaker: ' Speaker 1 ' }]), true);
  }
});

test('unlabelled meetings keep the existing toggle and recording-folder behavior', () => {
  const { shouldShowSpeakerAttribution } = loadTsModule(modulePath);
  const rows = [{}, { speaker: null }, { speaker: '' }, { speaker: ' \n ' }];
  assert.equal(shouldShowSpeakerAttribution(false, true, rows), false);
  assert.equal(shouldShowSpeakerAttribution(true, false, rows), false);
  assert.equal(shouldShowSpeakerAttribution(true, true, rows), true);
  assert.equal(shouldShowSpeakerAttribution(false, true), false);
});

test('labels beyond the loaded page and newly detected labels enable attribution', () => {
  const { shouldShowSpeakerAttribution } = loadTsModule(modulePath);
  const loaded = [{ speaker: undefined }];
  assert.equal(shouldShowSpeakerAttribution(false, true, undefined, loaded), false);
  const timeline = [...loaded, { speaker: 'Speaker 2' }];
  assert.equal(shouldShowSpeakerAttribution(false, true, timeline, loaded), true);
  assert.equal(shouldShowSpeakerAttribution(false, true, [{ speaker: 'Me' }]), true);
});

test('detection needs a recording folder and transcript rows, independently of labels', () => {
  const { canDetectSpeakers } = loadTsModule(modulePath);
  assert.equal(canDetectSpeakers(true, 1), true);
  assert.equal(canDetectSpeakers(true, 0), false);
  assert.equal(canDetectSpeakers(false, 1), false);
  assert.equal(canDetectSpeakers(false, 0), false);
});
