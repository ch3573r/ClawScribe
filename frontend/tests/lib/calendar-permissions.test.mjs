import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

test('upcoming meetings skip calendar requests on mount, timer, focus and visibility without permission', async () => {
  const previousWindow = globalThis.window, previousDocument = globalThis.document;
  const windowEvents = new Map(), documentEvents = new Map();
  let poll, removed = false, reads = 0;
  globalThis.window = {
    setInterval: fn => { poll = fn; return 1; }, clearInterval: () => { removed = true; },
    addEventListener: (name, fn) => windowEvents.set(name, fn),
    removeEventListener: name => windowEvents.delete(name),
  };
  globalThis.document = {
    visibilityState: 'visible',
    addEventListener: (name, fn) => documentEvents.set(name, fn),
    removeEventListener: name => documentEvents.delete(name),
  };
  const hooks = createHookHarness();
  let status = { state: 'connected', unavailableExports: ['Calendar lookup'] };
  try {
    const { UpcomingMeetings } = loadTsModule('src/components/UpcomingMeetings.tsx', {
      react: hooks.react,
      'lucide-react': {},
      '@/services/microsoftExportService': { microsoftExportService: {
        connectionStatus: async () => status,
        listCalendarEvents: async () => { reads++; return []; },
      } },
    });
    hooks.render(UpcomingMeetings);
    await flush();
    for (const run of [poll, windowEvents.get('focus'), documentEvents.get('visibilitychange')]) {
      run(); await flush();
    }
    assert.equal(reads, 0);
    status = { state: 'connected', unavailableExports: [] };
    windowEvents.get('focus')(); await flush();
    assert.equal(reads, 1, 'newly granted permission resumes calendar reads');
    status = { state: 'not_connected' };
    poll(); await flush();
    assert.equal(reads, 1);
    hooks.unmount();
    assert.ok(removed);
    assert.equal(windowEvents.size, 0);
    assert.equal(documentEvents.size, 0);
  } finally {
    hooks.unmount();
    globalThis.window = previousWindow;
    globalThis.document = previousDocument;
  }
});

test('settings calendar skips both event and current-meeting calls until permission is available', async () => {
  const hooks = createHookHarness();
  let status = { state: 'connected', unavailableExports: ['Calendar lookup'] };
  const calls = [];
  const { useMicrosoftExport } = loadTsModule('src/hooks/useMicrosoftExport.ts', {
    react: hooks.react,
    '@tauri-apps/api/event': { listen: async () => () => {}, emit: async () => {} },
    '@/services/microsoftExportService': { microsoftExportService: {
      connectionStatus: async () => status,
      listCalendarEvents: async () => { calls.push('events'); return []; },
      currentOrNextMeeting: async () => { calls.push('current'); return null; },
    }, isOneNoteLargeLibraryError: () => false },
    '@/lib/meetingCalendar': { clearAllCalendarLinks: () => {} },
  });
  const render = () => hooks.render(useMicrosoftExport);
  for (let attempt = 0; attempt < 3; attempt++) await render().loadCalendar();
  assert.deepEqual(calls, []);
  assert.equal(render().loadingCalendar, false);
  status = { state: 'connected', unavailableExports: [] };
  await render().loadCalendar();
  assert.deepEqual(calls, ['events', 'current']);
  hooks.unmount();
});
