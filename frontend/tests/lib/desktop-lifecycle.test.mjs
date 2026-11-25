import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props ? [node, ...nodes(node.props.children)] : [];
const components = (...names) => Object.fromEntries(names.map(name => [name, name]));
const noop = () => {};
const primitives = {
  'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
  '@tauri-apps/api/core': { invoke: async () => {} },
  'lucide-react': components('Play', 'Pause', 'Square', 'Mic', 'AlertCircle', 'X', 'PictureInPicture2'),
  sonner: { toast: { success: noop, error: noop } },
  '@/components/ui/button': components('Button'),
  '@/components/ui/tooltip': components('Tooltip', 'TooltipContent', 'TooltipProvider', 'TooltipTrigger'),
  '@/components/ui/alert': components('Alert', 'AlertDescription', 'AlertTitle'),
  '@/lib/analytics': { trackPageView: noop },
};

function withListeners(t, lateEvent) {
  const hooks = createHookHarness();
  const waiting = deferred();
  const registered = [];
  const unregistered = [];
  const listener = (name, callback) => {
    registered.push({ name, callback });
    const dispose = () => unregistered.push(name);
    return name === lateEvent ? waiting.promise.then(() => dispose) : Promise.resolve(dispose);
  };
  const mocks = { ...primitives, react: hooks.react, '@tauri-apps/api/event': { listen: listener } };
  t.after(() => hooks.unmount());
  return { ...hooks, mocks, registered, unregistered, waiting };
}

test('leaving modal state unregisters a model download listener that resolves late', async t => {
  const event = 'model-download-complete';
  const app = withListeners(t, event);
  const { useModalState } = loadTsModule('src/hooks/useModalState', app.mocks);
  app.render(() => useModalState({ provider: 'localWhisper', model: 'fixture' }));
  await flush();
  app.reset();
  app.waiting.resolve();
  await flush();
  assert.ok(app.unregistered.includes(event));
});

test('recording controls label Stop and release both registrations when unmounted during setup', async t => {
  const app = withListeners(t, 'transcription-error');
  const { RecordingControls } = loadTsModule('src/components/RecordingControls', {
    ...app.mocks,
    '@/contexts/RecordingStateContext': { RecordingStatus: {}, useRecordingState: () => ({ status: 'recording' }) },
    '@/components/MeetingDetails/MeetingBookmarks': components('LiveBookmarkButton'),
    '@/components/LiveRecordingNotes': components('LiveRecordingNotes'),
    '@/components/RecordingLevelMeter': components('RecordingLevelMeter'),
  });
  const tree = app.render(() => RecordingControls({ isRecording: true, variant: 'floating' }));
  await flush();
  app.reset();
  assert.deepEqual(app.unregistered, ['transcript-error']);
  app.waiting.resolve();
  await flush();
  assert.deepEqual(app.unregistered, ['transcript-error', 'transcription-error']);
  assert.ok(nodes(tree).some(node => node.type === 'Button' && node.props['aria-label'] === 'Stop recording'));
});

test('Ollama removes listeners registered before and after disposal without setting up another one', async t => {
  const ordered = ['ollama-model-download-progress', 'ollama-model-download-complete'];
  const app = withListeners(t, ordered[1]);
  const { OllamaDownloadProvider } = loadTsModule('src/contexts/OllamaDownloadContext', app.mocks);
  app.render(() => OllamaDownloadProvider({}));
  await flush();
  assert.deepEqual(app.registered.map(({ name }) => name), ordered);
  app.reset();
  assert.deepEqual(app.unregistered, ordered.slice(0, 1));
  app.waiting.resolve();
  await flush();
  assert.deepEqual(app.unregistered, ordered);
  assert.deepEqual(app.registered.map(({ name }) => name), ordered);
});

test('the onboarding container starts below the title bar', () => {
  const { OnboardingContainer } = loadTsModule('src/components/onboarding/OnboardingContainer', {
    ...primitives,
    './shared/ProgressIndicator': components('ProgressIndicator'),
    '@/contexts/OnboardingContext': { useOnboarding: () => ({}) },
  });
  assert.match(OnboardingContainer({ title: 'Setup' }).props.className, /\btop-\[var\(--titlebar-height\)\]/);
});

test('Home keeps recovery open for the next interrupted recording and clears its session flag after the last', async t => {
  const hooks = createHookHarness();
  const previousStorage = globalThis.sessionStorage;
  const storage = new Map();
  Object.defineProperty(globalThis, 'sessionStorage', { configurable: true, writable: true, value: {
    getItem: key => storage.get(key), removeItem: key => storage.delete(key), setItem: (key, value) => storage.set(key, value),
  } });
  t.after(() => { hooks.unmount(); globalThis.sessionStorage = previousStorage; });
  const timeouts = [];
  t.mock.method(globalThis, 'setTimeout', callback => { timeouts.push(callback); });
  let recoverableMeetings = [{ meetingId: 'first' }, { meetingId: 'second' }];
  const check = async () => {};
  const routes = [];
  const { default: Home } = loadTsModule('src/app/page.tsx', {
    ...primitives, react: hooks.react, 'framer-motion': { motion: { div: 'div' } },
    'next/navigation': { useRouter: () => ({ push: route => routes.push(route) }) },
    './_components/SettingsModal': components('SettingsModals'),
    './_components/TranscriptPanel': components('TranscriptPanel'),
    '@/app/_components/StatusOverlays': components('StatusOverlays'),
    '@/components/HomeDashboard': components('HomeDashboard'),
    '@/components/LiveAssistancePanel': components('LiveAssistancePanel'),
    '@/components/RecordingControls': components('RecordingControls'),
    '@/components/TranscriptRecovery': components('TranscriptRecovery'),
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ refetchMeetings: async () => {} }) },
    '@/components/ui/dialog': components('Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle'),
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ status: 'idle' }), RecordingStatus: {} },
    '@/contexts/TranscriptContext': { useTranscripts: () => ({ transcripts: [] }) },
    '@/contexts/ConfigContext': { useConfig: () => ({ transcriptModelConfig: {} }) },
    '@/hooks/usePermissionCheck': { usePermissionCheck: () => ({}) },
    '@/hooks/useModalState': { useModalState: () => ({ modelStartFailure: null }) },
    '@/hooks/useRecordingStart': { useRecordingStart: () => ({}) },
    '@/hooks/useLiveTranscriptionReadiness': { useLiveTranscriptionReadiness: () => ({}) },
    '@/hooks/useTranscriptRecovery': { useTranscriptRecovery: () => ({
      recoverableMeetings, checkForRecoverableTranscripts: check, recoverMeeting: async id => {
        recoverableMeetings = recoverableMeetings.filter(meeting => meeting.meetingId !== id);
        return { success: true, meetingId: id, remainingCount: recoverableMeetings.length };
      },
    }) },
  });
  const recovery = () => nodes(hooks.render(Home)).find(node => node.type === 'TranscriptRecovery').props;
  recovery();
  assert.equal(recovery().isOpen, true);
  await recovery().onRecover('first');
  assert.equal(recovery().isOpen, true);
  assert.equal(timeouts.length, 0);
  await recovery().onRecover('second');
  assert.equal(recovery().isOpen, false);
  assert.equal(storage.has('recovery_dialog_shown'), false);
  assert.equal(timeouts.length, 1);
  timeouts[0]();
  assert.deepEqual(routes, ['/meeting-details?id=second']);
});
