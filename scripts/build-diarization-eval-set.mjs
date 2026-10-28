#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const RATE = 16000;
const CONVERT = 'Convert with: ffmpeg -i in.mp3 -ac 1 -ar 16000 -sample_fmt s16 out.wav';
const defaults = { count: 5, speakers: 4, minutes: 5, seed: 1, overlap: 0.05,
  language: 'und', gapMin: 0.2, gapMax: 1.5, turnMin: 2, turnMax: 12 };

function validate(options) {
  for (const key of ['count', 'speakers', 'minutes', 'seed', 'overlap', 'gapMin', 'gapMax', 'turnMin', 'turnMax']) {
    if (!Number.isFinite(options[key])) throw new Error(`${key} must be finite`);
  }
  for (const key of ['count', 'speakers', 'seed']) {
    if (!Number.isSafeInteger(options[key])) throw new Error(`${key} must be an integer`);
  }
  if (options.count < 1 || options.speakers < 1 || options.speakers > 12) throw new Error('count must be positive; speakers must be 1..12');
  if (options.minutes <= 0 || options.overlap < 0 || options.overlap > 1) throw new Error('minutes must be positive; overlap must be 0..1');
  if (options.gapMin < 0 || options.gapMax < options.gapMin || options.turnMin <= 0 || options.turnMax < options.turnMin) {
    throw new Error('Require 0 <= gap-min <= gap-max and 0 < turn-min <= turn-max');
  }
  for (const [min, max] of [[options.gapMin, options.gapMax], [options.turnMin, options.turnMax]]) {
    if (Math.ceil(min * 1000) > Math.floor(max * 1000)) throw new Error('Timing bounds must contain a whole millisecond');
  }
  if (Math.round(options.minutes * 60000) < 1 || options.turnMin < 0.001) throw new Error('Duration and turns must be at least one millisecond');
  if (!/^[A-Za-z]{2,8}(?:-[A-Za-z0-9]{1,8})*$/.test(options.language)) throw new Error('language must be a language tag');
  // RIFF uses a 32-bit byte length. Reject before allocating any audio.
  if (Math.round(options.minutes * 60000) * 16 * 2 > 0xffffffff - 36) throw new Error('Requested duration exceeds PCM WAV size limit');
}

/** Decode a WAV buffer, accepting only 16 kHz mono little-endian PCM16. */
export function readWav(buffer) {
  const fail = () => { throw new Error(`Expected a complete 16 kHz mono PCM16 WAV. ${CONVERT}`); };
  if (!Buffer.isBuffer(buffer) || buffer.length < 12 || buffer.toString('ascii', 0, 4) !== 'RIFF'
      || buffer.toString('ascii', 8, 12) !== 'WAVE') fail();
  const end = buffer.readUInt32LE(4) + 8;
  if (end > buffer.length || end < 12) fail();
  let format = false;
  let data;
  let offset = 12;
  while (offset < end) {
    if (offset + 8 > end) fail();
    const id = buffer.toString('ascii', offset, offset + 4);
    const size = buffer.readUInt32LE(offset + 4);
    const start = offset + 8;
    if (start + size + (size % 2) > end) fail();
    if (id === 'fmt ') {
      if (format || size < 16 || buffer.readUInt16LE(start) !== 1
          || buffer.readUInt16LE(start + 2) !== 1 || buffer.readUInt32LE(start + 4) !== RATE
          || buffer.readUInt32LE(start + 8) !== RATE * 2 || buffer.readUInt16LE(start + 12) !== 2
          || buffer.readUInt16LE(start + 14) !== 16) fail();
      format = true;
    }
    if (id === 'data') {
      if (data || size === 0 || size % 2) fail();
      data = buffer.subarray(start, start + size);
    }
    offset = start + size + (size % 2);
  }
  if (!format || !data) fail();
  return Int16Array.from({ length: data.length / 2 }, (_, i) => data.readInt16LE(i * 2));
}

