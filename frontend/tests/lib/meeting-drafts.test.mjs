import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { deferred } from './hook-harness.mjs';

const { createDraftQueue, confirmSummaryRegeneration } = loadTsModule(fileURLToPath(new URL('../../src/lib/meetingDrafts.ts', import.meta.url)), {
  '@tauri-apps/api/core': { invoke: async () => ({}) },
  sonner: { toast: { error() {} } },
});

test('failed autosave retains the dirty revision and permits retry', async () => {
  let fail = true;
  const queue = createDraftQueue(async () => { if (fail) throw new Error('Synthetic disk error'); });
  queue.edit({ summary: { markdown: 'Unsaved edits' } });
  await assert.rejects(queue.flush(), /Synthetic disk error/);
  assert.equal(queue.dirty(), true);
  assert.equal(queue.state, 'error');
  assert.equal(queue.edits.summary.markdown, 'Unsaved edits');
  fail = false;
  await queue.flush();
  assert.equal(queue.state, 'saved');
  assert.equal(queue.dirty(), false);
});

test('a navigation flush drains edits arriving during the first write', async () => {
  const pending = deferred();
  const writes = [];
  const queue = createDraftQueue(async edits => { writes.push(edits.title); if (writes.length === 1) await pending.promise; });
  queue.edit({ title: 'First edit' });
  const leaving = queue.drain();
  queue.edit({ title: 'Latest edit' });
  pending.resolve();
  await leaving;
  assert.deepEqual(writes, ['First edit', 'Latest edit']);
  assert.equal(queue.dirty(), false);
});

test('regeneration flushes edits then asks before allowing replacement', async () => {
  const calls = [];
  const accepted = await confirmSummaryRegeneration(
    async () => { calls.push('save'); },
    async () => { calls.push('read'); return { data: { user_edited_at: 'synthetic' } }; },
    async message => { calls.push(message); return false; },
  );
  assert.equal(accepted, false);
  assert.deepEqual(calls, ['save', 'read', 'Regenerate will replace your edited summary. You can restore it afterwards.']);
  await assert.rejects(confirmSummaryRegeneration(
    async () => { throw new Error('Save failed'); },
    async () => { assert.fail('Must not continue after failed save'); },
    async () => true,
  ), /Save failed/);
});

test('autosave debounces successive edits into one write', async () => {
  const saved = deferred();
  const writes = [];
  const queue = createDraftQueue(async edits => { writes.push(edits.title); saved.resolve(); });
  queue.edit({ title: 'First title' });
  queue.edit({ title: 'Final title' });
  await saved.promise;
  await queue.drain();
  assert.deepEqual(writes, ['Final title']);
  assert.equal(queue.state, 'saved');
});

test('generation blocks draft writes without discarding edits', async () => {
  let status = 'processing';
  const writes = [];
  const { meetingDraft } = loadTsModule(fileURLToPath(new URL('../../src/lib/meetingDrafts.ts', import.meta.url)), {
    '@tauri-apps/api/core': { invoke: async command => {
      if (command === 'api_get_summary') return { status };
      if (command.startsWith('api_save_meeting_')) writes.push(command);
    } },
    sonner: { toast: { error() {} } },
  });
  const queue = meetingDraft('generation-race');
  queue.edit({ title: 'Edited title', summary: { markdown: 'Edited notes' } });
  await assert.rejects(queue.flush(), /generation is still running/);
  assert.equal(queue.dirty(), true);
  assert.equal(writes.length, 0);
  status = 'completed';
  await queue.drain();
  assert.deepEqual(writes, ['api_save_meeting_title', 'api_save_meeting_summary']);
});

test('a rejected quit flush reports failure with its attempt identity', async () => {
  const calls = [];
  const { flushSummaryEditsBeforeExit } = loadTsModule(fileURLToPath(new URL('../../src/lib/meetingDrafts.ts', import.meta.url)), {
    '@tauri-apps/api/core': { invoke: async (command, args) => { calls.push([command, args.attemptId]); } },
    sonner: { toast: { error() {} } },
  });
  await flushSummaryEditsBeforeExit(12, async () => { throw new Error('Synthetic save failure'); });
  assert.deepEqual(calls, [['api_summary_edit_exit_failed', 12]]);
  await flushSummaryEditsBeforeExit(13, async () => {});
  assert.deepEqual(calls[1], ['api_finish_summary_edit_exit', 13]);
});

test('deleting a meeting retires its draft without failing a quit flush or losing another meeting edit', async () => {
  const saving = deferred();
  const started = deferred();
  const saves = [];
  const pending = [];
  const { meetingDraft, discardMeetingDraft, flushMeetingDrafts } = loadTsModule(fileURLToPath(new URL('../../src/lib/meetingDrafts.ts', import.meta.url)), {
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'api_get_summary') return { status: 'completed' };
      if (command === 'api_set_summary_edits_pending') { pending.push(args.pending); return; }
      saves.push([args.meetingId, command]);
      if (args.meetingId === 'deleted') { started.resolve(); await saving.promise; }
    } },
    sonner: { toast: { error() { assert.fail('Deletion must not turn a late write into a save error'); } } },
  });
  const deleted = meetingDraft('deleted');
  deleted.edit({ title: 'Pending title', summary: { markdown: 'Pending summary' } });
  const writing = deleted.flush();
  writing.catch(() => {});
  await started.promise;
  discardMeetingDraft('deleted');
  deleted.edit({ title: 'Late editor update' });
  meetingDraft('deleted').edit({ title: 'Late page mount' });
  const other = meetingDraft('other');
  other.edit({ title: 'Other edits' });
  saving.reject(new Error('Meeting was deleted during the title save'));
  await writing;
  await flushMeetingDrafts();
  assert.deepEqual(saves, [['deleted', 'api_save_meeting_title'], ['other', 'api_save_meeting_title']]);
  assert.equal(deleted.dirty(), false);
  assert.equal(pending.at(-1), false);
});
