import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

function createView({ fromRecording = false } = {}) {
  const harness = createHookHarness();
  let meetingId = 'first';
  let metadata = null;
  const calls = [];
  const summaries = new Map();
  const jsx = (type, props) => ({ type, props });
  const sidebar = { setCurrentMeeting() {}, refetchMeetings() {}, stopSummaryPolling() {} };
  const router = { push() {} };
  const transcripts = [{ id: 'segment', text: 'A synthetic test meeting.' }];
  const { default: MeetingDetails } = loadTsModule(fileURLToPath(new URL('../../src/app/meeting-details/page.tsx', import.meta.url)), {
    react: { ...harness.react, Suspense: 'Suspense' },
    'react/jsx-runtime': { jsx, jsxs: jsx },
    './page-content': { default: 'PageContent', __esModule: true },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => sidebar },
    'next/navigation': { useRouter: () => router, useSearchParams: () => ({ get: key => key === 'id' ? meetingId : fromRecording ? 'recording' : null }) },
    '@/lib/analytics': { default: { trackPageView() {} }, __esModule: true },
    '@/contexts/ConfigContext': { useConfig: () => ({ isAutoSummary: true }) },
    '@/hooks/usePaginatedTranscripts': { usePaginatedTranscripts: () => ({ metadata, transcripts }) },
    '@tauri-apps/api/event': { listen: async () => () => {} },
    '@tauri-apps/api/core': { invoke(command, args) {
      calls.push({ command, args });
      if (command === 'api_get_summary') {
        const request = deferred();
        summaries.set(args.meetingId, request);
        return request.promise;
      }
      return Promise.resolve(command === 'api_get_model_config' ? {} : null);
    } },
  });
  const findContent = node => {
    if (!node || typeof node !== 'object') return undefined;
    if (Array.isArray(node)) return node.map(findContent).find(Boolean);
    if (node.type === 'PageContent') return node.props;
    return findContent(node.props?.children);
  };
  const render = () => harness.render(() => MeetingDetails().props.children.type());
  return {
    ...harness, calls, summaries, render,
    content() { return findContent(render()); },
    navigate(id) { meetingId = id; metadata = null; render(); },
    loadMetadata() { metadata = { id: meetingId, title: meetingId }; render(); },
  };
}

test('a summary that arrives after navigation cannot appear in or overwrite the next meeting', async () => {
  const view = createView();
  view.render();
  view.navigate('second');
  view.loadMetadata();
  view.summaries.get('second').resolve({ status: 'completed', data: { markdown: 'Second notes' } });
  await flush();
  view.summaries.get('first').resolve({ status: 'completed', data: { markdown: 'First notes' } });
  await flush();
  const content = view.content();
  assert.equal(content.meeting.id, 'second');
  assert.equal(content.summaryData.markdown, 'Second notes');
  view.unmount();
});

test('auto-generation waits for saved notes to load and preserves an existing summary', async () => {
  const view = createView({ fromRecording: true });
  view.render();
  view.loadMetadata();
  view.render();
  await flush();
  assert.equal(view.calls.some(call => call.command === 'api_get_model_config'), false);
  view.summaries.get('first').resolve({ status: 'completed', data: { markdown: 'Already saved' } });
  await flush();
  assert.equal(view.content().shouldAutoGenerate, false);
  await flush();
  assert.equal(view.calls.some(call => call.command === 'api_get_model_config'), false);
  view.unmount();
});

test('automatic summaries never configure a provider when none was selected', async () => {
  const view = createView({ fromRecording: true });
  view.render();
  view.loadMetadata();
  view.summaries.get('first').resolve({ status: 'idle' });
  await flush();
  view.render();
  await flush();
  assert.equal(view.calls.filter(call => call.command === 'api_get_model_config').length, 1);
  assert.equal(view.calls.some(call => call.command.startsWith('api_save_')), false);
  assert.equal(view.content().shouldAutoGenerate, false);
  view.unmount();
});
