import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const lib = loadTsModule('src/lib/interruptedRecordings.ts');
const json = value => JSON.parse(JSON.stringify(value));

const disk = (folder, extra = {}) => ({
  folder, title: 'Weekly sync', startedAt: '2026-11-01T10:00:00Z', approxDurationSeconds: 1800, sizeBytes: 5 * 1024 * 1024,
  hasTranscript: true, transcriptLines: 12, hasCapture: true, hasAudio: true,
  reasons: ['unfinished_audio'], reason: 'The recorded audio was not finished into a file', registeredMeetingId: null, kept: false, ...extra,
});
const browser = (meetingId, folderPath, extra = {}) => ({
  meetingId, title: 'From the browser', startTime: 1_000, lastUpdated: 2_000, transcriptCount: 3, savedToSQLite: false, folderPath, ...extra,
});

test('a folder known to both sources appears once, with the browser data and the disk information', () => {
  const merged = lib.mergeRecoveryCandidates(
    [browser('m1', undefined, { recordingFolder: 'D:\\Recordings\\Weekly_2026-11-01' })],
    [disk('d:/recordings/weekly_2026-11-01/')],
  );
  assert.equal(merged.length, 1);
  assert.equal(merged[0].source, 'browser');
  assert.equal(merged[0].meetingId, 'm1');
  assert.equal(merged[0].transcriptCount, 3, 'the browser transcript count is kept');
  assert.equal(merged[0].disk.folder, 'd:/recordings/weekly_2026-11-01/');
  assert.equal(merged[0].recordingFolder, 'D:\\Recordings\\Weekly_2026-11-01');
});

test('a recording only on disk becomes a disk entry and a browser entry without a folder stays', () => {
  const merged = lib.mergeRecoveryCandidates(
    [browser('b-only', undefined), browser('b-folder', 'C:\\Rec\\one', { recordingFolder: 'C:\\Rec\\one' })],
    [disk('C:\\Rec\\one'), disk('C:\\Rec\\two', { title: null, hasAudio: false, hasCapture: false, transcriptLines: 0, hasTranscript: false })],
  );
  assert.deepEqual(merged.map(entry => [entry.meetingId, entry.source]).sort(), [
    ['b-folder', 'browser'], ['b-only', 'browser'], ['disk:C:\\Rec\\two', 'disk'],
  ]);
  const onDisk = merged.find(entry => entry.source === 'disk');
  assert.equal(onDisk.title, 'Untitled recording');
  assert.equal(onDisk.folderPath, undefined, 'no audio on disk shows as no audio');
  assert.equal(onDisk.disk.folder, 'C:\\Rec\\two');
});

test('the same folder twice on disk and a folder spelled two ways are one entry', () => {
  const merged = lib.mergeRecoveryCandidates([], [disk('C:\\Rec\\one'), disk('c:/rec/one/')]);
  assert.equal(merged.length, 1);
  assert.equal(lib.folderKey('C:\\Rec\\One\\'), lib.folderKey('c:/rec/one'));
  assert.notEqual(lib.folderKey('C:\\Rec\\one'), lib.folderKey('C:\\Rec\\two'));
  assert.equal(lib.folderKey(''), '');
  assert.equal(lib.folderKey(undefined), '');
});

test('a scan result from the core is read tolerantly', () => {
  assert.equal(lib.readDiskScan(null).recordings.length, 0);
  assert.equal(lib.readDiskScan('nope').recordings.length, 0);
  const scan = lib.readDiskScan({
    truncated: true, recordingActive: false,
    recordings: [{ folder: 'C:\\Rec\\one', title: ' ', sizeBytes: 'big', hasAudio: true, reasons: ['unfinished_audio', 7] }, { title: 'no folder' }, null],
  });
  assert.equal(scan.truncated, true);
  assert.equal(scan.recordings.length, 1);
  assert.equal(scan.recordings[0].title, null);
  assert.equal(scan.recordings[0].sizeBytes, 0);
  assert.deepEqual(json(scan.recordings[0].reasons), ['unfinished_audio']);
  assert.equal(scan.recordings[0].reason, 'The recording was interrupted');
});

