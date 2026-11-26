import assert from 'node:assert/strict';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';

const load = () => loadTsModule(fileURLToPath(new URL('../../src/lib/speaker-labels.ts', import.meta.url)));

test('new speaker names collapse whitespace and reject empty or oversized names', () => {
  const { normalizeSpeakerName } = load();
  assert.equal(normalizeSpeakerName('  Ada \n Lovelace  '), 'Ada Lovelace');
  assert.equal(normalizeSpeakerName(' \t '), null);
  assert.equal(normalizeSpeakerName('a'.repeat(64)), 'a'.repeat(64));
  assert.equal(normalizeSpeakerName('a'.repeat(65)), null);
});

test('catalog orders speakers by time, counts all loaded rows and keeps unlabeled rows neutral', () => {
  const { buildSpeakerCatalog, getSpeakerColor } = load();
  const catalog = buildSpeakerCatalog([
    { timestamp: 30, speaker: 'Ada' }, { timestamp: 20, speaker: ' Ben ' },
    { timestamp: 0 }, { timestamp: 10, speaker: 'Ada' },
  ]);
  assert.deepEqual(Array.from(catalog, entry => [entry.name, entry.count]), [[null, 1], ['Ada', 2], ['Ben', 1]]);
  assert.equal(getSpeakerColor(catalog, ' Ada '), 'hsl(var(--primary))');
  assert.equal(getSpeakerColor(catalog, 'Ben'), 'hsl(var(--chart-2))');
  assert.equal(getSpeakerColor(catalog, null), 'hsl(var(--muted-foreground))');
});

test('speakers beyond the six lane colours use a neutral dot', () => {
  const { buildSpeakerCatalog, getSpeakerColor } = load();
  const catalog = buildSpeakerCatalog(Array.from({ length: 8 }, (_, timestamp) => ({ timestamp, speaker: `Person ${timestamp}` })));
  assert.equal(getSpeakerColor(catalog, 'Person 6'), 'hsl(var(--muted-foreground))');
});

test('source presets are offered only for source-labelled meetings or enabled Beta attribution', () => {
  const { buildSpeakerCatalog, getSpeakerOptions } = load();
  const detected = buildSpeakerCatalog([{ timestamp: 0, speaker: 'Speaker 1' }]);
  assert.deepEqual(Array.from(getSpeakerOptions(detected, false)), ['Speaker 1']);
  assert.deepEqual(Array.from(getSpeakerOptions(detected, true)), ['Speaker 1', 'Me', 'Participants']);
  assert.deepEqual(Array.from(getSpeakerOptions(buildSpeakerCatalog([{ speaker: 'Me', timestamp: 0 }]), false)), ['Me', 'Participants']);
});

test('confirmed attendance supplies name suggestions without email addresses or duplicate speakers', () => {
  const { buildSpeakerCatalog, getConfirmedAttendeeNames, getSpeakerOptions, resolveSpeakerName } = load();
  const confirmed = getConfirmedAttendeeNames([
    { attendance: 'present', name: '  Ada   Lovelace  ' },
    { attendance: 'present', name: 'ada lovelace' },
    { attendance: 'present', name: 'Grace Hopper' },
    { attendance: 'present', name: null, email: 'test@example.com' },
    { attendance: 'absent', name: 'Absent attendee' },
    { attendance: 'unreviewed', name: 'Invited attendee' },
  ]);
  assert.deepEqual(Array.from(confirmed), ['Ada Lovelace', 'Grace Hopper']);
  const catalog = buildSpeakerCatalog([{ speaker: 'ADA Lovelace' }]);
  assert.deepEqual(Array.from(getSpeakerOptions(catalog, false, confirmed)), ['ADA Lovelace', 'Grace Hopper']);
  assert.equal(resolveSpeakerName(catalog, 'grace hopper', false, confirmed), 'Grace Hopper');
});

test('rename detects merging into another speaker and reports the full matching count', () => {
  const { buildSpeakerCatalog, getSpeakerRename } = load();
  const catalog = buildSpeakerCatalog([{ speaker: 'A', timestamp: 0 }, { speaker: 'B', timestamp: 1 }, { speaker: 'A', timestamp: 2 }]);
  assert.equal(getSpeakerRename(catalog, 'A', ' B ').merge, 'B');
  assert.equal(getSpeakerRename(catalog, 'A', 'New person').count, 2);
  assert.equal(getSpeakerRename(catalog, 'A', ' A ').merge, null);
});

test('names match existing speakers regardless of case', () => {
  const { buildSpeakerCatalog, getSpeakerRename, resolveSpeakerName } = load();
  const catalog = buildSpeakerCatalog([{ speaker: 'Alice', timestamp: 0 }, { speaker: 'Speaker 2', timestamp: 1 }]);
  const merge = getSpeakerRename(catalog, 'Speaker 2', 'alice');
  assert.equal(merge.merge, 'Alice');
  assert.equal(merge.name, 'Alice');
  assert.equal(getSpeakerRename(catalog, 'Alice', 'ALICE').merge, null);
  assert.equal(getSpeakerRename(catalog, 'Alice', 'ALICE').changed, true);
  assert.equal(getSpeakerRename(catalog, 'Alice', ' Alice ').changed, false);
  assert.equal(resolveSpeakerName(catalog, 'alice'), 'Alice');
  assert.equal(resolveSpeakerName(catalog, 'me', true), 'Me');
  assert.equal(resolveSpeakerName(catalog, 'Bob'), 'Bob');
});

test('unlabelled rows do not take a speaker colour', () => {
  const { buildSpeakerCatalog } = load();
  const withGap = buildSpeakerCatalog([{ timestamp: 0 }, { speaker: 'A', timestamp: 1 }]);
  const without = buildSpeakerCatalog([{ speaker: 'A', timestamp: 1 }]);
  assert.equal(withGap.find(entry => entry.name === 'A').color, without[0].color);
});
