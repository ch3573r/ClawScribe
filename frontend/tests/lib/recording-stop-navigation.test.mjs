import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

// Runs one recording stop and changes the route before the save finishes.
async function stopOn(location) {
  const [pathname, search = ''] = location.split('?');
  const previousWindow = globalThis.window;
  const previousTimer = globalThis.setTimeout;
  const values = new Map([
    ['last_recording_folder_path', `synthetic-meeting-${location}`],
    ['last_recording_meeting_name', 'Synthetic meeting'],
  ]);
  const route = { pathname: '/', search: '' };
  globalThis.window = { location: route, sessionStorage: {
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key),
  } };
  globalThis.setTimeout = (callback) => {
    queueMicrotask(callback);
    return 1;
  };
  const hooks = createHookHarness();
  const pushes = [];
  const replaces = [];
  const summaries = [];
  const statuses = [];
  const toasts = [];
  const noop = () => {};
  const asyncNoop = async () => {};
  try {
    const { useRecordingStop } = loadTsModule('src/hooks/useRecordingStop.ts', {
      react: hooks.react,
      'next/navigation': { useRouter: () => ({ push: path => pushes.push(path), replace: path => replaces.push(path) }) },
      '@tauri-apps/api/core': { invoke: async (command, args) => {
        if (command === 'start_recording_summary') summaries.push({ ...args });
      } },
      '@tauri-apps/api/event': { listen: async () => noop },
      sonner: { toast: Object.assign(() => {}, { warning: noop, error: noop, success: (title, options) => toasts.push({ title, options }) }) },
      '@/contexts/TranscriptContext': { useTranscripts: () => ({
        transcriptsRef: { current: [] }, flushBuffer: noop, clearTranscripts: noop,
        meetingTitle: 'Synthetic', markMeetingAsSaved: asyncNoop,
      }) },
      '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({
        refetchMeetings: asyncNoop, setCurrentMeeting: noop, setMeetings: noop,
        meetings: [], setIsMeetingActive: noop, startSummaryPolling: noop,
      }) },
      '@/contexts/RecordingStateContext': {
        useRecordingState: () => ({ status: 'STOPPING', setStatus: value => statuses.push(value) }),
        RecordingStatus: Object.fromEntries(['STOPPING', 'PROCESSING_TRANSCRIPTS', 'SAVING', 'COMPLETED', 'IDLE', 'ERROR'].map(value => [value, value])),
      },
      '@/services/storageService': { storageService: {
        saveMeeting: async () => {
          route.pathname = pathname;
          route.search = search ? '?' + search : '';
          return { meeting_id: 'saved' };
        },
        getMeeting: async () => ({ title: 'Synthetic meeting' }),
      } },
      '@/services/transcriptService': { transcriptService: { getTranscriptionStatus: async () => ({ is_processing: false, chunks_in_queue: 0 }) } },
      '@/lib/analytics': { __esModule: true, default: new Proxy({}, { get: () => asyncNoop }) },
      '@/lib/meetingCalendar': { getActiveRecordingCalendar: () => null, setActiveRecordingCalendar: noop },
      '@/lib/summary-language-preferences': { applyPinnedSummaryLanguageToMeeting: async () => true, detectAndCacheSummaryLanguage: asyncNoop },
      '@tauri-apps/plugin-store': { Store: { load: async () => ({ get: async () => 0 }) } },
    });
    const hook = hooks.render(() => useRecordingStop(noop, noop));
    await hook.handleRecordingStop(true);
    hooks.unmount();
    return { pushes, replaces, summaries, statuses, toasts };
  } finally {
    globalThis.window = previousWindow;
    globalThis.setTimeout = previousTimer;
  }
}

test('Home opens the saved meeting after submitting its ID once', async () => {
  for (const home of ['/', '/index.html']) {
    const result = await stopOn(home);
    assert.deepEqual(result.pushes, ['/meeting-details?id=saved'], home);
    assert.deepEqual(result.summaries, [{ meetingId: 'saved' }]);
  }
});

for (const page of ['/meetings', '/settings', '/settings/', '/settings.html', '/meeting-details?id=other']) {
  test(`stop started on Home, user left for ${page}: they stay put and the summary still starts`, async () => {
    const result = await stopOn(page);
    assert.deepEqual(result.pushes, [], 'no automatic navigation');
    assert.deepEqual(result.replaces, []);
    assert.deepEqual(result.summaries, [{ meetingId: 'saved' }]);
    assert.ok(result.statuses.includes('IDLE'), 'the recording state is reset');
    assert.equal(result.toasts[0].options.action.label, 'View meeting', 'the toast still offers the meeting');
  });
}

test('an already open meeting receives no second navigation or summary request', async () => {
  const result = await stopOn('/meeting-details?id=saved');
  assert.deepEqual(result.pushes, []);
  assert.deepEqual(result.replaces, []);
  assert.deepEqual(result.summaries, [{ meetingId: 'saved' }]);
});
