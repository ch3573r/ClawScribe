import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

for (const failure of ['status', 'subscription']) {
  test(`library save uses the backend snapshot after a ${failure} failure`, async () => {
    const previousWindow = globalThis.window;
    const previousTimer = globalThis.setTimeout;
    const values = new Map([
      ['last_recording_folder_path', 'synthetic-meeting'],
      ['last_recording_meeting_name', 'Synthetic meeting'],
    ]);
    globalThis.window = { sessionStorage: {
      getItem: key => values.get(key) ?? null,
      setItem: (key, value) => values.set(key, value),
      removeItem: key => values.delete(key),
    } };
    globalThis.setTimeout = callback => { queueMicrotask(callback); return 1; };
    const hooks = createHookHarness();
    const saves = [];
    const statuses = [];
    const notices = [];
    const noop = () => {};
    const asyncNoop = async () => {};
    try {
      const { useRecordingStop } = loadTsModule('src/hooks/useRecordingStop.ts', {
        react: hooks.react,
        'next/navigation': { useRouter: () => ({ push: noop }) },
        '@tauri-apps/api/event': { listen: async name => {
          if (name === 'transcription-complete' && failure === 'subscription') throw new Error('Synthetic subscription failure');
          return noop;
        } },
        sonner: { toast: Object.fromEntries(['warning', 'success', 'error'].map(kind => [kind, title => notices.push({ kind, title })])) },
        '@/contexts/TranscriptContext': { useTranscripts: () => ({
          transcriptsRef: { current: [] }, flushBuffer: noop, clearTranscripts: noop,
          meetingTitle: 'Synthetic', markMeetingAsSaved: asyncNoop,
        }) },
        '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({
          refetchMeetings: asyncNoop, setCurrentMeeting: noop, setMeetings: noop,
          meetings: [], setIsMeetingActive: noop,
        }) },
        '@/contexts/RecordingStateContext': {
          useRecordingState: () => ({ status: 'STOPPING', setStatus: value => statuses.push(value) }),
          RecordingStatus: Object.fromEntries(['STOPPING', 'PROCESSING_TRANSCRIPTS', 'SAVING', 'COMPLETED', 'IDLE', 'ERROR'].map(value => [value, value])),
        },
        '@/services/storageService': { storageService: {
          saveMeeting: async (...args) => { saves.push(args); return { meeting_id: 'saved' }; },
          getMeeting: async () => ({ title: 'Synthetic meeting' }),
        } },
        '@/services/transcriptService': { transcriptService: { getTranscriptionStatus: async () => {
          if (failure === 'status') throw new Error('Synthetic status failure');
          return { is_processing: false, chunks_in_queue: 0 };
        } } },
        '@/lib/analytics': { __esModule: true, default: new Proxy({}, { get: () => asyncNoop }) },
        '@/lib/meetingCalendar': { takeActiveRecordingCalendar: () => null, setMeetingCalendar: noop },
        '@/lib/summary-language-preferences': { applyPinnedSummaryLanguageToMeeting: async () => true, detectAndCacheSummaryLanguage: asyncNoop },
        '@tauri-apps/plugin-store': { Store: { load: async () => ({ get: async () => 0 }) } },
      });
      const hook = hooks.render(() => useRecordingStop(noop, noop));
      await hook.handleRecordingStop(true);
      assert.equal(saves.length, 1);
      assert.equal(saves[0][2], 'synthetic-meeting');
      assert.equal(saves[0][4], true, 'request backend transcript snapshot even if UI has no lines');
      assert.ok(statuses.includes('COMPLETED'));
      assert.ok(!statuses.includes('ERROR'));
      assert.ok(!notices.some(notice => notice.kind === 'error'));
      hooks.unmount();
    } finally {
      globalThis.window = previousWindow;
      globalThis.setTimeout = previousTimer;
    }
  });
}
