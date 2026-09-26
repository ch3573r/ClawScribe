import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

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
