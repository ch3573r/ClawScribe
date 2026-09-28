import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const row = { id: 'first', text: 'Saved words.', timestamp: '09:10:11', speaker: 'Speaker A' };
function contentModule(invoke = async () => { throw new Error('Unexpected transcript read'); }) {
  return loadTsModule('src/lib/meetingExportContent.ts', { '@tauri-apps/api/core': { invoke } });
}

test('summary timestamp stripping removes citations, metadata times and bracketed clocks only', () => {
  const { stripSummaryTimestamps } = contentModule();
  for (const [input, expected] of [
    ['Send draft [00:12:34](#clawscribe-source-abc123).', 'Send draft.'],
    ['Send draft [source](#clawscribe-source-ABC123) tomorrow.', 'Send draft tomorrow.'],
    ['Send draft (owner: Ana; timestamp: 00:12; confidence: high).', 'Send draft (owner: Ana; confidence: high).'],
    ['Send draft (timestamp: 00:12).', 'Send draft.'],
    ['Send draft [12:34] and review [01:12:34].', 'Send draft and review.'],
    ['Agenda: 10 items', 'Agenda: 10 items'],
    ['  - Draft [12:34].\n  - Review tomorrow.', '  - Draft.\n  - Review tomorrow.'],
    ['| Timestamp | Item |\n| --- | --- |\n| 00:12 | Draft |', '| Timestamp | Item |\n| --- | --- |\n| 00:12 | Draft |'],
    ['Ordinary  spacing and [reference](https://example.com) (Agenda: 10 items)', 'Ordinary  spacing and [reference](https://example.com) (Agenda: 10 items)'],
  ]) assert.equal(stripSummaryTimestamps(input), expected);
});

test('summary-only exports honor the timestamp option', async () => {
  const { readExportSummary } = contentModule();
  const markdown = 'Send draft [00:12:34](#clawscribe-source-abc123) (owner: Ana; timestamp: 00:12).';
  assert.equal(await readExportSummary(async () => markdown, { content: 'summary', speakers: false, timestamps: false }), 'Send draft (owner: Ana).');
  assert.equal(await readExportSummary(async () => markdown, { content: 'summary', speakers: false, timestamps: true }), markdown);
});

test('summary-only exports never read or include transcript content', async () => {
  const { prepareMeetingExport, defaultExportOptions } = contentModule();
  const result = await prepareMeetingExport('meeting', async () => 'Edited notes', defaultExportOptions());
  assert.equal(result.summary, 'Edited notes');
  assert.equal(result.markdown, 'Edited notes');
  assert.equal(result.transcript, null);
});

test('transcript-only exports omit notes and use the complete saved snapshot beyond page limits', async () => {
  const rows = Array.from({ length: 1201 }, (_, index) => ({ ...row, id: String(index), text: `Sentence ${index}.` }));
  const calls = [];
  const { prepareMeetingExport, defaultExportOptions } = contentModule(async (command, args) => {
    calls.push({ command, ...args });
    return { transcripts: rows };
  });
  const result = await prepareMeetingExport('meeting', async () => { throw new Error('Summary must not be read'); }, defaultExportOptions('transcript'));
  assert.deepEqual(calls, [{ command: 'api_get_meeting', meetingId: 'meeting' }]);
  assert.equal(result.summary, '');
  assert.match(result.transcript, /Sentence 1200\./);
  assert.equal(result.transcript.split('\n\n').length, 1201);
  assert.ok(result.markdown.startsWith('## Transcript\n\n'));
});

test('speaker and timestamp options are independent and do not alter spoken words', () => {
  const { formatExportTranscript } = contentModule();
  for (const speakers of [false, true]) {
    for (const timestamps of [false, true]) {
      const result = formatExportTranscript([row, { ...row, is_partial: true, text: 'Draft' }, { ...row, text: ' ' }], { content: 'both', speakers, timestamps });
      assert.equal(result, `${timestamps ? '[09:10:11] ' : ''}${speakers ? 'Speaker A: ' : ''}Saved words.`);
    }
  }
});

test('timestamps use audio offsets for live and imported recordings, including zero and hours', () => {
  const { formatExportTranscript, defaultExportOptions } = contentModule();
  for (const timestamp of ['09:10:11', '2026-09-26T08:15:30+00:00']) {
    for (const [audio_start_time, expected] of [[0, '00:00:00'], [65.9, '00:01:05'], [3661.1, '01:01:01'], [360000, '100:00:00']]) {
      assert.equal(formatExportTranscript([{ ...row, timestamp, audio_start_time }], defaultExportOptions('transcript')), `[${expected}] Speaker A: Saved words.`);
    }
  }
});

test('missing or invalid audio offsets retain legacy timestamps and the timestamp switch still applies', () => {
  const { formatExportTranscript, defaultExportOptions } = contentModule();
  for (const audio_start_time of [undefined, null, NaN, Infinity, -1]) {
    assert.equal(formatExportTranscript([{ ...row, audio_start_time }], defaultExportOptions('transcript')), '[09:10:11] Speaker A: Saved words.');
  }
  assert.equal(formatExportTranscript([{ ...row, audio_start_time: 123 }], { content: 'both', timestamps: false, speakers: true }), 'Speaker A: Saved words.');
});

test('both includes edited notes and saved words; failures and empty selections reject', async () => {
  const { prepareMeetingExport, defaultExportOptions } = contentModule(async () => ({ transcripts: [row] }));
  const result = await prepareMeetingExport('meeting', async () => 'Current editor contents', defaultExportOptions('both'));
  assert.match(result.markdown, /Current editor contents\n\n## Transcript/);
  assert.match(result.markdown, /Saved words/);
  await assert.rejects(prepareMeetingExport('meeting', async () => '', defaultExportOptions()), /Generate or save notes/);
  const empty = contentModule(async () => ({ transcripts: [] }));
  await assert.rejects(empty.prepareMeetingExport('meeting', async () => '', defaultExportOptions('transcript')), /no notes or transcripts/);
  const failure = contentModule(async () => { throw new Error('Synthetic read failure'); });
  await assert.rejects(failure.prepareMeetingExport('meeting', async () => 'Notes', defaultExportOptions('both')), /Synthetic read failure/);
});
