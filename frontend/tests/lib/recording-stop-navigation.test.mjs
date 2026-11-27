import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

// Hold the summary preference lookup while the user changes route or starts
// recording again. The completed meeting already exists in the native library.
async function stopOn(location, { autoSummary = true, newRecording = false, failed = false } = {}) {
  const [pathname, search = ''] = location.split('?');
  const previousWindow = globalThis.window;
  const route = { pathname: '/', search: '' };
  globalThis.window = { location: route };
  const hooks = createHookHarness();
  const pushes = [];
  const replaces = [];
  const summaries = [];
  const cache = [];
  const toasts = [];
  const active = [];
  const calendars = [];
  let clearCount = 0;
  let listener;
  const subscribeStopped = callback => { listener = callback; return () => {}; };
  const preference = deferred();
  const noop = () => {};
  const asyncNoop = async () => {};
  try {
    const { useRecordingStop } = loadTsModule('src/hooks/useRecordingStop.ts', {
      react: hooks.react,
      '@tauri-apps/api/core': { invoke: async (command, args) => {
        if (command === 'start_recording_summary') summaries.push(args.meetingId);
        else assert.fail(`Unexpected recording-stop command ${command}`);
      } },
      'next/navigation': { useRouter: () => ({ push: path => pushes.push(path), replace: path => replaces.push(path) }) },
      sonner: { toast: Object.fromEntries(['success', 'warning', 'error'].map(kind => [
        kind, (title, options) => toasts.push({ kind, title, options }),
      ])) },
      '@/contexts/TranscriptContext': { useTranscripts: () => ({
        transcriptsRef: { current: [] }, flushBuffer: noop, clearTranscripts: () => { clearCount++; },
        markMeetingAsSaved: async id => cache.push(id),
      }) },
      '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({
        refetchMeetings: asyncNoop, setIsMeetingActive: value => active.push(value), startSummaryPolling: noop,
      }) },
      '@/contexts/ConfigContext': { useConfig: () => ({ modelConfig: { provider: 'synthetic', model: 'synthetic' }, isAutoSummary: autoSummary }) },
      '@/lib/auto-summary': { startAutomaticSummary: async request => { summaries.push(request.meetingId); return true; } },
      '@/contexts/RecordingStateContext': { useRecordingState: () => ({ subscribeStopped }) },
      '@/lib/meetingCalendar': { clearActiveRecordingCalendar: id => calendars.push(id) },
      '@/lib/summary-language-preferences': { detectAndCacheSummaryLanguage: () => preference.promise },
    });
    hooks.render(() => useRecordingStop());
    let ownsCompletion = true;
    listener({
      meeting_id: failed ? null : 'saved', recording_id: 'stopped-recording', session_id: 'stopped-session', transcript_count: 7,
      message: 'Open Interrupted recordings to recover the meeting.',
    }, () => ownsCompletion);
    route.pathname = pathname;
    route.search = search ? `?${search}` : '';
    if (newRecording) {
      ownsCompletion = false;
      active.push(true);
    }
    preference.resolve(true);
    await flush();
    return { pushes, replaces, summaries, cache, calendars, clearCount, active, toasts };
  } finally {
    hooks.unmount();
    globalThis.window = previousWindow;
  }
}

test('still on Home when post-processing finishes: the finished meeting opens', async () => {
  for (const home of ['/', '/index.html']) {
    const result = await stopOn(home);
    assert.deepEqual(result.pushes, ['/meeting-details?id=saved&source=recording'], home);
    assert.deepEqual(result.summaries, []);
  }
});

for (const page of ['/meetings', '/settings', '/settings/', '/settings.html', '/meeting-details?id=other']) {
  test(`stop started on Home, user left for ${page}: they stay put and native Stop owns summary dispatch`, async () => {
    const result = await stopOn(page);
    assert.deepEqual(result.pushes, [], 'no automatic navigation');
    assert.deepEqual(result.replaces, []);
    assert.deepEqual(result.summaries, []);
    assert.equal(result.toasts[0].options.action.label, 'View meeting', 'the toast still offers the meeting');
  });
}

test('the user opened the saved meeting from the toast: preserve that route without a second run', async () => {
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

test('a previous Stop cannot clear the replacement recording or navigate away from it', async () => {
  const result = await stopOn('/', { newRecording: true });
  assert.deepEqual(result.active, [false, true]);
  assert.deepEqual(result.cache, ['stopped-recording']);
  assert.deepEqual(result.calendars, ['stopped-session']);
  assert.equal(result.clearCount, 1);
  assert.deepEqual(result.pushes, []);
  assert.deepEqual(result.replaces, []);
  assert.deepEqual(result.summaries, []);
});

test('a library failure leaves the recovery cache and directs the user to recovery', async () => {
  const result = await stopOn('/', { failed: true });
  assert.equal(result.toasts[0].kind, 'error');
  assert.match(result.toasts[0].options.description, /Interrupted recordings/);
  assert.deepEqual(result.cache, []);
  assert.equal(result.clearCount, 0);
});
