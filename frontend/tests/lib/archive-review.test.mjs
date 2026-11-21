import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

const { archiveReview, archiveRecordingFacts, visibleSelection } = loadTsModule('src/lib/archive-review.ts');

const meeting = {
  id: 'meeting',
  has_folder: true,
  has_transcript: true,
  has_summary: false,
  summary_status: null,
  audio_save_failed: false,
  transcription_incomplete: false,
  capture_incomplete: false,
  recording_files_incomplete: false,
  recovery_files_elsewhere: false,
  pending_file_sync: false,
};

test('review routes local recovery to transcription and preserves the remote recovery instruction', () => {
  assert.equal(archiveReview({ ...meeting, transcription_incomplete: true }).action, 'transcribe');
  assert.equal(archiveReview({ ...meeting, recovery_files_elsewhere: true, transcription_incomplete: true }).action, 'review');
  assert.equal(archiveReview({ ...meeting, capture_incomplete: true }).action, 'review');
});

test('archive date keys follow the displayed local day for instants and legacy dates', () => {
  const timeZone = process.env.TZ;
  try {
    for (const [zone, early, late] of [
      ['Europe/Berlin', '2026-11-21', '2026-11-22'],
      ['America/Los_Angeles', '2026-11-20', '2026-11-21'],
    ]) {
      process.env.TZ = zone;
      const { archiveDateKey, formatArchiveDate } = loadTsModule('src/lib/archive-review.ts');
      assert.equal(archiveDateKey('2026-11-21T00:30:00Z'), early);
      assert.equal(archiveDateKey('2026-11-21T23:30:00+00:00'), late);
      assert.equal(archiveDateKey('2026-11-21 23:30:00.125'), late);
      assert.equal(archiveDateKey('2026-11-21'), '2026-11-21');
      assert.equal(archiveDateKey('2026-02-30'), null);
      assert.match(formatArchiveDate('2026-11-21'), /21/);
      assert.equal(formatArchiveDate('2026-11-21 23:30:00'), formatArchiveDate('2026-11-21T23:30:00Z'));
    }
  } finally {
    if (timeZone === undefined) delete process.env.TZ;
    else process.env.TZ = timeZone;
  }
});

test('transcript-only meetings can generate a summary without entering the review queue', () => {
  const review = archiveReview({ ...meeting, has_folder: false });
  assert.equal(review.needsAttention, false);
  assert.equal(review.action, 'summary');
});

test('pending file publication routes to the transcript and keeps other recording warnings visible', () => {
  const pending = { ...meeting, pending_file_sync: true, recording_files_incomplete: true, capture_incomplete: true };
  const review = archiveReview(pending);
  assert.equal(review.needsAttention, true);
  assert.equal(review.action, 'files');
  assert.deepEqual(Array.from(review.warnings), ['Audio was lost during capture', 'Recording-folder copy needs updating']);
  assert.equal(archiveReview({ ...pending, recovery_files_elsewhere: true }).action, 'review');
  assert.equal(archiveReview({ ...pending, audio_save_failed: true }).action, 'transcribe');
  assert.equal(archiveReview({ ...pending, has_folder: false }).action, 'review');
});

test('failed summary can be retried even when its earlier result was preserved', () => {
  const review = archiveReview({ ...meeting, summary_status: 'failed', has_summary: true });
  assert.equal(review.needsAttention, true);
  assert.equal(review.action, 'summary');
  assert.equal(review.summary, 'Summary needs retry');
});

test('audio-only meetings need transcription while an active summary is only a status', () => {
  assert.equal(archiveReview({ ...meeting, has_transcript: false }).action, 'transcribe');
  const review = archiveReview({ ...meeting, summary_status: 'processing' });
  assert.equal(review.needsAttention, false);
  assert.equal(review.action, 'review');
});

test('archive shows measured audio-only duration and omits unknown legacy facts', () => {
  assert.deepEqual(Array.from(archiveRecordingFacts({
    recording_source: 'recording', duration_seconds: 93,
  })), ['Recorded', '1m 33s']);
  assert.deepEqual(Array.from(archiveRecordingFacts({
    recording_source: 'import', duration_seconds: null,
  })), ['Imported audio']);
  assert.deepEqual(Array.from(archiveRecordingFacts({
    recording_source: null, duration_seconds: null,
  })), []);
});

test('recorded and imported audio source labels are distinct', () => {
  const recorded = archiveRecordingFacts({ recording_source: 'recording', duration_seconds: null });
  const imported = archiveRecordingFacts({ recording_source: 'import', duration_seconds: null });
  assert.notEqual(recorded[0], imported[0]);
  assert.equal(recorded[0], 'Recorded');
  assert.equal(imported[0], 'Imported audio');
});

test('filtered-out selections cannot enter a bulk operation', () => {
  assert.deepEqual([...visibleSelection(new Set(['b', 'hidden', 'a']), ['a', 'b', 'c'])], ['a', 'b']);
});

test('audio review sends only eligible revisions after explicit confirmation', async () => {
  const harness = createHookHarness();
  const calls = [];
  const removed = [];
  const jsx = (type, props) => ({ type, props });
  const { ArchiveAudioRemoval } = loadTsModule('src/components/Archive/ArchiveAudioRemoval.tsx', {
    react: harness.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/core': { invoke: async (command, payload) => {
      calls.push([command, JSON.parse(JSON.stringify(payload))]);
      if (command === 'inspect_archive_audio') {
        return [
          { id: 'safe', bytes: 1024, revision: 'reviewed', reason: null },
          { id: 'recovery', bytes: null, revision: null, reason: 'Recovery files are present.' },
        ];
      }
      return [{ id: 'safe', removed_bytes: 1024, reason: null }];
    } },
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/dialog': Object.fromEntries([
      'Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle',
    ].map(name => [name, name])),
  });
  const meetings = [{ id: 'safe', title: 'Synthetic one' }, { id: 'recovery', title: 'Synthetic two' }];
  const render = () => harness.render(() => ArchiveAudioRemoval({ meetings, onClose() {}, onRemoved: ids => removed.push(...ids) }));
  const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node && typeof node === 'object' ? [node, ...nodes(node.props?.children)] : [];
  render();
  await flush();
  assert.deepEqual(calls, [['inspect_archive_audio', { ids: ['safe', 'recovery'] }]]);
  const confirm = nodes(render()).find(node => node.type === 'button' && node.props.variant === 'destructive');
  assert.ok(confirm);
  confirm.props.onClick();
  await flush();
  assert.deepEqual(calls[1], ['remove_archive_audio', { requests: [{ id: 'safe', revision: 'reviewed' }] }]);
  assert.deepEqual(removed, ['safe']);
  harness.unmount();
});
