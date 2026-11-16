import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { readWav, writeWav, buildMixture, formatRttm } from './build-diarization-eval-set.mjs';

const script = fileURLToPath(new URL('./build-diarization-eval-set.mjs', import.meta.url));
const sine = (frequency, seconds = 3) => Int16Array.from(
  { length: seconds * 16000 }, (_, i) => Math.round(20000 * Math.sin(2 * Math.PI * frequency * i / 16000)),
);
const speakers = ['speaker-a', 'speaker-b', 'speaker-c'].map((name, i) => ({ name, clips: [sine(220 + i * 110)] }));
const options = { speakers: 3, minutes: 0.3, seed: 7, overlap: 0, gapMin: 0.2, gapMax: 0.5, turnMin: 1, turnMax: 2 };
const overlapTime = segments => segments.reduce((sum, a, i) => sum + segments.slice(i + 1)
  .reduce((time, b) => time + Math.max(0, Math.min(a.endSample, b.endSample) - Math.max(a.startSample, b.startSample)), 0), 0);

test('PCM16 WAV round trip and unsupported formats', () => {
  const pcm = sine(440);
  const wav = writeWav(pcm);
  assert.equal(wav.readUInt32LE(24), 16000);
  assert.equal(wav.readUInt16LE(22), 1);
  assert.equal(wav.readUInt16LE(34), 16);
  assert.equal(wav.length, 44 + pcm.length * 2);
  assert.deepEqual(readWav(wav), pcm);
  for (const [offset, value] of [[24, 8000], [22, 2], [20, 3], [34, 8]]) {
    const buffer = writeWav(pcm);
    if (offset === 24) buffer.writeUInt32LE(value, offset);
    else buffer.writeUInt16LE(value, offset);
    assert.throws(() => readWav(buffer), /ffmpeg -i in.mp3 -ac 1 -ar 16000 -sample_fmt s16 out.wav/);
  }
  assert.throws(() => readWav(Buffer.from('invalid')), /WAV/);
  assert.throws(() => readWav(writeWav(pcm).subarray(0, 45)), /WAV/);
});

test('equal seeds reproduce samples and segments; different seeds change them', () => {
  const first = buildMixture(speakers, options);
  assert.deepEqual(buildMixture(speakers, options), first);
  const other = buildMixture(speakers, { ...options, seed: 8 });
  assert.notDeepEqual(other.segments, first.segments);
  assert.notDeepEqual(other.samples, first.samples);
  assert.equal(new Set(first.segments.map(s => s.speaker)).size, 3);
  for (let i = 1; i < first.segments.length; i++) assert.notEqual(first.segments[i].speaker, first.segments[i - 1].speaker);
});

test('RTTM represents placed samples exactly and zero overlap preserves gaps', () => {
  const mixture = buildMixture(speakers, options);
  const lines = formatRttm('mix-01', mixture.segments).trim().split('\n');
  assert.equal(lines.length, mixture.segments.length);
  lines.forEach((line, i) => {
    const fields = line.split(/\s+/);
    const segment = mixture.segments[i];
    assert.equal(Number(fields[3]) * 16000, segment.startSample);
    assert.ok(Math.abs(Number(fields[4]) * 16000 - (segment.endSample - segment.startSample)) < 1e-8);
    assert.equal(fields[7], segment.speaker);
    // With no overlap, samples in each RTTM segment equal the actual clip excerpt.
    const clip = speakers.find(s => s.name === segment.speaker).clips[segment.clipIndex];
    assert.deepEqual(mixture.samples.slice(segment.startSample, segment.endSample),
      clip.slice(segment.sourceStartSample, segment.sourceStartSample + segment.endSample - segment.startSample));
    if (i) assert.ok(segment.startSample >= mixture.segments[i - 1].endSample);
  });
  assert.equal(overlapTime(mixture.segments), 0);
  assert.ok(Math.abs(mixture.samples.length / 16000 - options.minutes * 60) <= options.turnMax);
});

test('requested overlap is present and summed PCM is protected from clipping', () => {
  const mixture = buildMixture(speakers, { ...options, overlap: 1 });
  assert.ok(overlapTime(mixture.segments) > 0);
  assert.ok(mixture.samples.every(sample => sample >= -32768 && sample <= 32767));
  const sums = new Float64Array(mixture.samples.length);
  for (const segment of mixture.segments) {
    const clip = speakers.find(s => s.name === segment.speaker).clips[segment.clipIndex];
    for (let i = segment.startSample; i < segment.endSample; i++) sums[i] += clip[segment.sourceStartSample + i - segment.startSample];
  }
  const gain = Math.min(1, 32767 / sums.reduce((peak, sample) => Math.max(peak, Math.abs(sample)), 0));
  for (let i = 0; i < sums.length; i++) assert.equal(mixture.samples[i], Math.round(sums[i] * gain));
});

