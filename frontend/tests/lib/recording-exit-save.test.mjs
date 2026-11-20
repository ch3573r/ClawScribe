import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

// Two hook instances stand for Home and the app-wide provider: they share the
// module-level guard, as in the app.
function setup(t, { folder, saveFails = false, slowSave = null, exitFails = false }) {
  const previousWindow = globalThis.window;
  const previousTimer = globalThis.setTimeout;
  const values = new Map([['last_recording_folder_path', folder], ['last_recording_meeting_name', 'Synthetic meeting']]);
  globalThis.window = { location: { pathname: '/settings', search: '' }, sessionStorage: {
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key),
  } };
  globalThis.setTimeout = callback => { queueMicrotask(callback); return 1; };
  t.after(() => { globalThis.window = previousWindow; globalThis.setTimeout = previousTimer; });
  const hooks = createHookHarness();
  const events = [];
  const noop = () => {};
  const asyncNoop = async () => {};
  const { useRecordingStop } = loadTsModule('src/hooks/useRecordingStop.ts', {
    react: hooks.react,
    'next/navigation': { useRouter: () => ({ push: noop, replace: noop }) },
    '@tauri-apps/api/core': { invoke: async command => {
      events.push(command);
      if (exitFails) throw new Error('Synthetic exit failure');
    } },
    '@tauri-apps/api/event': { listen: async () => noop },
    sonner: { toast: Object.assign(() => {}, { warning: noop, error: noop, success: noop }) },
    '@/contexts/TranscriptContext': { useTranscripts: () => ({
      transcriptsRef: { current: [] }, flushBuffer: noop, clearTranscripts: noop,
      meetingTitle: 'Synthetic', markMeetingAsSaved: asyncNoop,
    }) },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({
      refetchMeetings: asyncNoop, setCurrentMeeting: noop, setMeetings: noop,
      meetings: [], setIsMeetingActive: noop, startSummaryPolling: noop,
    }) },
    '@/contexts/ConfigContext': { useConfig: () => ({ modelConfig: {}, isAutoSummary: false }) },
    '@/lib/auto-summary': { startAutomaticSummary: asyncNoop },
    '@/contexts/RecordingStateContext': {
      useRecordingState: () => ({ status: 'STOPPING', setStatus: noop }),
      RecordingStatus: Object.fromEntries(['STOPPING', 'PROCESSING_TRANSCRIPTS', 'SAVING', 'COMPLETED', 'IDLE', 'ERROR'].map(value => [value, value])),
    },
    '@/services/storageService': { storageService: {
      saveMeeting: async () => {
        events.push('save-start');
        if (slowSave) await slowSave.promise;
        if (saveFails) throw new Error('Synthetic save failure');
        events.push('save-end');
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
  t.after(() => hooks.unmount());
  const [home, provider] = hooks.render(() => [useRecordingStop(noop, noop), useRecordingStop(noop, noop)]);
  return { home, provider, events };
}

const confirmations = events => events.filter(event => event === 'recording_exit_save_finished').length;

test('the save is confirmed once, after the library row is written', async t => {
  const { home, events } = setup(t, { folder: 'exit-success' });
  await home.handleRecordingStop(true);
  assert.equal(confirmations(events), 1);
  assert.ok(events.indexOf('recording_exit_save_finished') > events.indexOf('save-end'));
});

test('a failed save is confirmed too, so a quit does not wait for the grace period', async t => {
  const { home, events } = setup(t, { folder: 'exit-failure', saveFails: true });
  await home.handleRecordingStop(true);
  assert.equal(confirmations(events), 1);
});

test('a stop with nothing to save is confirmed', async t => {
  const { home, events } = setup(t, { folder: 'exit-nothing' });
  await home.handleRecordingStop(false);
  assert.equal(confirmations(events), 1);
  assert.ok(!events.includes('save-start'));
});

test('a repeat stop for a folder that is already saved confirms again, because nothing else will', async t => {
  const { home, provider, events } = setup(t, { folder: 'exit-saved' });
  await home.handleRecordingStop(true);
  await provider.handleRecordingStop(true);
  assert.equal(events.filter(event => event === 'save-start').length, 1, 'saved only once');
  assert.equal(confirmations(events), 2);
});

test('a stop that finds another save in progress does not confirm; the owner does, once', async t => {
  let release;
  const slowSave = { promise: new Promise(resolve => { release = resolve; }) };
  const { home, provider, events } = setup(t, { folder: 'exit-race', slowSave });
  const owner = home.handleRecordingStop(true);
  await new Promise(resolve => setImmediate(resolve));
  assert.ok(events.includes('save-start'), 'the first flow owns the save and is writing it');
  await provider.handleRecordingStop(true);
  assert.equal(confirmations(events), 0, 'the early return confirms nothing while the row is still being written');
  release();
  await owner;
  assert.equal(confirmations(events), 1);
  assert.ok(events.indexOf('recording_exit_save_finished') > events.indexOf('save-end'));
});

test('two stops that start together confirm exactly once', async t => {
  const { home, provider, events } = setup(t, { folder: 'exit-together' });
  await Promise.all([home.handleRecordingStop(true), provider.handleRecordingStop(true)]);
  assert.equal(events.filter(event => event === 'save-start').length, 1);
  assert.equal(confirmations(events), 1);
});

test('an error from the confirmation is ignored', async t => {
  const { home, events } = setup(t, { folder: 'exit-error', exitFails: true });
  await home.handleRecordingStop(true);
  assert.equal(confirmations(events), 1);
});
