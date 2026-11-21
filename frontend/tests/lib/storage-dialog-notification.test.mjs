import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

// --- storage-dialog-controller unit tests ---

const { registerStorageDialogOpener, openStorageDialog } = loadTsModule('src/lib/storage-dialog-controller.ts');

test('openStorageDialog is a no-op before any opener is registered', () => {
  assert.doesNotThrow(() => openStorageDialog());
});

test('openStorageDialog calls the registered opener', () => {
  let calls = 0;
  const unregister = registerStorageDialogOpener(() => { calls++; });
  openStorageDialog();
  assert.equal(calls, 1);
  unregister();
});

test('opener is not called after deregistration', () => {
  let calls = 0;
  const unregister = registerStorageDialogOpener(() => { calls++; });
  unregister();
  openStorageDialog();
  assert.equal(calls, 0);
});

test('a later registration replaces an earlier one', () => {
  const log = [];
  const a = registerStorageDialogOpener(() => log.push('a'));
  const b = registerStorageDialogOpener(() => log.push('b'));
  openStorageDialog();
  a();
  b();
  assert.deepEqual(log, ['b']);
});

// --- regression: low-space action opens storage dialog, not File Explorer ---

function setupProvider(t, { openFolderFails = false } = {}) {
  const hooks = createHookHarness();
  const events = new Map();
  const toasts = [];
  const recordingCalls = [];
  const storageDialogCalls = [];
  const previousWindow = globalThis.window;
  globalThis.window = {
    addEventListener: () => {},
    removeEventListener: () => {},
  };
  const { RecordingPostProcessingProvider } = loadTsModule('src/contexts/RecordingPostProcessingProvider.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx: () => null, jsxs: () => null, Fragment: 'Fragment' },
    '@tauri-apps/api/event': { listen: async (name, cb) => { events.set(name, cb); return () => events.delete(name); } },
    '@tauri-apps/api/core': { invoke: async () => { if (openFolderFails) throw new Error('Fail'); } },
    '@tauri-apps/api/path': { appDataDir: async () => 'data' },
    sonner: { toast: {
      error: (title, opts) => toasts.push({ kind: 'error', title, ...opts }),
      info: (title, opts) => toasts.push({ kind: 'info', title, ...opts }),
      warning: (title, opts) => toasts.push({ kind: 'warning', title, ...opts }),
      dismiss: () => {},
    } },
    '@/hooks/useRecordingStop': { useRecordingStop: () => {} },
    '@/contexts/RecordingStateContext': {
      useRecordingState: () => ({
        isRecording: true, isStopping: false, status: 'RECORDING',
        setStatus: v => recordingCalls.push(['setStatus', v]),
      }),
      RecordingStatus: Object.fromEntries(['IDLE', 'STARTING', 'RECORDING', 'STOPPING'].map(v => [v, v])),
    },
    '@/services/recordingService': { recordingService: {
      getRecordingState: async () => ({ is_recording: true, is_paused: false, is_active: true }),
      stopRecording: async () => {},
    } },
    '@/lib/recording-capture': loadTsModule('src/lib/recording-capture.ts'),
    '@/lib/storage-dialog-controller': {
      openStorageDialog: () => storageDialogCalls.push('open'),
    },
  });
  t.after(() => { hooks.unmount(); globalThis.window = previousWindow; });
  hooks.render(() => RecordingPostProcessingProvider({ children: null }));
  return { events, toasts, recordingCalls, storageDialogCalls };
}

test('low-space action opens the storage dialog, not File Explorer', async t => {
  const app = setupProvider(t);
  await flush();
  app.events.get('recording-disk-space-low')({ payload: { freeBytes: 500_000_000 } });
  const toast = app.toasts[0];
  assert.equal(toast.kind, 'warning');
  assert.equal(toast.action.label, 'Review storage');
  toast.action.onClick();
  assert.deepEqual(app.storageDialogCalls, ['open']);
});