test('duration and size are described briefly', () => {
  assert.equal(lib.describeDuration(30), 'under a minute');
  assert.equal(lib.describeDuration(1800), '30 min');
  assert.equal(lib.describeDuration(3900), '1 h 5 min');
  assert.equal(lib.describeDuration(null), null);
  assert.equal(lib.describeSize(5 * 1024 * 1024), '5 MB');
  assert.equal(lib.describeSize(100), '1 KB');
  assert.equal(lib.describeSize(0), null);
});

function harness({ browserMeetings = [], scan = { recordings: [], truncated: false, recordingActive: false }, scanFails = false, audioStatus = 'success', failDiscard = false } = {}) {
  const hooks = createHookHarness();
  const calls = [];
  const saves = [];
  const { useTranscriptRecovery } = loadTsModule('src/hooks/useTranscriptRecovery.ts', {
    react: hooks.react,
    '@/lib/interruptedRecordings': lib,
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === 'has_audio_checkpoints') return true;
      if (command === 'find_interrupted_recordings') {
        if (typeof scanFails === 'function' ? scanFails() : scanFails) throw new Error('scan failed');
        return typeof scan === 'function' ? scan() : scan;
      }
      if (command === 'preview_interrupted_recording') return [{ text: 'first line', timestamp: '[00:01]', audioStartTime: 1 }];
      if (command === 'recover_audio_from_checkpoints') {
        return { status: audioStatus, chunk_count: 1, estimated_duration_seconds: 1, audio_file_path: ['success', 'partial'].includes(audioStatus) ? args.meetingFolder + '/audio-recovered.wav' : undefined };
      }
      if (command === 'keep_interrupted_recording') return;
      if (command === 'repair_saved_recording') return { meetingId: args.folder === 'C:\Rec\saved' ? 'meeting-7' : 'other', audioStatus: 'success', audioPath: args.folder + '/audio-recovered.wav' };
      if (command === 'discard_interrupted_recording') { if (failDiscard) throw new Error('refused'); return; }
      if (command === 'release_recovered_capture' || command === 'cleanup_checkpoints') return;
      throw new Error('Unexpected command: ' + command);
    } },
    '@/services/indexedDBService': { indexedDBService: {
      getAllMeetings: async () => browserMeetings,
      getMeetingMetadata: async id => browserMeetings.find(m => m.meetingId === id),
      getTranscripts: async () => [],
      markMeetingSaved: async id => { calls.push(['markSaved', id]); },
      deleteMeeting: async id => { calls.push(['deleteBrowser', id]); },
    } },
    '@/services/storageService': { storageService: { saveMeeting: async (...args) => { calls.push(['save']); saves.push(args); return { meeting_id: 'saved-1' }; } } },
    '@/lib/summary-language-preferences': { applyPinnedSummaryLanguageToMeeting: async () => {} },
    sonner: { toast: { warning: () => {} } },
  });
  const render = () => hooks.render(useTranscriptRecovery);
  const names = () => calls.map(call => call[0]);
  return { render, calls, saves, names, hooks };
}

const old = Date.now() - 60_000;

test('the recovery list merges the browser data with what the core found on disk', async () => {
  const h = harness({
    browserMeetings: [browser('m1', 'C:\\Rec\\one', { lastUpdated: old })],
    scan: { recordings: [disk('C:\\Rec\\one'), disk('C:\\Rec\\two', { title: 'Only on disk' })], truncated: true, recordingActive: false },
  });
  const found = await h.render().checkForRecoverableTranscripts();
  assert.equal(found.length, 2);
  const list = h.render().recoverableMeetings;
  assert.deepEqual(json(list.map(entry => entry.source).sort()), ['browser', 'disk']);
  assert.equal(h.render().diskScanTruncated, true);
  h.hooks.unmount();
});

