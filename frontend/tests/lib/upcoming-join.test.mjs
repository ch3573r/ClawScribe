import test from 'node:test';
import assert from 'node:assert/strict';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

function entry(overrides = {}) {
  return { sourceId: null, label: 'Microsoft 365', allDay: false, event: {
    id: 'chosen', subject: 'Planning', start: new Date(Date.now() - 60_000).toISOString(),
    end: new Date(Date.now() + 600_000).toISOString(), joinUrl: 'https://teams.microsoft.com/meet/123456',
    attendees: [{ name: 'Invited', email: null }], ...overrides,
  } };
}

test('only timed, uncancelled invitations with a detectable Teams or Meet link offer a recording request', () => {
  const { supportsJoinRecording: eligible } = loadTsModule('src/hooks/useJoinRecording.ts');
  const chosen = entry();
  assert.equal(eligible(chosen), true);
  assert.equal(eligible({ ...chosen, sourceId: 'synthetic-calendar' }), true);
  assert.equal(eligible(entry({ joinUrl: 'https://meet.google.com/abc-defg-hij?authuser=0' })), true);
  for (const joinUrl of ['https://sub.meet.google.com/abc-defg-hij', 'https://meet.google.com/abc-defg-hij/extra',
    'http://meet.google.com/abc-defg-hij', 'https://meet.google.com:444/abc-defg-hij',
    'https://meet.google.com:443/abc-defg-hij', 'https://@meet.google.com/abc-defg-hij',
    'https://meet.google.com@invalid.example/abc-defg-hij', 'https://zoom.us/j/123456789']) {
    assert.equal(eligible(entry({ joinUrl })), false, joinUrl);
  }
  assert.equal(eligible(entry({ joinUrl: 'https://teams.cloud.microsoft/l/meetup-join/19%3ameeting_example%40thread.v2/0?context=example' })), true);
  for (const joinUrl of ['https://teams.microsoft.com/', 'https://teams.live.com/meet/',
    'https://teams.microsoft.com.example.org/meet/123', 'http://teams.microsoft.com/meet/123',
    'https://user@teams.microsoft.com/meet/123', 'https://teams.microsoft.com:444/meet/123']) {
    assert.equal(eligible(entry({ joinUrl })), false, joinUrl);
  }
  assert.equal(eligible(entry({ end: new Date(Date.now() - 1).toISOString() })), false);
  assert.equal(eligible(entry({ isCancelled: true })), false);
  assert.equal(eligible({ ...chosen, allDay: true }), false);
});

test('Next up offers prep for a recurring event and reuses the Teams recording guard', t => {
  const hooks = createHookHarness();
  t.after(hooks.unmount);
  const previousWindow = globalThis.window;
  globalThis.window = { setInterval: () => 1, clearInterval() {} };
  t.after(() => { globalThis.window = previousWindow; });
  const chosen = entry({ seriesId: 'series' });
  const joined = [];
  let windows = true;
  const jsx = (type, props) => ({ type, props });
  const { NextUpCard } = loadTsModule('src/components/Sidebar/NextUpCard.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx }, 'lucide-react': {},
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/settings-layout': { Meta: 'span' },
    '@/components/MeetingDetails/SeriesPreparation': { UpcomingSeriesPreparation: 'Brief' },
    'next/navigation': { usePathname: () => '/meetings' },
    '@/hooks/useUpcomingEvents': { useUpcomingEvents: () => ({
      entries: [chosen], isSelected: () => false, select() {},
    }) },
    '@/hooks/useJoinRecording': {
      supportsJoinRecording: loadTsModule('src/hooks/useJoinRecording.ts').supportsJoinRecording,
      useJoinRecording: () => ({ windows, pending: null, join: event => joined.push(event) }),
    },
  });
  const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props ? [node, ...nodes(node.props.children)] : [];
  const view = () => nodes(hooks.render(NextUpCard));
  const button = label => view().find(node => node.type === 'button' && node.props.children === label);
  button('Preparation brief').props.onClick();
  assert.equal(view().find(node => node.type === 'Brief').props.event.eventId, 'chosen');
  assert.equal(view().find(node => node.type === 'Brief').props.event.attendees.length, 0);
  button('Join and record').props.onClick();
  assert.deepEqual(joined, [chosen]);
  windows = false;
  assert.equal(button('Join and record'), undefined);
  windows = true;
  chosen.event.joinUrl = 'https://example.com/meet/123';
  assert.equal(button('Join and record'), undefined);
});

