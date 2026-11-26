import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

function recoveryHarness({ status = 'success', hasAudio = status !== 'none', transcripts = [], failSave = false, failRead = false } = {}) {
  const hooks = createHookHarness();
  const calls = [];
  const saves = [];
  const metadata = { meetingId: 'interrupted', title: 'Synthetic audio-only meeting', folderPath: 'synthetic-recording', lastUpdated: Date.now() - 60000, transcriptCount: transcripts.length };
  const { useTranscriptRecovery } = loadTsModule('src/hooks/useTranscriptRecovery.ts', {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push(command);
      if (command === 'has_audio_checkpoints') return hasAudio;
      if (command === 'find_interrupted_recordings') return { recordings: [], truncated: false, recordingActive: false };
      if (command === 'recover_audio_from_checkpoints') {
        assert.equal(args.meetingFolder, metadata.folderPath);
        if (status === 'throw') throw new Error('Synthetic audio failure');
        return { status, chunk_count: 1, estimated_duration_seconds: 1,
          audio_file_path: ['success', 'partial'].includes(status) ? 'synthetic-recording/audio-recovered.wav' : undefined };
      }
      if (command === 'cleanup_checkpoints' || command === 'release_recovered_capture') return;
      throw new Error('Unexpected command: ' + command);
    } },
    '@/services/indexedDBService': { indexedDBService: {
      getAllMeetings: async () => [metadata],
      getMeetingMetadata: async () => metadata,
      getTranscripts: async () => { if (failRead) throw new Error('Synthetic transcript read failure'); return transcripts; },
      markMeetingSaved: async () => { calls.push('markSaved'); },
    } },
    '@/services/storageService': { storageService: { saveMeeting: async (...args) => {
      calls.push('save'); saves.push(args);
      if (failSave) throw new Error('Synthetic database failure');
      return { meeting_id: 'saved-audio' };
    } } },
    '@/lib/summary-language-preferences': { applyPinnedSummaryLanguageToMeeting: async () => {} },
    sonner: { toast: { warning: () => {} } },
  });
  const render = () => hooks.render(useTranscriptRecovery);
  return { calls, saves, render, hooks };
}

for (const status of ['success', 'partial']) {
  test(`zero transcripts permits ${status} audio recovery and saves the folder for transcription`, async () => {
    const h = recoveryHarness({ status });
    await h.render().checkForRecoverableTranscripts();
    const result = await h.render().recoverMeeting('interrupted');
    assert.equal(result.success, true);
    assert.equal(result.transcriptCount, 0);
    assert.equal(result.meetingId, 'saved-audio');
    assert.equal(result.remainingCount, 0);
    assert.equal(h.saves.length, 1);
    assert.equal(h.saves[0][1].length, 0);
    assert.equal(h.saves[0][2], 'synthetic-recording');
    assert.equal(h.saves[0][3].capture_incomplete, status === 'partial');
    assert.equal(h.saves[0][3].transcription_incomplete, false);
    assert.ok(h.calls.indexOf('recover_audio_from_checkpoints') < h.calls.indexOf('save'));
    assert.ok(h.calls.indexOf('save') < h.calls.indexOf('markSaved'));
    assert.ok(h.calls.indexOf('save') < h.calls.indexOf('release_recovered_capture'));
    assert.equal(h.calls.includes('cleanup_checkpoints'), status === 'success');
    assert.equal(h.render().recoverableMeetings.length, 0);
    h.hooks.unmount();
  });
}

for (const status of ['none', 'failed', 'throw']) {
  test(`unsuccessful ${status} audio-only recovery keeps the entry and originals for retry`, async () => {
    const h = recoveryHarness({ status });
    await h.render().checkForRecoverableTranscripts();
    await assert.rejects(h.render().recoverMeeting('interrupted'), /No audio could be recovered yet/);
    assert.equal(h.saves.length, 0);
    assert.ok(!h.calls.includes('markSaved'));
    assert.ok(!h.calls.includes('cleanup_checkpoints'));
    assert.ok(!h.calls.includes('release_recovered_capture'));
    assert.equal(h.render().recoverableMeetings.length, 1);
    assert.equal(h.render().isRecovering, false);
    h.hooks.unmount();
  });
}

