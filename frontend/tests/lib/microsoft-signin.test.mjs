import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

test('explicit consent failure retains the connected account and shows the admin message', async () => {
  const hooks = createHookHarness();
  let complete;
  const partial = { state: 'connected', userDisplayName: 'Test User', userEmail: null,
    missingScopes: ['Tasks.ReadWrite'], unavailableExports: ['Planner and Microsoft To Do export'], sessionOnly: false };
  let requests = 0;
  const { useMicrosoftExport } = loadTsModule('src/hooks/useMicrosoftExport.ts', {
    react: hooks.react,
    '@tauri-apps/api/event': { listen: async (_, callback) => { complete = callback; return () => {}; }, emit: async () => {} },
    '@/services/microsoftExportService': { microsoftExportService: {
      requestMissingPermissions: async () => { requests++; },
      connectionStatus: async () => partial,
    }, isOneNoteLargeLibraryError: () => false },
    '@/lib/meetingCalendar': { clearAllCalendarLinks: () => {} },
  });
  const render = () => hooks.render(useMicrosoftExport);
  render();
  await flush();
  await render().refreshStatus();
  await render().requestMissingPermissions();
  assert.equal(requests, 1);
  assert.equal(render().connection.state, 'connected');
  complete({ payload: { state: 'connected', error: 'Ask your administrator to approve: Tasks.ReadWrite' } });
  await flush();
  assert.equal(render().connection.state, 'connected');
  assert.equal(render().error, 'Ask your administrator to approve: Tasks.ReadWrite');
  assert.deepEqual(render().connection.missingScopes, ['Tasks.ReadWrite']);
});

test('Microsoft cancellation calls the backend and refreshes disconnected state', async () => {
  const hooks = createHookHarness();
  const calls = [];
  const { useMicrosoftExport } = loadTsModule('src/hooks/useMicrosoftExport.ts', {
    react: hooks.react,
    '@tauri-apps/api/event': { listen: async () => () => {}, emit: async () => {} },
    '@/services/microsoftExportService': { microsoftExportService: {
      signIn: async () => { calls.push('signIn'); },
      cancelSignIn: async () => { calls.push('cancel'); },
      connectionStatus: async () => ({ state: 'not_connected', userDisplayName: null, userEmail: null }),
    }, isOneNoteLargeLibraryError: () => false },
    '@/lib/meetingCalendar': { clearAllCalendarLinks: () => {} },
  });
  const render = () => hooks.render(useMicrosoftExport);
  await render().signIn();
  assert.equal(render().signingIn, true);
  await render().cancelSignIn();
  assert.deepEqual(calls, ['signIn', 'cancel']);
  assert.equal(render().signingIn, false);
  assert.equal(render().connection.state, 'not_connected');
  hooks.unmount();
});