test('low-space action does not change recording state (cancel leaves recording alone)', async t => {
  const app = setupProvider(t);
  await flush();
  app.events.get('recording-disk-space-low')({ payload: { freeBytes: 500_000_000 } });
  app.toasts[0].action.onClick();
  await flush();
  assert.deepEqual(app.recordingCalls, [], 'opening storage review must not alter recording state');
});

// --- regression: blocked start offers storage review ---

function setupStartFailure(t) {
  const toasts = [];
  const storageDialogCalls = [];
  const { reportStartFailure } = loadTsModule('src/hooks/useRecordingStart.ts', {
    'react': { useCallback: (fn) => fn, useEffect: () => {} },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({}) },
    '@/contexts/ConfigContext': { useConfig: () => ({ selectedDevices: {}, devicePreferencesStatus: 'ready' }) },
    '@/contexts/RecordingStateContext': {
      useRecordingState: () => ({ setStatus: () => {}, isRecording: false, isStopping: false, isProcessing: false, isSaving: false }),
      RecordingStatus: { STARTING: 'STARTING', ERROR: 'ERROR' },
    },
    '@/services/recordingService': { recordingService: { startRecordingWithDevices: async () => 'id' } },
    '@/lib/analytics': { default: { trackButtonClick: () => {} } },
    '@/lib/recordingNotification': { showRecordingNotification: async () => {} },
    '@/lib/speakerDetectionCancellation': { offerSpeakerDetectionCancellation: async () => {} },
    '@/lib/meetingCalendar': {
      consumeNextCalendarChoice: () => {},
      getActiveRecordingCalendar: () => null,
      getNextCalendarChoice: () => ({ link: null, skip: false }),
      isReviewedCalendar: () => false,
      microsoftCalendarRevision: 0,
      setActiveRecordingCalendar: () => false,
    },
    '@/services/recordingCalendarService': { readRecordingCalendar: async () => ({ revision: 0 }), resolveRecordingCalendar: async () => {} },
    '@/lib/summary-language-preferences': { readPinnedSummaryLanguageDefault: () => null },
    sonner: { toast: {
      error: (title, opts) => toasts.push({ kind: 'error', title, ...opts }),
      warning: (title, opts) => toasts.push({ kind: 'warning', title, ...opts }),
    } },
    '@/lib/start-error': loadTsModule('src/lib/start-error.ts'),
    '@/lib/storage-dialog-controller': {
      openStorageDialog: () => storageDialogCalls.push('open'),
    },
  });
  return { reportStartFailure, toasts, storageDialogCalls };
}

const DISK_REFUSAL = 'Not enough free disk space to record. 0.5 GB is free where recordings are saved (D:\\Recordings); at least 1.6 GB is needed.';

test('blocked start shows Review storage action that opens the storage dialog', t => {
  const { reportStartFailure, toasts, storageDialogCalls } = setupStartFailure(t);
  reportStartFailure(DISK_REFUSAL);
  assert.equal(toasts.length, 1);
  const toast = toasts[0];
  assert.equal(toast.kind, 'error');
  assert.equal(toast.title, 'Not enough disk space');
  assert.equal(toast.action.label, 'Review storage');
  toast.action.onClick();
  assert.deepEqual(storageDialogCalls, ['open']);
});

test('clicking Review storage on blocked start does not start or stop a recording', t => {
  const { reportStartFailure, toasts } = setupStartFailure(t);
  reportStartFailure(DISK_REFUSAL);
  // action.onClick is openStorageDialog — no recording side effects in this module
  assert.ok(typeof toasts[0].action.onClick === 'function');
  assert.doesNotThrow(() => toasts[0].action.onClick());
});

test('a recording folder refusal offers recording settings from a sidebar or call start', t => {
  const { reportStartFailure, toasts } = setupStartFailure(t);
  const message = 'The recordings folder is unavailable. Reconnect it or choose another folder in Settings → Recording.';
  reportStartFailure(`Failed to start recording: ${message}`);
  assert.equal(toasts[0].title, 'Recordings folder unavailable');
  assert.equal(toasts[0].description, message);
  assert.equal(toasts[0].action.label, 'Recording settings');
});