for (const status of ['success', 'partial', 'none']) {
  test(`recovered live transcript is incomplete when audio recovery is ${status}`, async () => {
    const h = recoveryHarness({ status, transcripts: [{ text: 'Synthetic saved words', timestamp: '00:00', sequenceId: 0 }] });
    await h.render().checkForRecoverableTranscripts();
    const result = await h.render().recoverMeeting('interrupted');
    assert.equal(result.transcriptCount, 1);
    assert.equal(h.saves[0][1][0].text, 'Synthetic saved words');
    assert.equal(h.saves[0][3].audio_save_failed, false);
    assert.equal(h.saves[0][3].transcription_incomplete, true);
    assert.equal(h.calls.includes('cleanup_checkpoints'), status === 'success');
    assert.equal(h.calls.includes('release_recovered_capture'), ['success', 'partial'].includes(status));
    h.hooks.unmount();
  });
}

for (const status of ['none', 'failed', 'throw']) {
  test(`a live draft stays recoverable when previously detected audio yields ${status}`, async () => {
    const h = recoveryHarness({ status, hasAudio: true, transcripts: [{ text: 'Synthetic draft', timestamp: '00:00' }] });
    await h.render().checkForRecoverableTranscripts();
    await assert.rejects(h.render().recoverMeeting('interrupted'), /No audio could be recovered yet/);
    assert.equal(h.saves.length, 0);
    assert.ok(!h.calls.includes('markSaved'));
    assert.equal(h.render().recoverableMeetings[0].recordingFolder, 'synthetic-recording');
    h.hooks.unmount();
  });
}

test('recovery preserves timing, sequence, speaker and word fields from both cache formats', async () => {
  const words = [{ text: 'Sample', start: 0, end: 1 }];
  const h = recoveryHarness({ transcripts: [
    { id: 1, text: 'Sample', timestamp: '12:00', sequence_id: 0, source: 'Me',
      chunk_start_time: 0, audio_start_time: 0, audio_end_time: 1, duration: 1,
      confidence: null, is_partial: false, word_timestamps: words },
    { id: 2, text: 'Later', timestamp: '12:02', sequenceId: 3, speaker: 'Participants' },
  ] });
  await h.render().checkForRecoverableTranscripts();
  await h.render().recoverMeeting('interrupted');
  const [first, second] = h.saves[0][1];
  assert.equal(first.id, '1');
  assert.equal(first.sequence_id, 0);
  assert.equal(first.speaker, 'Me');
  assert.equal(first.chunk_start_time, 0);
  assert.equal(first.audio_start_time, 0);
  assert.equal(first.audio_end_time, 1);
  assert.equal(first.duration, 1);
  assert.equal(first.confidence, null);
  assert.deepEqual(first.word_timestamps, words);
  assert.equal(second.sequence_id, 3);
  assert.equal(second.speaker, 'Participants');
  h.hooks.unmount();
});

for (const failure of ['failSave', 'failRead']) {
  test(`${failure} retains recovery data instead of treating it as an empty successful meeting`, async () => {
    const h = recoveryHarness({ [failure]: true });
    await h.render().checkForRecoverableTranscripts();
    await assert.rejects(h.render().recoverMeeting('interrupted'), /Synthetic/);
    assert.ok(!h.calls.includes('markSaved'));
    assert.ok(!h.calls.includes('cleanup_checkpoints'));
    assert.ok(!h.calls.includes('release_recovered_capture'));
    assert.equal(h.render().recoverableMeetings.length, 1);
    h.hooks.unmount();
  });
}
