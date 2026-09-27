import assert from 'node:assert/strict';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

function createHook(invoke) {
  const messages = [];
  const harness = createHookHarness();
  const react = harness.react;
  const props = { meeting: { id: 'synthetic', title: 'Review', transcripts: [] }, summaryData: { markdown: 'Old summary' } };
  const modulePath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../src/hooks/meeting-details/useMeetingData.ts');
  const { useMeetingData } = loadTsModule(modulePath, {
    react,
    '@tauri-apps/api/core': { invoke },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ meetings: [], setMeetings() {}, setCurrentMeeting() {} }) },
    sonner: { toast: { success(message) { messages.push({ kind: 'success', message }); }, error(message) { messages.push({ kind: 'error', message }); } } },
  });
  const render = () => harness.render(() => useMeetingData(props));
  render.unmount = () => harness.unmount();
  render.messages = messages;
  return render;
}

test('a later title save retains the successfully edited summary', async () => {
  const calls = [];
  const render = createHook(async (command, args) => { calls.push({ command, args }); });
  const corrected = { markdown: 'Corrected notes', summary_json: [{ id: 'stable', type: 'paragraph', content: [] }] };
  await render().handleSaveSummary(corrected);
  assert.equal(render().aiSummary, corrected);
  render().handleTitleChange('Renamed review');
  await render().saveAllChanges();
  const summaries = calls.filter(call => call.command === 'api_save_meeting_summary');
  assert.equal(summaries.length, 1, 'A title-only save must not rewrite an unchanged summary');
  assert.equal(render().aiSummary.markdown, 'Corrected notes');
});

test('a failed summary write rejects instead of clearing the editor dirty state', async () => {
  const render = createHook(async () => { throw new Error('Synthetic disk failure'); });
  await assert.rejects(render().handleSaveSummary({ markdown: 'Edited draft' }), /Synthetic disk failure/);
  assert.equal(render().aiSummary.markdown, 'Old summary');
});

test('a failed title write preserves the draft and reports failure instead of success', async () => {
  const render = createHook(async () => { throw new Error('Synthetic title failure'); });
  render().handleTitleChange('Draft title');
  await render().saveAllChanges();
  assert.equal(render().isTitleDirty, true);
  assert.equal(render().meetingTitle, 'Draft title');
  assert.deepEqual(render.messages.map(message => message.kind), ['error']);
});

test('duplicate save clicks share one write and later title edits remain dirty', async () => {
  const pending = deferred();
  let writes = 0;
  const render = createHook((command) => { if (command === 'api_save_meeting_title') { writes++; return pending.promise; } return Promise.resolve({}); });
  render().handleTitleChange('First title');
  const current = render();
  const saving = current.saveAllChanges();
  await current.saveAllChanges();
  await flush();
  assert.equal(writes, 1);
  render().handleTitleChange('Later title');
  pending.resolve();
  await saving;
  assert.equal(render().meetingTitle, 'Later title');
  assert.equal(render().isTitleDirty, true);
  await render().saveAllChanges();
});

test('edits made during a legacy summary save are retained for the next save', async () => {
  const pending = deferred();
  const render = createHook(() => pending.promise);
  const first = { overview: { title: 'Overview', blocks: [{ content: 'First' }] } };
  const later = { overview: { title: 'Overview', blocks: [{ content: 'Later' }] } };
  render().handleSummaryChange(first);
  const saving = render().saveAllChanges();
  render().handleSummaryChange(later);
  pending.resolve();
  await saving;
  assert.equal(render().aiSummary, later);
  await render().saveAllChanges();
});


test('switching meetings flushes the edited snapshot exactly once', async () => {
  const calls = [];
  const render = createHook(async (command, args) => { calls.push({ command, args }); return {}; });
  render();
  render().handleSummaryDraft({ summary_json: [{ id: 'edited' }] }, async () => ({ markdown: 'Edited before switching', summary_json: [{ id: 'edited' }] }));
  render.unmount();
  await flush();
  const writes = calls.filter(call => call.command === 'api_save_meeting_summary');
  assert.equal(writes.length, 1);
  assert.equal(writes[0].args.meetingId, 'synthetic');
  assert.equal(writes[0].args.summary.markdown, 'Edited before switching');
});