/** Encode samples as a standard 16 kHz mono PCM16 WAV buffer. */
export function writeWav(samples) {
  if (!(samples instanceof Int16Array) || !samples.length || samples.length * 2 > 0xffffffff - 36) {
    throw new Error('WAV samples must be a non-empty PCM16 array within the RIFF size limit');
  }
  const buffer = Buffer.alloc(44 + samples.length * 2);
  buffer.write('RIFF', 0);
  buffer.writeUInt32LE(buffer.length - 8, 4);
  buffer.write('WAVEfmt ', 8);
  buffer.writeUInt32LE(16, 16);
  buffer.writeUInt16LE(1, 20);
  buffer.writeUInt16LE(1, 22);
  buffer.writeUInt32LE(RATE, 24);
  buffer.writeUInt32LE(RATE * 2, 28);
  buffer.writeUInt16LE(2, 32);
  buffer.writeUInt16LE(16, 34);
  buffer.write('data', 36);
  buffer.writeUInt32LE(samples.length * 2, 40);
  samples.forEach((sample, i) => buffer.writeInt16LE(sample, 44 + i * 2));
  return buffer;
}

function randomGenerator(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let value = Math.imul(state ^ (state >>> 15), state | 1);
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}

/** Pure builder: speakers = [{ name, clips: Int16Array[] }]; returns PCM and sample-indexed turns. */
export function buildMixture(speakers, input = {}) {
  const options = { ...defaults, ...input };
  validate(options);
  if (speakers.length < options.speakers) throw new Error('Not enough distinct speaker directories');
  if (new Set(speakers.map(s => s.name)).size !== speakers.length || speakers.some(s => !s.name || /\s/.test(s.name))) {
    throw new Error('Speaker directory names must be distinct, non-empty RTTM tokens without whitespace');
  }
  const random = randomGenerator(options.seed);
  const integer = (min, max) => min + Math.floor(random() * (max - min + 1));
  const shuffled = speakers.slice();
  for (let i = shuffled.length - 1; i > 0; i--) {
    const j = integer(0, i);
    [shuffled[i], shuffled[j]] = [shuffled[j], shuffled[i]];
  }
  const selected = shuffled.slice(0, options.speakers);
  const minMs = Math.ceil(options.turnMin * 1000);
  const maxMs = Math.floor(options.turnMax * 1000);
  for (const speaker of selected) {
    if (!speaker.clips?.length || speaker.clips.some(clip => !(clip instanceof Int16Array) || clip.length < minMs * 16)) {
      throw new Error('Every selected speaker clip must contain at least turn-min seconds of PCM16 audio');
    }
  }
  const length = Math.round(options.minutes * 60000) * 16;
  const segments = [];
  const used = new Set();
  while (!segments.length || segments.at(-1).endSample < length) {
    const previous = segments.at(-1);
    // Introduce every selected speaker before allowing repeats, so manifest counts are exact.
    let pool = selected.filter(s => !used.has(s.name));
    if (!pool.length) pool = selected.filter(s => s.name !== previous?.speaker || selected.length === 1);
    const speaker = pool[integer(0, pool.length - 1)];
    const clipIndex = integer(0, speaker.clips.length - 1);
    const clip = speaker.clips[clipIndex];
    const duration = integer(minMs, Math.min(maxMs, Math.floor(clip.length / 16))) * 16;
    let startSample = 0;
    if (previous) {
      if (selected.length > 1 && duration > 16 && previous.endSample - previous.startSample > 16 && random() < options.overlap) {
        // Short excerpts must still end after the previous turn, so later
        // transitions cannot turn a requested overlap into an accidental gap.
        const overlapMs = integer(1, Math.min(1500, duration / 16 - 1, (previous.endSample - previous.startSample) / 16 - 1));
        // Keep forward progress and at most two concurrent turns, even for very short clips.
        startSample = Math.max(previous.startSample + 16, previous.endSample - overlapMs * 16,
          segments.at(-2)?.endSample ?? 0);
      } else {
        startSample = previous.endSample + integer(Math.ceil(options.gapMin * 1000), Math.floor(options.gapMax * 1000)) * 16;
      }
    }
    if (startSample >= length) break;
    const endSample = Math.min(length, startSample + duration);
    const sourceStartSample = integer(0, clip.length - (endSample - startSample));
    segments.push({ speaker: speaker.name, startSample, endSample, clipIndex, sourceStartSample });
    used.add(speaker.name);
  }
  if (used.size !== options.speakers) throw new Error('Requested duration is too short to include every selected speaker');
  const summed = new Float64Array(length);
  const byName = new Map(selected.map(s => [s.name, s]));
  for (const segment of segments) {
    const clip = byName.get(segment.speaker).clips[segment.clipIndex];
    for (let i = segment.startSample; i < segment.endSample; i++) summed[i] += clip[segment.sourceStartSample + i - segment.startSample];
  }
  let peak = 0;
  for (const sample of summed) peak = Math.max(peak, Math.abs(sample));
  const gain = peak > 32767 ? 32767 / peak : 1;
  return { samples: Int16Array.from(summed, sample => Math.round(sample * gain)), segments };
}

