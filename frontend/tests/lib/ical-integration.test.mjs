import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

function storage() {
  const values = new Map();
  return {
    get length() { return values.size; },
    key: index => [...values.keys()][index],
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key),
  };
}

test('Microsoft sign-out preserves imported appointments and clears legacy Microsoft bindings', t => {
  const before = globalThis.window;
  const win = Object.assign(new EventTarget(), { localStorage: storage(), sessionStorage: storage() });
  globalThis.window = win;
  t.after(() => { globalThis.window = before; });
  const links = loadTsModule('src/lib/meetingCalendar.ts');
  const ical = { eventId: 'ics:1', subject: 'Planning', attendees: [], source: 'ical', sourceId: 'a' };
  const legacy = { eventId: 'graph', subject: 'Review', attendees: [] };
  let changes = 0;
  win.addEventListener('clawscribe-pending-calendar-changed', () => changes++);
  links.setPendingCalendar(ical);
  links.setActiveRecordingCalendar({ sessionId: 'active', revision: 1, link: ical, matches: [], status: 'none' });
  const microsoft = links.parseCalendarLink(JSON.stringify(legacy));
  links.setActiveRecordingCalendar({ sessionId: 'microsoft', revision: 1, link: microsoft, matches: [microsoft, ical], status: 'matched' });
  win.localStorage.setItem(links.LEGACY_MEETING_CALENDAR_PREFIX + 'new', JSON.stringify(ical));
  win.localStorage.setItem(links.LEGACY_MEETING_CALENDAR_PREFIX + 'old', JSON.stringify(legacy));
  links.clearMicrosoftCalendarLinks();
  assert.equal(win.localStorage.getItem(links.LEGACY_MEETING_CALENDAR_PREFIX + 'old'), null);
  assert.equal(links.parseCalendarLink(win.localStorage.getItem(links.LEGACY_MEETING_CALENDAR_PREFIX + 'new')).sourceId, 'a');
  assert.equal(links.getActiveRecordingCalendar('active').link.source, 'ical');
  assert.equal(links.getActiveRecordingCalendar('microsoft').link, null);
  assert.deepEqual(Array.from(links.getActiveRecordingCalendar('microsoft').matches), [ical]);
  assert.equal(links.getPendingCalendar().sourceId, 'a');
  assert.equal(changes, 1);
});

const calendar = { id: 'file-id', name: 'Team', kind: 'file' };
const subscription = { id: 'feed-id', name: 'Other', kind: 'subscription' };
function service(invoke, parser = async (_text, sourceId) => [{
  id: 'instance-' + sourceId, subject: 'Planning', start: new Date(Date.now() + 3_600_000).toISOString(),
  end: new Date(Date.now() + 7_200_000).toISOString(), allDay: false, attendees: [],
}]) {
  return loadTsModule('src/services/icalService.ts', {
    '@tauri-apps/api/core': { invoke },
    '@/lib/icalWorkerClient': { CalendarReadError: class extends Error {}, parseCalendarInWorker: parser },
    '@/lib/meetingCalendar': { getPendingCalendar: () => null },
  });
}

test('concurrent refreshes share one read and a failed subscription leaves imported meetings usable', async () => {
  const calls = [];
  const api = service(async (name, args) => {
    calls.push(name);
    if (name === 'list_ical_sources') return [calendar, subscription];
    if (args.id === subscription.id) throw new Error('private feed detail');
    return 'calendar fixture';
  });
  await Promise.all([api.refreshICalendars(), api.refreshICalendars()]);
  await api.refreshICalendars();
  const state = api.getICalendarSnapshot();
  assert.equal(calls.filter(c => c === 'read_ical_source').length, 2);
  assert.equal(state.events.length, 1);
  assert.equal(state.events[0].sourceId, calendar.id);
  assert.ok(state.issues[subscription.id]);
  assert.doesNotMatch(JSON.stringify(state), /private feed detail/);
});

test('removing a source discards a late refresh response', async () => {
  let sources = [calendar];
  const entered = deferred(), read = deferred();
  const api = service(async (name) => {
    if (name === 'list_ical_sources') return [...sources];
    if (name === 'remove_ical_source') { sources = []; return; }
    entered.resolve(); return read.promise;
  });
  const refresh = api.refreshICalendars();
  await entered.promise;
  await api.removeICalendarSource(calendar.id);
  read.resolve('old data');
  await refresh;
  assert.equal(api.getICalendarSnapshot().events.length, 0);
  assert.equal(api.getICalendarSnapshot().sources.length, 0);
});

