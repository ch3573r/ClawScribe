import assert from 'node:assert/strict';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';

function createHook(invoke) {
  const slots = [];
  let cursor = 0;
  const react = {
    useEffect() {},
    useCallback: callback => callback,
    useRef(value) { return slots[cursor++] ??= { current: value }; },
    useState(value) {
      const index = cursor++;
      const slot = slots[index] ??= { value };
      return [slot.value, next => { slot.value = typeof next === 'function' ? next(slot.value) : next; }];
    },
  };
  const props = { meeting: { id: 'synthetic', title: 'Review', transcripts: [] }, summaryData: { markdown: 'Old summary' } };
  const modulePath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../src/hooks/meeting-details/useMeetingData.ts');
  const { useMeetingData } = loadTsModule(modulePath, {
    react,
    '@tauri-apps/api/core': { invoke },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ meetings: [], setMeetings() {}, setCurrentMeeting() {} }) },
    sonner: { toast: { success() {}, error() {} } },
  });
  return () => { cursor = 0; return useMeetingData(props); };
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
  assert.equal(summaries.length, 2);
  assert.equal(summaries[1].args.summary.markdown, 'Corrected notes');
});

test('a failed summary write rejects instead of clearing the editor dirty state', async () => {
  const render = createHook(async () => { throw new Error('Synthetic disk failure'); });
  await assert.rejects(render().handleSaveSummary({ markdown: 'Edited draft' }), /Synthetic disk failure/);
  assert.equal(render().aiSummary.markdown, 'Old summary');
});
