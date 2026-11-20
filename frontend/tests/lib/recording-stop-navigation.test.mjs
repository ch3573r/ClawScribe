import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

// Runs one recording stop that was started on Home (so the hook instance belongs
// to Home), lets the user move to `location` before the 2 s timer fires, then
// reports what the app did.
async function stopOn(location, { autoSummary = true } = {}) {
  const [pathname, search = ''] = location.split('?');
  const previousWindow = globalThis.window;
  const previousTimer = globalThis.setTimeout;
  const values = new Map([
    ['last_recording_folder_path', `synthetic-meeting-${location}-${autoSummary}`],
    ['last_recording_meeting_name', 'Synthetic meeting'],
  ]);
  const timers = [];
  const route = { pathname: '/', search: '' };
  globalThis.window = { location: route, sessionStorage: {
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key),
  } };
  // The 2 s navigation timer is held back; the other short waits run at once.
  globalThis.setTimeout = (callback, delay) => {
    if (delay === 2000) timers.push(callback);
    else queueMicrotask(callback);
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
      '@tauri-apps/api/core': { invoke: async () => {} },
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
      '@/contexts/ConfigContext': { useConfig: () => ({ modelConfig: { provider: 'synthetic', model: 'synthetic' }, isAutoSummary: autoSummary }) },
      '@/lib/auto-summary': { startAutomaticSummary: async request => { summaries.push(request.meetingId); return true; } },
      '@/contexts/RecordingStateContext': {
        useRecordingState: () => ({ status: 'STOPPING', setStatus: value => statuses.push(value) }),
        RecordingStatus: Object.fromEntries(['STOPPING', 'PROCESSING_TRANSCRIPTS', 'SAVING', 'COMPLETED', 'IDLE', 'ERROR'].map(value => [value, value])),
      },
      '@/services/storageService': { storageService: {
        saveMeeting: async () => ({ meeting_id: 'saved' }),
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
    // Home unmounts: its hook instance and refs are gone, the user is elsewhere.
    hooks.unmount();
    route.pathname = pathname;
    route.search = search ? `?${search}` : '';
    assert.equal(timers.length, 1, 'the navigation timer is armed');
    timers[0]();
    await new Promise(resolve => queueMicrotask(resolve));
    return { pushes, replaces, summaries, statuses, toasts };
  } finally {
    globalThis.window = previousWindow;
    globalThis.setTimeout = previousTimer;
  }
}

test('still on Home when the timer fires: the finished meeting opens with the summary request', async () => {
  for (const home of ['/', '/index.html']) {
    const result = await stopOn(home);
    assert.deepEqual(result.pushes, ['/meeting-details?id=saved&source=recording'], home);
    assert.deepEqual(result.summaries, [], 'the meeting page starts the summary after the visit');
  }
});

for (const page of ['/meetings', '/settings', '/settings/', '/settings.html', '/meeting-details?id=other']) {
  test(`stop started on Home, user left for ${page}: they stay put and the summary still starts`, async () => {
    const result = await stopOn(page);
    assert.deepEqual(result.pushes, [], 'no automatic navigation');
    assert.deepEqual(result.replaces, []);
    assert.deepEqual(result.summaries, ['saved']);
    assert.ok(result.statuses.includes('IDLE'), 'the recording state is reset');
    assert.equal(result.toasts[0].options.action.label, 'View meeting', 'the toast still offers the meeting');
  });
}

test('the user opened the saved meeting from the toast: the page takes over the summary, no second run', async () => {
  const result = await stopOn('/meeting-details?id=saved');
  assert.deepEqual(result.pushes, []);
  assert.deepEqual(result.replaces, ['/meeting-details?id=saved&source=recording']);
  assert.deepEqual(result.summaries, []);
});

test('with automatic summaries off nothing is generated away from Home', async () => {
  const result = await stopOn('/settings', { autoSummary: false });
  assert.deepEqual(result.pushes, []);
  assert.deepEqual(result.summaries, []);
});