test('imported meetings are available while subscriptions are still downloading', async () => {
  const slow = deferred();
  const feeds = ['a', 'b', 'c'].map(id => ({ ...subscription, id }));
  const api = service(async (name, args) => {
    if (name === 'list_ical_sources') return [...feeds, calendar];
    return args.id === calendar.id ? 'imported fixture' : slow.promise;
  });
  const refresh = api.refreshICalendars();
  try {
    await flush();
    assert.equal(api.getICalendarSnapshot().loading, true);
    assert.deepEqual(Array.from(api.getICalendarSnapshot().events, event => event.sourceId), [calendar.id]);
  } finally {
    slow.resolve('feed fixture');
    await refresh;
  }
});

test('oversized or unparsable files are rejected before they reach storage', async () => {
  let writes = 0;
  const api = service(async () => { writes++; }, async () => { throw Error('private file content'); });
  await assert.rejects(api.importICalendarFile({ name: 'large.ics', size: 2 ** 24 + 1 }), /16 MiB/);
  const bad = new File(['not a calendar'], 'bad.ics');
  await assert.rejects(api.importICalendarFile(bad),
    err => !String(err).includes('private file content'));
  assert.equal(writes, 0);
});

test('file import validates the bytes, saves the display name and refreshes the shared list', async () => {
  const writes = [];
  const api = service(async (name, args) => {
    if (name === 'add_ical_file') { writes.push(args); return calendar; }
    if (name === 'list_ical_sources') return [calendar];
    return 'example';
  });
  await api.importICalendarFile(new File(['example'], 'Team.ics'));
  await api.refreshICalendars();
  assert.equal(writes[0].name, 'Team');
  assert.equal(writes[0].content, 'example');
  assert.equal(api.getICalendarSnapshot().events[0].sourceId, calendar.id);
});

test('Home offers an imported event for the next recording with Microsoft disconnected', async t => {
  const before = globalThis.window, doc = globalThis.document;
  globalThis.window = Object.assign(new EventTarget(), { setInterval: () => 1, clearInterval() {} });
  globalThis.document = new EventTarget();
  const hooks = createHookHarness();
  t.after(() => { hooks.unmount(); globalThis.window = before; globalThis.document = doc; });
  const jsx = (type, props) => ({ type, props });
  const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props ? [node, ...nodes(node.props.children)] : [];
  const selected = [];
  const event = { id: 'ics:instance', subject: 'Design review', sourceId: calendar.id, allDay: false,
    start: new Date(Date.now() + 3_600_000).toISOString(), end: new Date(Date.now() + 7_200_000).toISOString(),
    attendees: [{ name: 'Alex', email: null }] };
  const { UpcomingMeetings } = loadTsModule('src/components/UpcomingMeetings.tsx', {
    '@/components/MeetingDetails/SeriesPreparation': { UpcomingSeriesPreparation: 'PreparationBrief' },
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx }, 'lucide-react': {}, sonner: { toast: { error() {} } },
    '@/components/ui/button': { Button: 'button' }, '@/lib/openExternal': {},
    '@/hooks/useICalendar': { useICalendar: () => ({ events: [event], sources: [calendar] }) },
    '@/hooks/usePendingCalendar': { useNextCalendarChoice: () => ({ link: null }) },
    '@/lib/meetingCalendar': { ...loadTsModule('src/lib/meetingCalendar.ts'), setPendingCalendar: link => selected.push(link) },
    '@/services/microsoftExportService': { microsoftExportService: { connectionStatus: async () => ({ state: 'not_connected' }) } },
  });
  hooks.render(UpcomingMeetings);
  await flush();
  const buttons = nodes(hooks.render(UpcomingMeetings)).filter(node => node.type === 'button');
  assert.equal(buttons.length, 1);
  buttons[0].props.onClick();
  assert.equal(selected[0].sourceId, calendar.id);
  assert.equal(selected[0].subject, 'Design review');
  assert.equal(selected[0].attendees[0].attendance, 'unreviewed');
});