test('CLI builds manifest, WAV and RTTM from per-speaker directories', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'diarization-builder-test-'));
  try {
    const clips = path.join(directory, 'clips');
    const out = path.join(directory, 'out');
    for (const speaker of speakers) {
      fs.mkdirSync(path.join(clips, speaker.name), { recursive: true });
      fs.writeFileSync(path.join(clips, speaker.name, 'clip.wav'), writeWav(speaker.clips[0]));
    }
    const args = ['--clips', clips, '--out', out, '--count', '2', '--speakers', '3', '--minutes', '0.2',
      '--turn-min', '1', '--turn-max', '2', '--language', 'de'];
    const result = spawnSync(process.execPath, [script, ...args], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    const manifest = JSON.parse(fs.readFileSync(path.join(out, 'manifest.json'), 'utf8'));
    assert.equal(manifest.items.length, 2);
    for (const item of manifest.items) {
      assert.equal(item.language, 'de');
      assert.equal(item.num_speakers, 3);
      assert.ok(readWav(fs.readFileSync(path.join(out, item.audio))).length > 0);
      assert.match(fs.readFileSync(path.join(out, item.rttm), 'utf8'), /^SPEAKER mix-0[12] 1 /);
    }
    const original = fs.readFileSync(path.join(out, 'mix-01.wav'));
    assert.equal(spawnSync(process.execPath, [script, ...args]).status, 0);
    assert.deepEqual(fs.readFileSync(path.join(out, 'mix-01.wav')), original);
  } finally { fs.rmSync(directory, { recursive: true, force: true }); }
});

test('bad CLI arguments exit 2', () => {
  for (const args of [[], ['--unknown', '1'], ['--clips'], ['--count', '1', '--count', '2']]) {
    const result = spawnSync(process.execPath, [script, ...args], { encoding: 'utf8' });
    assert.equal(result.status, 2);
    assert.match(result.stderr, /Required:|Expected unique/);
  }
  for (const [args, message] of [
    [['--count', '0'], /count must be positive/],
    [['--seed', '1.2'], /seed must be an integer/],
    [['--overlap', '1.1'], /overlap must be 0\.\.1/],
    [['--minutes', 'NaN'], /minutes must be finite/],
    [['--gap-min', '-1'], /Require 0 <= gap-min/],
    [['--turn-min', '0'], /0 < turn-min/],
    [['--speakers', '13'], /speakers must be 1\.\.12/],
    [['--language', ''], /Expected unique/],
  ]) {
    const result = spawnSync(process.execPath, [script, '--clips', '<clips-directory>', '--out', '<output-directory>', ...args], { encoding: 'utf8' });
    assert.equal(result.status, 2);
    assert.match(result.stderr, message);
  }
});

test('one speaker can take consecutive turns without overlapping itself', () => {
  const mixture = buildMixture(speakers, { ...options, speakers: 1, overlap: 1 });
  assert.equal(new Set(mixture.segments.map(s => s.speaker)).size, 1);
  assert.ok(mixture.segments.length > 1);
  assert.equal(overlapTime(mixture.segments), 0);
});

test('overlap probability one overlaps each transition even with short variable turns', () => {
  const mixture = buildMixture(speakers, { ...options, overlap: 1, turnMin: 0.1 });
  for (let i = 1; i < mixture.segments.length; i++) {
    const previous = mixture.segments[i - 1];
    const current = mixture.segments[i];
    assert.ok(current.startSample < previous.endSample, 'each selected overlap must precede the previous turn end');
    assert.ok(previous.endSample - current.startSample <= 1.5 * 16000);
  }
});

test('rejects insufficient speakers, short clips and unrepresentable timing', () => {
  assert.throws(() => buildMixture(speakers, { ...options, speakers: 4 }), /Not enough/);
  assert.throws(() => buildMixture(speakers.map(s => ({ ...s, clips: [sine(440, 0.5)] })), options), /turn-min/);
  assert.throws(() => buildMixture(speakers, { ...options, gapMin: 0.0001, gapMax: 0.0002 }), /whole millisecond/);
  assert.throws(() => buildMixture(speakers, { ...options, minutes: 0.00002 }), /too short/);
  assert.throws(() => formatRttm('mix', [{ speaker: 'a', startSample: 1, endSample: 17 }]), /millisecond-aligned/);
});
