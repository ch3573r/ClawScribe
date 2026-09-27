import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';

// Model asynchronous IDB requests and transaction completion, including cursors.
function recoveryCache(rows) {
  const meetings = new Map(rows.map(row => [row.meetingId, { ...row }]));
  const transcripts = new Map(rows.map((row, id) => [id, { id, meetingId: row.meetingId }]));
  const module = loadTsModule(fileURLToPath(new URL('../../src/services/indexedDBService.ts', import.meta.url)));
  module.indexedDBService.db = {
    transaction() {
      let pending = 0;
      let finished = false;
      const transaction = {};
      const finish = () => setTimeout(() => {
        if (!pending && !finished) {
          finished = true;
          transaction.oncomplete?.();
        }
      }, 0);
      const request = operation => {
        assert.equal(finished, false, 'request must use an active transaction');
        pending++;
        const result = {};
        queueMicrotask(() => {
          result.result = operation();
          result.onsuccess?.({ target: result });
          pending--;
          finish();
        });
        return result;
      };
      transaction.objectStore = name => {
        const data = name === 'meetings' ? meetings : transcripts;
        return {
          get: key => request(() => data.has(key) ? { ...data.get(key) } : undefined),
          getAll: () => request(() => [...data.values()].map(row => ({ ...row }))),
          put: row => request(() => data.set(row.meetingId, { ...row })),
          delete: key => request(() => data.delete(key)),
          index: () => ({
            openCursor(meetingId) {
              pending++;
              const result = {};
              const keys = [...transcripts].filter(([, row]) => row.meetingId === meetingId).map(([id]) => id);
              let position = 0;
              const next = () => queueMicrotask(() => {
                const key = keys[position++];
                result.result = key === undefined ? null : {
                  delete: () => request(() => transcripts.delete(key)),
                  continue: next,
                };
                result.onsuccess?.({ target: result });
                if (key === undefined) {
                  pending--;
                  finish();
                }
              });
              next();
              return result;
            },
          }),
        };
      };
      return transaction;
    },
  };
  return { ...module, meetings, transcripts };
}

test('successful SQLite save immediately removes the recovery meeting and transcripts', async () => {
  const cache = recoveryCache([{ meetingId: 'saved', savedToSQLite: false }]);
  await cache.indexedDBService.markMeetingSaved('saved');
  assert.equal(cache.meetings.size, 0);
  assert.equal(cache.transcripts.size, 0);
});

test('library deletion removes matching recovery IDs by folder without deleting other drafts', async () => {
  const cache = recoveryCache([
    { meetingId: 'recovery-id', folderPath: 'synthetic-folder', savedToSQLite: true },
    { meetingId: 'other-draft', folderPath: 'other-folder', savedToSQLite: false },
  ]);
  await cache.indexedDBService.deleteMeeting('sqlite-id', 'synthetic-folder');
  assert.deepEqual([...cache.meetings.keys()], ['other-draft']);
  assert.deepEqual([...cache.transcripts.values()].map(row => row.meetingId), ['other-draft']);
});

test('startup cleanup runs once and age cleanup never expires unsaved recovery data', async () => {
  const cache = recoveryCache([
    { meetingId: 'old-saved', lastUpdated: 1, savedToSQLite: true },
    { meetingId: 'old-unsaved', lastUpdated: 1, savedToSQLite: false },
    { meetingId: 'recent-saved', lastUpdated: Date.now(), savedToSQLite: true },
  ]);
  let calls = 0;
  const cleanup = cache.indexedDBService.deleteSavedMeetings.bind(cache.indexedDBService);
  cache.indexedDBService.deleteSavedMeetings = hours => { calls++; return cleanup(hours); };
  await Promise.all([cache.cleanupTranscriptRecoveryOnce(), cache.cleanupTranscriptRecoveryOnce()]);
  assert.equal(calls, 1);
  await cache.indexedDBService.deleteOldMeetings(7);
  assert.deepEqual([...cache.meetings.keys()], ['old-unsaved', 'recent-saved']);
  assert.deepEqual([...cache.transcripts.values()].map(row => row.meetingId), ['old-unsaved', 'recent-saved']);
});