test('a failed native scan clears the offered cache and preserves browser-only drafts for retry', async () => {
  let scanFails = false;
  const h = harness({
    browserMeetings: [
      browser('pending', undefined, { lastUpdated: old }),
      browser('saved', 'saved-folder', { lastUpdated: old }),
      browser('deleted', 'retained-folder', { lastUpdated: old }),
      browser('current', 'current-folder', { lastUpdated: old }),
    ],
    scan: { recordings: [], activeCacheId: 'current', resolvedCacheIds: ['saved', 'deleted'] },
    scanFails: () => scanFails,
  });
  assert.equal((await h.render().checkForRecoverableTranscripts()).length, 1);
  scanFails = true;
  assert.deepEqual(json(await h.render().checkForRecoverableTranscripts()), []);
  assert.deepEqual(json(h.render().recoverableMeetings), []);
  assert.equal(h.render().scanFailed, true);
  await assert.rejects(h.render().recoverMeeting('pending'), /check for interrupted recordings/i);
  await assert.rejects(h.render().deleteRecoverableMeeting('pending'), /check for interrupted recordings/i);
  assert.ok(!h.names().includes('deleteBrowser'));
  assert.equal(h.saves.length, 0);
  scanFails = false;
  const found = await h.render().checkForRecoverableTranscripts();
  assert.deepEqual(json(found.map(entry => entry.meetingId)), ['pending']);
  assert.equal(h.render().scanFailed, false);
  h.hooks.unmount();
});

test('a saved or deleted native recording cannot reappear from the browser cache', async () => {
  const h = harness({
    browserMeetings: [
      browser('committed', 'saved-folder', { lastUpdated: old }),
      browser('deleted', 'retained-folder', { lastUpdated: old }),
      browser('pending', undefined, { lastUpdated: old }),
    ],
    scan: { recordings: [], resolvedCacheIds: ['committed', 'deleted'] },
  });
  const found = await h.render().checkForRecoverableTranscripts();
  assert.deepEqual(json(found.map(entry => entry.meetingId)), ['pending']);
  assert.deepEqual(json(h.calls[0][1].cacheEntries.map(entry => entry.id)), ['committed', 'deleted', 'pending']);
  assert.ok(!h.names().includes('has_audio_checkpoints'));
  h.hooks.unmount();
});

test('a late successful recovery scan cannot repopulate the list after a newer scan fails', async () => {
  let finishFirst;
  let requests = 0;
  const h = harness({
    browserMeetings: [browser('pending', undefined, { lastUpdated: old })],
    scan: () => {
      if (++requests === 1) return new Promise(resolve => { finishFirst = resolve; });
      throw new Error('newer scan failed');
    },
  });
  const first = h.render().checkForRecoverableTranscripts();
  await Promise.resolve();
  await h.render().checkForRecoverableTranscripts();
  finishFirst({ recordings: [] });
  await first;
  assert.equal(h.render().scanFailed, true);
  assert.equal(h.render().recoverableMeetings.length, 0);
  h.hooks.unmount();
});

test('a folder reused by another recording is never attached to a detached browser entry', async () => {
  const folder = 'C:\\Rec\\reused';
  const h = harness({
    browserMeetings: [browser('old-browser', folder, { lastUpdated: old })],
    scan: { recordings: [disk(folder)], detachedCacheIds: ['old-browser'] },
  });
  const entries = await h.render().checkForRecoverableTranscripts();
  assert.equal(entries.length, 2);
  const oldEntry = entries.find(entry => entry.meetingId === 'old-browser');
  assert.equal(oldEntry.disk, undefined);
  assert.equal(oldEntry.folderPath, undefined);
  assert.equal(oldEntry.recordingFolder, undefined);
  assert.ok(!h.names().includes('has_audio_checkpoints'));
  await assert.rejects(h.render().recoverMeeting('old-browser'), /No audio could be recovered yet/);
  assert.ok(!h.names().includes('recover_audio_from_checkpoints'));
  assert.equal(h.saves.length, 0);
  h.hooks.unmount();
});

test('old browser-only recovery stays visible while a live or just-written cache stays hidden', async () => {
  const older = Date.now() - 30 * 24 * 60 * 60 * 1000;
  const h = harness({
    browserMeetings: [
      browser('old-unsaved', undefined, { lastUpdated: older }),
      browser('old-resolved', undefined, { lastUpdated: older }),
      browser('live', 'C:\\Recordings\\current', { lastUpdated: old }),
      browser('just-written', undefined, { lastUpdated: Date.now() }),
    ],
    scan: { recordings: [], activeCacheId: 'live', resolvedCacheIds: ['old-resolved'] },
  });
  const found = await h.render().checkForRecoverableTranscripts();
  assert.deepEqual(json(found.map(entry => entry.meetingId)), ['old-unsaved']);
  assert.ok(h.calls[0][1].cacheEntries.some(entry => entry.id === 'old-resolved'));
  h.hooks.unmount();
});