test('the call panel uses the selected appearance and answers only the displayed prompt', async t => {
  const hooks = createHookHarness();
  t.after(hooks.unmount);
  let scheduled;
  t.mock.method(globalThis, 'setTimeout', fn => { scheduled = fn; return 1; });
  t.mock.method(globalThis, 'clearTimeout', () => {});
  let pending = { provider: 'teams', key: 'active-prompt', eventTitle: 'Planning', retry: false };
  const replies = [], dismissals = [], themes = [], accents = [];
  const jsx = (type, props) => ({ type, props });
  const Prompt = loadTsModule('src/pages/call-recording-prompt.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
    'next/head': 'Head', '@/components/ui/button': { Button: 'button' },
    '@/lib/theme': {
      applyThemePreference: value => themes.push(value),
      applyAccent: value => accents.push(value),
      subscribeToSystemTheme: () => () => {}, DEFAULT_ACCENT_ID: 'claw',
    },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'recording_window_appearance') return { theme: 'dark', accent: 'violet' };
      if (command === 'get_pending_call_prompt') return pending;
      if (command === 'dismiss_call_detection_prompt') {
        dismissals.push(args.key);
        pending = null;
        return true;
      }
      assert.equal(command, 'confirm_call_detection_prompt');
      replies.push(args.key);
      pending = null;
    } },
  }).default;
  const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props ? [node, ...nodes(node.props.children)] : [];
  const button = label => nodes(hooks.render(Prompt)).find(node => node.type === 'button' && node.props.children === label);
  assert.equal(button('Record'), undefined);
  await flush();
  button('Record').props.onClick();
  assert.equal(themes.at(-1), 'dark');
  assert.equal(accents.at(-1), 'violet');
  assert.deepEqual(replies, ['active-prompt']);
  scheduled();
  await flush();
  assert.equal(button('Record'), undefined);

  pending = { provider: 'teams', key: 'next-prompt', eventTitle: null, retry: false };
  scheduled();
  await flush();
  const decline = button('Not now');
  assert.equal(decline.props.variant, 'outline');
  decline.props.onClick();
  decline.props.onClick();
  await flush();
  assert.deepEqual(dismissals, ['next-prompt']);
  assert.equal(button('Not now'), undefined);
  assert.deepEqual(replies, ['active-prompt']);
});

test('Upcoming opens plain Join independently and binds a double click to one unreviewed invitation', async t => {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const chosen = entry();
  const opened = [], joined = [], cancelled = [], confirmed = [];
  const launch = deferred(), delayedRead = deferred();
  let changed, pending = null, read = () => pending;
  const calendar = loadTsModule('src/lib/meetingCalendar.ts');
  const jsx = (type, props) => ({ type, props });
  const { UpcomingMeetings } = loadTsModule('src/components/UpcomingMeetings.tsx', {
    '@/components/MeetingDetails/SeriesPreparation': { UpcomingSeriesPreparation: 'PreparationBrief' },
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx }, 'lucide-react': {},
    '@/components/ui/button': { Button: 'button' },
    '@/lib/meetingCalendar': calendar,
    '@/hooks/usePlatform': { usePlatform: () => 'windows' },
    '@/hooks/useUpcomingEvents': { useUpcomingEvents: () => ({
      entries: [chosen], choice: { link: null }, isSelected: () => false, select: () => assert.fail('Unexpected manual selection'), hasCalendars: true,
    }) },
    '@/services/callDetectionService': { confirmCallPrompt: async key => confirmed.push(key) },
    '@/lib/openExternal': { openExternal: async url => opened.push(url) },
    '@tauri-apps/api/event': { listen: async (_name, receive) => { changed = receive; return () => {}; } },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'get_join_recording') return read();
      if (command === 'join_and_record') { joined.push(args); return launch.promise; }
      if (command === 'cancel_join_recording') { cancelled.push(args.id); pending = null; changed(); return true; }
      assert.fail(command);
    } },
  });
  const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props ? [node, ...nodes(node.props.children)] : [];
  const buttons = () => nodes(hooks.render(UpcomingMeetings)).filter(node => node.type === 'button');
  const button = name => buttons().find(node => [node.props.children].flat().includes(name));
  buttons();
  await flush();
  button('Join').props.onClick();
  assert.deepEqual(opened, [chosen.event.joinUrl]);
  assert.equal(joined.length, 0);
  const click = button('Join and record').props.onClick;
  click(); click();
  assert.equal(joined.length, 1);
  assert.equal(joined[0].link.eventId, 'chosen');
  assert.equal(joined[0].url, chosen.event.joinUrl);
  assert.equal(joined[0].link.attendees[0].attendance, 'unreviewed');
  assert.equal(joined[0].link.reviewed, undefined);
  pending = { id: 'first', subject: 'Planning', opening: true };
  changed(); await flush();
  assert.equal(button('Join and record').props.disabled, true);
  // Cancel while the native launcher is outstanding. Late reads cannot restore it.
  read = () => delayedRead.promise;
  changed(); read = () => pending;
  button('Cancel recording request').props.onClick();
  launch.resolve(); delayedRead.resolve({ id: 'first', opening: true });
  await flush();
  assert.deepEqual(cancelled, ['first']);
  assert.equal(button('Cancel recording request'), undefined);
  pending = { id: 'second', subject: 'Planning', opening: false, promptKey: 'second-observation' };
  changed(); await flush();
  button('Record this call').props.onClick();
  assert.deepEqual(confirmed, ['second-observation']);
});