export function formatRttm(name, segments) {
  if (!name || /\s/.test(name)) throw new Error('RTTM file name must be one token');
  return segments.map(segment => {
    if (!segment.speaker || /\s/.test(segment.speaker) || !Number.isInteger(segment.startSample)
        || !Number.isInteger(segment.endSample) || segment.startSample < 0 || segment.endSample <= segment.startSample
        || segment.startSample % 16 || segment.endSample % 16) throw new Error('RTTM requires millisecond-aligned sample positions and speaker tokens');
    return `SPEAKER ${name} 1 ${(segment.startSample / RATE).toFixed(3)} ${((segment.endSample - segment.startSample) / RATE).toFixed(3)} <NA> <NA> ${segment.speaker} <NA> <NA>`;
  }).join('\n') + '\n';
}

function parseArgs(argv) {
  const options = { ...defaults };
  const flags = { clips: 'clips', out: 'out', count: 'count', speakers: 'speakers', minutes: 'minutes', seed: 'seed',
    overlap: 'overlap', language: 'language', 'gap-min': 'gapMin', 'gap-max': 'gapMax', 'turn-min': 'turnMin', 'turn-max': 'turnMax' };
  const seen = new Set();
  for (let i = 0; i < argv.length; i += 2) {
    const flag = argv[i].slice(2);
    const value = argv[i + 1];
    if (!argv[i].startsWith('--') || !Object.hasOwn(flags, flag) || value === undefined || !value.trim() || seen.has(flag)) {
      throw new Error('Expected unique --option value pairs; required: --clips <dir> --out <dir>');
    }
    seen.add(flag);
    const key = flags[flag];
    options[key] = ['clips', 'out', 'language'].includes(key) ? value : Number(value);
  }
  if (!options.clips || !options.out) throw new Error('Required: --clips <dir> --out <dir>');
  validate(options);
  return options;
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  const speakers = fs.readdirSync(options.clips, { withFileTypes: true }).filter(entry => entry.isDirectory())
    .sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0).map(entry => {
      const directory = path.join(options.clips, entry.name);
      const files = fs.readdirSync(directory, { withFileTypes: true }).filter(file => file.isFile()).map(file => file.name).sort();
      if (!files.length || files.some(file => !file.toLowerCase().endsWith('.wav'))) throw new Error(`Each speaker directory needs only WAV clips. ${CONVERT}`);
      return { name: entry.name, clips: files.map(file => readWav(fs.readFileSync(path.join(directory, file)))) };
    });
  fs.mkdirSync(options.out, { recursive: true });
  const items = [];
  for (let i = 0; i < options.count; i++) {
    const name = `mix-${String(i + 1).padStart(2, '0')}`;
    const mixture = buildMixture(speakers, { ...options, seed: options.seed + i });
    fs.writeFileSync(path.join(options.out, `${name}.wav`), writeWav(mixture.samples));
    fs.writeFileSync(path.join(options.out, `${name}.rttm`), formatRttm(name, mixture.segments));
    items.push({ name, audio: `${name}.wav`, rttm: `${name}.rttm`, language: options.language, num_speakers: options.speakers });
  }
  fs.writeFileSync(path.join(options.out, 'manifest.json'), JSON.stringify({ items }, null, 2) + '\n');
  console.log(`Built ${items.length} mixtures and manifest.json`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(); } catch (error) {
    console.error(error.code ? 'Cannot read clips or write the evaluation set; check the input and output directories' : error.message);
    process.exitCode = 2;
  }
}