test('recovering a clean Stop handoff does not mark its complete transcript as partial', async () => {
  const h = harness({ scan: { recordings: [disk('pending-folder', { reasons: ['library_handoff_pending'] })] } });
  await h.render().checkForRecoverableTranscripts();
  await h.render().recoverMeeting(h.render().recoverableMeetings[0].meetingId);
  assert.equal(h.saves[0][3].transcription_incomplete, false);
  assert.equal(h.saves[0][4], true);
  h.hooks.unmount();
});

test('a recording only on disk is recovered through audio recovery and the normal save, then released', async () => {
  const h = harness({ scan: { recordings: [disk('C:\\Rec\\two', { title: 'Only on disk' })], truncated: false, recordingActive: false } });
  await h.render().checkForRecoverableTranscripts();
  const id = h.render().recoverableMeetings[0].meetingId;
  const result = await h.render().recoverMeeting(id);
  assert.equal(result.success, true);
  assert.equal(result.meetingId, 'saved-1');
  assert.equal(h.saves.length, 1);
  const [title, transcripts, folder, outcome, useSaved] = h.saves[0];
  assert.equal(title, 'Only on disk');
  assert.equal(transcripts.length, 0, 'the core reads the transcript snapshot from the folder');
  assert.equal(folder, 'C:\\Rec\\two');
  assert.equal(useSaved, true);
  assert.equal(outcome.transcription_incomplete, true);
  assert.equal(outcome.capture_incomplete, false);
  const names = h.names();
  assert.ok(names.indexOf('recover_audio_from_checkpoints') < names.indexOf('save'));
  assert.ok(names.indexOf('save') < names.indexOf('release_recovered_capture'));
  assert.ok(names.includes('cleanup_checkpoints'));
  assert.equal(h.render().recoverableMeetings.length, 0);
  h.hooks.unmount();
});

test('a disk recording without a transcript snapshot is saved for transcription and is not read as a saved one', async () => {
  const h = harness({ scan: { recordings: [disk('C:\\Rec\\audio-only', { hasTranscript: false, transcriptLines: 0 })], truncated: false, recordingActive: false }, audioStatus: 'partial' });
  await h.render().checkForRecoverableTranscripts();
  await h.render().recoverMeeting(h.render().recoverableMeetings[0].meetingId);
  const [, , , outcome, useSaved] = h.saves[0];
  assert.equal(useSaved, false);
  assert.equal(outcome.capture_incomplete, true);
  assert.equal(outcome.transcription_incomplete, false);
  assert.equal(h.names().includes('cleanup_checkpoints'), false, 'a partial recovery keeps the originals');
  h.hooks.unmount();
});

test('when no audio can be recovered and there is no transcript the entry and the folder are kept', async () => {
  const h = harness({
    scan: { recordings: [disk('C:\\Rec\\empty', { hasTranscript: false, transcriptLines: 0, hasAudio: false, hasCapture: false, reasons: ['did_not_finish'] })], truncated: false, recordingActive: false },
  });
  await h.render().checkForRecoverableTranscripts();
  await assert.rejects(h.render().recoverMeeting(h.render().recoverableMeetings[0].meetingId), /No audio could be recovered yet/);
  assert.equal(h.saves.length, 0);
  assert.equal(h.render().recoverableMeetings.length, 1);
  assert.equal(h.names().includes('discard_interrupted_recording'), false, 'nothing is deleted by a failed recovery');
  h.hooks.unmount();
});

test('a disk recording is previewed by the core and deleted only on request and through the core', async () => {
  const h = harness({ scan: { recordings: [disk('C:\\Rec\\two')], truncated: false, recordingActive: false } });
  await h.render().checkForRecoverableTranscripts();
  const id = h.render().recoverableMeetings[0].meetingId;
  const preview = await h.render().loadMeetingTranscripts(id);
  assert.equal(preview[0].text, 'first line');
  assert.equal(h.names().includes('discard_interrupted_recording'), false, 'looking never deletes');
  await h.render().deleteRecoverableMeeting(id);
  assert.deepEqual(json(h.calls.find(call => call[0] === 'discard_interrupted_recording')[1]), { folder: 'C:\\Rec\\two' });
  assert.equal(h.render().recoverableMeetings.length, 0);
  h.hooks.unmount();
});

test('a failed deletion keeps the entry, and a recording saved in the library is never deleted from here', async () => {
  const failing = harness({ scan: { recordings: [disk('C:\\Rec\\two')], truncated: false, recordingActive: false }, failDiscard: true });
  await failing.render().checkForRecoverableTranscripts();
  await assert.rejects(failing.render().deleteRecoverableMeeting(failing.render().recoverableMeetings[0].meetingId));
  assert.equal(failing.render().recoverableMeetings.length, 1);
  failing.hooks.unmount();

  const saved = harness({ scan: { recordings: [disk('C:\\Rec\\two', { registeredMeetingId: 'meeting-7' })], truncated: false, recordingActive: false } });
  await saved.render().checkForRecoverableTranscripts();
  await assert.rejects(saved.render().deleteRecoverableMeeting(saved.render().recoverableMeetings[0].meetingId), /saved in your library/);
  assert.equal(saved.names().includes('discard_interrupted_recording'), false);
  saved.hooks.unmount();
});

test('deleting a browser entry only removes the browser data, never the folder', async () => {
  const h = harness({
    browserMeetings: [browser('m1', 'C:\\Rec\\one', { lastUpdated: old })],
    scan: { recordings: [disk('C:\\Rec\\one')], truncated: false, recordingActive: false },
  });
  await h.render().checkForRecoverableTranscripts();
  await h.render().deleteRecoverableMeeting('m1');
  assert.deepEqual(h.names().filter(name => name.startsWith('delete') || name.startsWith('discard')), ['deleteBrowser']);
  h.hooks.unmount();
});

test('a recording the user asked not to be asked about is left out and counted, and shown on request', async () => {
  const scan = { recordings: [disk('C:\Rec\one'), disk('C:\Rec\kept', { kept: true })], truncated: false, recordingActive: false };
  const h = harness({ scan });
  const found = await h.render().checkForRecoverableTranscripts();
  assert.equal(found.length, 1);
  assert.equal(found[0].disk.folder, 'C:\Rec\one');
  const all = await h.render().checkForRecoverableTranscripts({ includeKept: true });
  assert.equal(all.length, 2);
  h.hooks.unmount();
});

test('"Keep, do not ask again" goes to the core for that folder and removes the entry; nothing is deleted', async () => {
  const h = harness({ scan: { recordings: [disk('C:\Rec\one')], truncated: false, recordingActive: false } });
  await h.render().checkForRecoverableTranscripts();
  await h.render().keepRecoverableMeeting(h.render().recoverableMeetings[0].meetingId);
  const keep = h.calls.find(call => call[0] === 'keep_interrupted_recording');
  assert.deepEqual(json(keep[1]), { folder: 'C:\Rec\one', keep: true });
  assert.equal(h.render().recoverableMeetings.length, 0);
  assert.equal(h.names().includes('discard_interrupted_recording'), false);
  h.hooks.unmount();
});

test('a recording already saved in the library repairs that meeting and never saves a second one', async () => {
  const h = harness({ scan: { recordings: [disk('C:\Rec\saved', { registeredMeetingId: 'meeting-7' })], truncated: false, recordingActive: false } });
  await h.render().checkForRecoverableTranscripts();
  const result = await h.render().recoverMeeting(h.render().recoverableMeetings[0].meetingId);
  assert.equal(result.meetingId, 'meeting-7');
  assert.equal(result.success, true);
  assert.equal(h.saves.length, 0, 'no second meeting is saved');
  assert.deepEqual(json(h.calls.find(call => call[0] === 'repair_saved_recording')[1]), { folder: 'C:\Rec\saved' });
  assert.equal(h.names().includes('save'), false);
  assert.equal(h.render().recoverableMeetings.length, 0);
  h.hooks.unmount();
});
