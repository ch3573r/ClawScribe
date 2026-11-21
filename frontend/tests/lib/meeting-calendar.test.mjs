import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const start = '2030-04-15T10:00:00Z';
const end = '2030-04-15T11:00:00Z';
const now = Date.parse('2030-04-15T10:15:00Z');
const event = (id, changes = {}) => ({
  id, subject: 'Planning', start, end, attendees: [{ name: 'Invitee', email: null }], ...changes,
});
const graphSeries = 'graph:v1:5b910e68a596c61027e658aaf8baa621706d748d0af2b1e2ea52d49a13c6f49f';

test('a fresh invitation and a legacy checked invitee both require attendance review', t => {
  const before = globalThis.window;
  const data = new Map([['clawscribe.pendingCalendar', JSON.stringify({
    eventId: 'previous', subject: null, start: new Date().toISOString(),
    attendees: [{ name: 'Checked by default', included: true }, { name: 'Excluded', included: false }],
  })]]);
  globalThis.window = { sessionStorage: {
    getItem: key => data.get(key) ?? null,
  } };
  t.after(() => { globalThis.window = before; });
  const links = loadTsModule('src/lib/meetingCalendar.ts');
  assert.deepEqual(Array.from(links.getPendingCalendar().attendees, person => person.attendance),
    ['unreviewed', 'absent']);
  assert.equal(links.calendarLinkFromEvent(event('next')).attendees[0].attendance, 'unreviewed');
});

test('automatic matching uses only a unique, timed event across sources', () => {
  const { matchOngoingCalendar } = loadTsModule('src/lib/meetingCalendar.ts');
  const matching = [event('live'), event('future', { start: end }), event('ended', { end: start }),
    event('holiday', { allDay: true }), event('cancelled', { isCancelled: true })];
  assert.equal(matchOngoingCalendar(matching, now, false).matches[0].eventId, 'live');
  assert.equal(matchOngoingCalendar(matching, Date.parse(end), false).matches.length, 0);
  const overlap = matchOngoingCalendar([...matching, event('another', { sourceId: 'file' })], now, false);
  assert.equal(overlap.matches.length, 2);
  assert.equal(overlap.status, 'ambiguous');
  assert.equal(matchOngoingCalendar(matching, now, true).status, 'unavailable');
});

test('an early join offers a nearby invitation for review and leaves competing invitations unlinked', () => {
  const { matchOngoingCalendar, EARLY_JOIN_WINDOW_MS } = loadTsModule('src/lib/meetingCalendar.ts');
  const early = event('early', { start: new Date(now + EARLY_JOIN_WINDOW_MS - 1).toISOString() });
  const far = event('far', { start: new Date(now + EARLY_JOIN_WINDOW_MS).toISOString() });
  const next = matchOngoingCalendar([early, far], now, false);
  assert.equal(next.status, 'upcoming');
  assert.equal(next.matches[0].eventId, 'early');
  assert.equal(next.matches[0].reviewed, undefined);
  assert.equal(next.matches[0].attendees[0].attendance, 'unreviewed');
  assert.equal(matchOngoingCalendar([early, event('running')], now, false).status, 'ambiguous');
  assert.equal(matchOngoingCalendar([early, { ...early, sourceId: 'second' }], now, false).status, 'ambiguous');
  assert.equal(matchOngoingCalendar([early], now, true).status, 'unavailable');
});

function withStorage(t) {
  const before = globalThis.window;
  const storage = () => {
    const data = new Map();
    return {
      get length() { return data.size; },
      key: index => Array.from(data.keys())[index],
      getItem: key => data.get(key) ?? null,
      setItem: (key, value) => data.set(key, value),
      removeItem: key => data.delete(key),
    };
  };
  globalThis.window = Object.assign(new EventTarget(), { sessionStorage: storage(), localStorage: storage() });
  t.after(() => { globalThis.window = before; });
  return loadTsModule('src/lib/meetingCalendar.ts');
}

test('Calendars settings forwards the Microsoft series when selecting an occurrence for recording', async t => {
  const links = withStorage(t);
  const hooks = createHookHarness();
  globalThis.window.setInterval = () => 1;
  globalThis.window.clearInterval = () => {};
  const previousDocument = globalThis.document;
  globalThis.document = new EventTarget();
  t.after(() => hooks.unmount());
  t.after(() => { globalThis.document = previousDocument; });
  const source = JSON.parse(readFileSync(new URL('../fixtures/microsoft-calendar-series.json', import.meta.url), 'utf8')).value;
  const { id, subject, attendees } = source[0];
  const graphEvent = event(id, {
    subject, seriesId: graphSeries,
    attendees: attendees.map(({ emailAddress }) => ({ name: emailAddress.name, email: emailAddress.address })),
  });
  const jsx = (type, props) => ({ type, props });
  const ui = names => Object.fromEntries(names.split(' ').map(name => [name, name]));
  const button = 'UseEventButton';
  const settings = loadTsModule('src/components/settings/CalendarsSettings.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx, jsxs: jsx }, 'lucide-react': {},
    '@/components/IntegrationIcons': {},
    '@/components/ICalendarSettings': { UseEventButton: button, formatWhen: () => 'Upcoming' },
    '@/components/ui/button': ui('Button'),
    '@/components/ui/dropdown-menu': ui('DropdownMenu DropdownMenuContent DropdownMenuItem DropdownMenuTrigger'),
    '@/components/ui/settings-layout': ui('SettingsCard SettingsColumns SettingsFooter SettingsRow StatusText'),
    '@/components/settings/settings-parts': ui('IconAction IconTile NoteRow'),
    '@/hooks/useICalendar': { useICalendar: () => ({ sources: [], events: [], issues: {} }) },
    '@/hooks/useMicrosoftExport': { useMicrosoftExport: () => ({
      connection: { state: 'connected' }, calendarEvents: [graphEvent], loadCalendar() {},
    }) },
    '@/hooks/usePendingCalendar': { usePendingCalendar: links.getPendingCalendar },
    '@/lib/meetingCalendar': links,
    '@/services/icalService': {},
    '@/services/microsoftExportService': { microsoftExportService: { listCalendarEvents: async () => [] } },
  });
  const { UseEventButton } = loadTsModule('src/components/ICalendarSettings.tsx', {
    'react/jsx-runtime': { jsx, jsxs: jsx }, 'lucide-react': {}, sonner: {},
    '@/lib/meetingCalendar': links, '@/services/icalService': {},
    '@/components/ui/button': ui('Button'), '@/components/ui/dialog': {},
    '@/components/ui/input': {}, '@/components/ui/tabs': {},
  });
  const findEventButton = node => {
    if (Array.isArray(node)) return node.map(findEventButton).find(Boolean);
    if (node?.type === button) return node;
    return node?.props && findEventButton(node.props.children);
  };
  const view = hooks.render(settings.CalendarsSettings);
  const control = findEventButton(view.props.aside);
  assert.ok(control);
  const click = UseEventButton(control.props).props.onClick;
  click();
  const pending = links.getPendingCalendar();
  assert.equal(pending.source, 'microsoft');
  assert.equal(pending.eventId, id);
  assert.equal(pending.seriesId, graphSeries);
  assert.equal(pending.attendees[0].attendance, 'unreviewed');
  links.clearMicrosoftCalendarLinks();
  click();
  assert.equal(links.getPendingCalendar(), null);
});

test('sign-out clears cached Microsoft series events and ignores an in-flight Home refresh', async t => {
  const links = withStorage(t);
  const hooks = createHookHarness();
  const previousDocument = globalThis.document;
  globalThis.document = new EventTarget();
  t.after(() => hooks.unmount());
  t.after(() => { globalThis.document = previousDocument; });
  let refresh;
  Object.assign(globalThis.window, {
    setInterval: run => { refresh = run; return 1; },
    clearInterval() {},
  });
  const when = {
    start: new Date(Date.now() + 60_000).toISOString(),
    end: new Date(Date.now() + 120_000).toISOString(),
  };
  const graphEvent = event('microsoft-series', { ...when, seriesId: graphSeries });
  const imported = event('ics:abc:456', { ...when, sourceId: 'file', seriesId: 'ics:abc' });
  const delayed = deferred();
  let reads = 0;
  const { useUpcomingEvents } = loadTsModule('src/hooks/useUpcomingEvents.ts', {
    react: hooks.react, sonner: {},
    '@/lib/meetingCalendar': links,
    '@/hooks/useICalendar': { useICalendar: () => ({ events: [imported], sources: [{ id: 'file', name: 'File' }] }) },
    '@/hooks/usePendingCalendar': { useNextCalendarChoice: links.getNextCalendarChoice },
    '@/services/microsoftExportService': { microsoftExportService: {
      connectionStatus: async () => ({ state: 'connected' }),
      listCalendarEvents: async () => (++reads === 1 ? [graphEvent] : delayed.promise),
    } },
  });
  const render = () => hooks.render(useUpcomingEvents);
  render();
  await flush();
  const before = render();
  const microsoft = before.entries.find(entry => entry.label === 'Microsoft 365');
  assert.ok(microsoft);
  before.select(microsoft);
  assert.equal(links.getPendingCalendar().seriesId, graphSeries);
  refresh();
  await flush();
  assert.equal(reads, 2);
  links.clearMicrosoftCalendarLinks();
  before.select(microsoft);
  assert.equal(links.getPendingCalendar(), null);
  assert.deepEqual(Array.from(render().entries, entry => entry.event.id), [imported.id]);
  delayed.resolve([graphEvent]);
  await flush();
  assert.deepEqual(Array.from(render().entries, entry => entry.event.id), [imported.id]);
});

test('starting one recording preserves an explicit choice made for the next recording during startup', t => {
  const links = withStorage(t);
  const first = links.calendarLinkFromEvent(event('first'));
  const second = links.calendarLinkFromEvent(event('second'));
  assert.ok(links.setPendingCalendar(first));
  const original = links.getNextCalendarChoice();
  assert.ok(links.setPendingCalendar(second));
  links.consumeNextCalendarChoice(original);
  assert.equal(links.getNextCalendarChoice().link.eventId, 'second');
  assert.ok(links.setPendingCalendar(null, true));
  const skip = links.getNextCalendarChoice();
  assert.equal(skip.skip, true);
  links.consumeNextCalendarChoice(skip);
  assert.equal(links.getNextCalendarChoice().skip, false);
});

test('a selection left in Upcoming cannot name a later tray or call recording', t => {
  const links = withStorage(t);
  const selected = links.calendarLinkFromEvent(event('earlier-meeting'));
  links.setPendingCalendar(selected, false, end);
  assert.equal(links.getNextCalendarChoice(Date.parse(end) + 60_000).link.eventId, 'earlier-meeting');
  assert.equal(links.getNextCalendarChoice(Date.parse(end) + 6 * 60_000).link, null);
  // A pending selection written by 0.64.1 only contained the link, without an end or expiry.
  window.sessionStorage.setItem('clawscribe.pendingCalendar', JSON.stringify(selected));
  assert.equal(links.getNextCalendarChoice(Date.parse(end) + 86_400_000).link, null);
  // An explicit opt-out carries no event data and still applies once across an upgrade.
  window.sessionStorage.setItem('clawscribe.pendingCalendar', '"skip"');
  const skip = links.getNextCalendarChoice();
  assert.equal(skip.skip, true);
  links.consumeNextCalendarChoice(skip);
  assert.equal(links.getNextCalendarChoice().skip, false);
});

test('a joined call keeps the saved devices and leaves a separate manual calendar choice for the next start', async t => {
  const links = withStorage(t);
  links.setPendingCalendar(links.calendarLinkFromEvent(event('manual-choice')));
  const joined = links.calendarLinkFromEvent(event('joined-event'));
  const calls = [];
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const noop = () => {};
  const { useStartRecordingSession } = loadTsModule('src/hooks/useRecordingStart.ts', {
    react: hooks.react, '@/lib/meetingCalendar': links,
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ setIsMeetingActive: noop }) },
    '@/contexts/ConfigContext': { useConfig: () => ({
      selectedDevices: { micDevice: 'Chosen input', systemDevice: 'Chosen output' }, devicePreferencesStatus: 'ready',
    }) },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ setStatus: noop }), RecordingStatus: { STARTING: 'starting' } },
    '@/services/recordingService': { recordingService: {
      startRecordingWithDevices: async (...args) => { calls.push(args); return 'bound-session'; },
    } },
    '@/services/recordingCalendarService': {
      readRecordingCalendar: async () => ({ link: joined, revision: 1 }),
      resolveRecordingCalendar: () => assert.fail('A joined call already has its own invitation'),
    },
    '@/lib/recordingNotification': { showRecordingNotification: noop },
    '@/lib/analytics': { __esModule: true, default: { trackButtonClick: noop } },
  });
  await hooks.render(useStartRecordingSession)('calendar_call', 'native-claim');
  await flush();
  assert.deepEqual(calls[0].slice(0, 2), ['Chosen input', 'Chosen output']);
  assert.equal(calls[0][3], undefined);
  assert.equal(calls[0][4], 'native-claim');
  assert.equal(links.getActiveRecordingCalendar('bound-session').link.eventId, 'joined-event');
  assert.equal(links.getPendingCalendar().eventId, 'manual-choice');
});

function legacyStore(t, outcomes, onCall = () => {}) {
  const links = withStorage(t);
  const store = globalThis.window.localStorage;
  const sent = [];
  const api = loadTsModule('src/lib/meetingCalendarStore.ts', {
    '@/lib/meetingCalendar': links,
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      onCall(command, args);
      if (command === 'migrate_legacy_meeting_calendars') {
        sent.push(args.entries);
        if (outcomes instanceof Error) throw outcomes;
        return args.entries.map(entry => ({ meetingId: entry.meetingId, outcome: outcomes[entry.meetingId] }));
      }
      return null;
    } },
  });
  return { links, store, sent, api, key: id => links.LEGACY_MEETING_CALENDAR_PREFIX + id };
}

test('one malformed legacy link is set aside without blocking reads, backup or the other entries', async t => {
  const outcomes = { good: 'migrated', rejected: 'invalid', busy: 'retry', locked: 'failed' };
  const { store, sent, api, key } = legacyStore(t, outcomes);
  store.setItem(key('good'), JSON.stringify({ eventId: 'ok', attendees: [{ name: 'Previously checked', included: true }] }));
  store.setItem(key('broken'), '{not json');
  store.setItem(key('rejected'), JSON.stringify({ eventId: 'two words' }));
  store.setItem(key('busy'), JSON.stringify({ eventId: 'later' }));
  store.setItem(key('locked'), JSON.stringify({ eventId: 'never' }));
  // The backup path awaits this; it must resolve however the entries fare.
  await api.migrateLegacyMeetingCalendars();
  assert.deepEqual(Array.from(sent[0], entry => entry.meetingId), ['good', 'rejected', 'busy', 'locked']);
  assert.equal(sent[0][0].link.attendees[0].attendance, 'unreviewed');
  assert.equal(store.getItem(key('good')), null);
  // Only the bad entries are set aside, and their data is kept.
  assert.equal(store.getItem(key('broken')), null);
  assert.equal(store.getItem(api.REJECTED_MEETING_CALENDAR_PREFIX + 'broken'), '{not json');
  assert.equal(store.getItem(key('rejected')), null);
  assert.ok(store.getItem(api.REJECTED_MEETING_CALENDAR_PREFIX + 'rejected'));
  // A transient or lasting failure keeps the original untouched.
  assert.ok(store.getItem(key('busy')));
  assert.ok(store.getItem(key('locked')));
  // Reads still work, and nothing is retried straight away.
  assert.equal(await api.getMeetingCalendar('good'), null);
  assert.equal(sent.length, 1);
});

test('only a transient failure is retried, and only after a pause', async t => {
  const outcomes = { busy: 'retry', locked: 'failed' };
  const { store, sent, api, key } = legacyStore(t, outcomes);
  let now = 1_000_000;
  t.mock.method(Date, 'now', () => now);
  store.setItem(key('busy'), JSON.stringify({ eventId: 'later' }));
  store.setItem(key('locked'), JSON.stringify({ eventId: 'never' }));
  await api.migrateLegacyMeetingCalendars();
  await api.migrateLegacyMeetingCalendars();
  assert.equal(sent.length, 1);
  now += 10_000;
  outcomes.busy = 'migrated';
  await api.migrateLegacyMeetingCalendars();
  assert.equal(sent.length, 2);
  // The lasting failure is sent again with the pass, but never discarded.
  assert.equal(store.getItem(key('busy')), null);
  assert.ok(store.getItem(key('locked')));
  await api.migrateLegacyMeetingCalendars();
  assert.equal(sent.length, 2);
});

test('a failing migration command keeps every entry and still resolves', async t => {
  const { store, api, key } = legacyStore(t, new Error('database is busy'));
  store.setItem(key('saved'), JSON.stringify({ eventId: 'legacy' }));
  await api.migrateLegacyMeetingCalendars();
  assert.ok(store.getItem(key('saved')));
  assert.equal(await api.getMeetingCalendar('saved'), null);
});

test('signing out keeps reviewed attendance and clears only what came from the account', t => {
  const links = withStorage(t);
  const store = globalThis.window.localStorage;
  const prefix = links.LEGACY_MEETING_CALENDAR_PREFIX;
  store.setItem(prefix + 'kept', JSON.stringify({ eventId: 'a', attendees: [{ name: 'Present', attendance: 'present' }] }));
  store.setItem(prefix + 'dropped', JSON.stringify({ eventId: 'b', attendees: [{ name: 'Default', included: true }] }));
  store.setItem(prefix + 'file', JSON.stringify({ source: 'ical', sourceId: 'calendar', eventId: 'c', attendees: [] }));
  const reviewed = { ...links.calendarLinkFromEvent(event('reviewed')), reviewed: true };
  const automatic = links.calendarLinkFromEvent(event('automatic'));
  const fromFile = links.calendarLinkFromEvent(event('file-event', { sourceId: 'calendar' }));
  links.setActiveRecordingCalendar({
    sessionId: 'active', revision: 1, link: reviewed, status: 'ambiguous', matches: [reviewed, automatic, fromFile],
  });
  links.clearMicrosoftCalendarLinks();
  const active = links.getActiveRecordingCalendar('active');
  assert.equal(active.link.eventId, 'reviewed');
  // Current-account candidates are transient; imported calendars are unaffected.
  assert.deepEqual(Array.from(active.matches, match => match.eventId), ['file-event']);
  assert.ok(store.getItem(prefix + 'kept'));
  assert.equal(store.getItem(prefix + 'dropped'), null);
  assert.ok(store.getItem(prefix + 'file'));
  links.setActiveRecordingCalendar({ sessionId: 'active', revision: 2, link: automatic, status: 'matched', matches: [automatic] });
  links.clearMicrosoftCalendarLinks();
  assert.equal(links.getActiveRecordingCalendar('active').link, null);
  assert.ok(links.setPendingCalendar(automatic));
  links.clearMicrosoftCalendarLinks();
  assert.equal(links.getPendingCalendar(), null);
  assert.ok(links.setPendingCalendar(reviewed));
  links.clearMicrosoftCalendarLinks();
  assert.equal(links.getPendingCalendar().eventId, 'reviewed');
});

test('a link counts as reviewed once the person has checked anyone, or saved it', t => {
  const links = withStorage(t);
  const automatic = links.calendarLinkFromEvent(event('automatic'));
  assert.equal(links.isReviewedCalendar(automatic), false);
  assert.equal(links.isReviewedCalendar({ ...automatic, reviewed: true }), true);
  const checked = { ...automatic, attendees: [{ name: 'Invitee', email: null, attendance: 'absent' }] };
  assert.equal(links.isReviewedCalendar(checked), true);
  assert.equal(links.parseCalendarLink(JSON.stringify({ ...automatic, reviewed: true })).reviewed, true);
  assert.equal('reviewed' in links.parseCalendarLink(JSON.stringify(automatic)), false);
});

test('recording lookup asks for a complete Graph result and avoids guessing when a source fails', async () => {
  const requests = [];
  const imported = event('file-event', { sourceId: 'file' });
  const api = loadTsModule('src/services/recordingCalendarService.ts', {
    '@/services/icalService': {
      refreshICalendars: async () => {},
      getICalendarSnapshot: () => ({ events: [imported], loading: false, issues: {} }),
    },
    '@/services/microsoftExportService': { microsoftExportService: {
      connectionStatus: async () => ({ state: 'connected' }),
      listCalendarEvents: async (...args) => { requests.push(args); throw Error('Calendar unavailable'); },
    } },
  });
  const lookup = await api.findOngoingRecordingCalendar(now);
  assert.deepEqual(requests, [['2030-04-15T10:14:00.000Z', '2030-04-15T10:20:00.000Z', true]]);
  assert.equal(lookup.status, 'unavailable');
  assert.equal(lookup.matches[0].eventId, 'file-event');
  assert.deepEqual(Array.from(lookup.unavailable), ['Microsoft 365']);
});

test('a saved recording can be linked afterwards, and a failing source is named', async () => {
  const during = event('during', { sourceId: 'file' });
  const before = event('before', { start: '2030-04-15T08:00:00Z', end: '2030-04-15T09:00:00Z', sourceId: 'file' });
  const api = loadTsModule('src/services/recordingCalendarService.ts', {
    '@/services/icalService': {
      refreshICalendars: async () => {},
      getICalendarSnapshot: () => ({ events: [during, before], loading: false, issues: {} }),
    },
    '@/services/microsoftExportService': { microsoftExportService: {
      connectionStatus: async () => ({ state: 'connected' }),
      listCalendarEvents: async () => { throw Error('Calendar unavailable'); },
    } },
  });
  const result = await api.findCalendarEventsBetween(Date.parse(start), Date.parse(end));
  assert.deepEqual(Array.from(result.matches, match => match.eventId), ['during']);
  assert.deepEqual(Array.from(result.unavailable), ['Microsoft 365']);
});

test('a disconnected Microsoft calendar cannot hold up an imported match', async () => {
  const imported = event('file-event', { sourceId: 'file' });
  const api = loadTsModule('src/services/recordingCalendarService.ts', {
    '@/services/icalService': {
      refreshICalendars: async () => {},
      getICalendarSnapshot: () => ({ events: [imported], issues: {} }),
    },
    '@/services/microsoftExportService': { microsoftExportService: {
      connectionStatus: async () => ({ state: 'not_connected' }),
      listCalendarEvents: () => assert.fail('unexpected Graph request'),
    } },
  });
  const lookup = await api.findOngoingRecordingCalendar(now);
  assert.equal(lookup.status, 'matched');
  assert.equal(lookup.matches[0].eventId, 'file-event');
});

test('a connected calendar with missing permission leaves other sources available for manual review', async () => {
  const api = loadTsModule('src/services/recordingCalendarService.ts', {
    '@/services/icalService': {
      refreshICalendars: async () => {},
      getICalendarSnapshot: () => ({ events: [event('local', { sourceId: 'file' })], issues: {} }),
    },
    '@/services/microsoftExportService': { microsoftExportService: {
      connectionStatus: async () => ({ state: 'connected', unavailableExports: ['Calendar lookup'] }),
      listCalendarEvents: () => assert.fail('unexpected Graph request'),
    } },
  });
  const result = await api.findOngoingRecordingCalendar(now);
  assert.equal(result.status, 'unavailable');
  assert.deepEqual(Array.from(result.unavailable), ['Microsoft 365']);
  assert.equal(result.matches[0].eventId, 'local');
});

test('an early calendar candidate reaches the active recording without writing a link', async t => {
  const links = withStorage(t);
  const initial = { sessionId: 'early-join', revision: 0, link: null, status: 'none', matches: [] };
  links.setActiveRecordingCalendar(initial);
  const writes = [];
  const api = loadTsModule('src/services/recordingCalendarService.ts', {
    '@/lib/meetingCalendar': links,
    '@tauri-apps/api/core': { invoke: async (...args) => { writes.push(args); return 1; } },
    '@/services/icalService': { refreshICalendars: async () => {}, getICalendarSnapshot: () => ({
      events: [event('nearby', { sourceId: 'file', start: new Date(now + 120_000).toISOString() })], issues: {},
    }) },
    '@/services/microsoftExportService': { microsoftExportService: { connectionStatus: async () => ({ state: 'not_connected' }) } },
  });
  await api.resolveRecordingCalendar(initial, now, links.microsoftCalendarRevision);
  const active = links.getActiveRecordingCalendar(initial.sessionId);
  assert.equal(active.status, 'upcoming');
  assert.equal(active.matches[0].eventId, 'nearby');
  assert.equal(active.link, null);
  assert.deepEqual(writes, []);
});

test('a stalled subscription stays within the recording lookup budget', async t => {
  const stalled = deferred();
  t.mock.method(globalThis, 'setTimeout', (callback, wait) => {
    assert.ok(wait < 2000);
    queueMicrotask(callback);
    return 1;
  });
  const api = loadTsModule('src/services/recordingCalendarService.ts', {
    '@/services/icalService': {
      refreshICalendars: () => stalled.promise,
      getICalendarSnapshot: () => ({ events: [], loading: true, issues: {} }),
    },
    '@/services/microsoftExportService': { microsoftExportService: {
      connectionStatus: async () => ({ state: 'not_connected' }),
    } },
  });
  assert.equal((await api.findOngoingRecordingCalendar(now)).status, 'unavailable');
  stalled.resolve();
});

test('capture starts before calendar lookup and a late match cannot bind the replacement recording', async t => {
  const links = withStorage(t);
  const calendar = deferred();
  const native = deferred();
  const calls = [];
  const hooks = createHookHarness();
  const api = loadTsModule('src/services/recordingCalendarService.ts', {
    '@/lib/meetingCalendar': links,
    '@/services/icalService': {
      refreshICalendars: () => calendar.promise,
      getICalendarSnapshot: () => ({ events: [], issues: {} }),
    },
    '@/services/microsoftExportService': { microsoftExportService: {
      connectionStatus: async () => { calls.push('lookup'); return { state: 'connected' }; },
      listCalendarEvents: async () => [{ ...event('late'), start: '2000-01-01T00:00:00Z', end: '2099-01-01T00:00:00Z' }],
    } },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args.sessionId]);
      if (command === 'get_active_recording_calendar') return { link: null, revision: 0 };
      assert.fail('The stopped session may not persist a late match');
    } },
  });
  const noop = () => {};
  const config = { selectedDevices: { micDevice: null, systemDevice: 'Selected audio (output)' }, devicePreferencesStatus: 'ready' };
  const state = { setStatus: noop };
  const { useStartRecordingSession, DEVICE_PREFERENCES_UNAVAILABLE } = loadTsModule('src/hooks/useRecordingStart.ts', {
    react: hooks.react,
    '@/lib/meetingCalendar': links,
    '@/services/recordingCalendarService': api,
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ setIsMeetingActive: noop }) },
    '@/contexts/ConfigContext': { useConfig: () => config },
    '@/contexts/RecordingStateContext': { useRecordingState: () => state, RecordingStatus: { STARTING: 'starting', ERROR: 'error' } },
    '@/services/recordingService': { recordingService: { startRecordingWithDevices: (...args) => {
      calls.push(['start', ...args]);
      return native.promise;
    } } },
    '@/lib/recordingNotification': { showRecordingNotification: noop },
    '@/lib/speakerDetectionCancellation': { offerSpeakerDetectionCancellation: async () => {} },
    '@/lib/analytics': { __esModule: true, default: { trackButtonClick: noop } },
  });
  t.after(() => hooks.unmount());
  for (const status of ['loading', 'error']) {
    config.devicePreferencesStatus = status;
    await assert.rejects(hooks.render(useStartRecordingSession)('remote_request', 'tray-claim'),
      error => error === DEVICE_PREFERENCES_UNAVAILABLE);
    assert.deepEqual(calls, []);
  }
  config.devicePreferencesStatus = 'ready';
  const start = hooks.render(useStartRecordingSession);
  const first = start('detected_call', 'call-claim');
  assert.equal(calls[0][0], 'start');
  assert.equal(calls[0][2], 'Selected audio (output)');
  assert.equal(calls[0][5], 'call-claim');
  assert.equal(calls.includes('lookup'), false);
  await assert.rejects(start('home_page'), error => error === 'recording_start_busy');
  native.resolve('session-a');
  assert.equal(await first, 'session-a');
  await flush();
  assert.ok(calls.includes('lookup'));
  assert.ok(links.getActiveRecordingCalendar('session-a'));

  links.clearActiveRecordingCalendar('session-a');
  const replacement = { sessionId: 'session-b', revision: 1, status: 'none', matches: [], link: links.calendarLinkFromEvent(event('chosen')) };
  links.setActiveRecordingCalendar(replacement);
  calendar.resolve();
  await flush();
  assert.equal(links.getActiveRecordingCalendar('session-a'), null);
  assert.equal(links.getActiveRecordingCalendar('session-b'), replacement);
  assert.equal(calls.some(call => call[0] === 'set_active_recording_calendar'), false);
});

test('sign-out during startup keeps a stale Microsoft calendar subject out of the active session', async t => {
  const links = withStorage(t);
  const native = deferred();
  const read = deferred();
  const invitation = links.calendarLinkFromEvent(event('former-account', { subject: 'Invitation subject' }));
  links.setPendingCalendar(invitation);
  const calls = [];
  const hooks = createHookHarness();
  const noop = () => {};
  const { useStartRecordingSession } = loadTsModule('src/hooks/useRecordingStart.ts', {
    react: hooks.react,
    '@/lib/meetingCalendar': links,
    '@/services/recordingCalendarService': {
      readRecordingCalendar: () => read.promise,
      resolveRecordingCalendar: () => assert.fail('Explicit selection must never start calendar lookup'),
    },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ setIsMeetingActive: noop }) },
    '@/contexts/ConfigContext': { useConfig: () => ({ selectedDevices: { micDevice: null, systemDevice: null }, devicePreferencesStatus: 'ready' }) },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ setStatus: noop }), RecordingStatus: { STARTING: 'starting' } },
    '@/services/recordingService': { recordingService: { startRecordingWithDevices: (...args) => {
      calls.push(args);
      return native.promise;
    } } },
    '@/lib/recordingNotification': { showRecordingNotification: noop },
    '@/lib/analytics': { __esModule: true, default: { trackButtonClick: noop } },
  });
  t.after(() => hooks.unmount());
  const started = hooks.render(useStartRecordingSession)('sidebar_direct');
  assert.match(calls[0][2], /^Meeting /);
  assert.equal(calls[0][3].subject, invitation.subject);
  links.clearMicrosoftCalendarLinks();
  native.resolve('active');
  await started;
  read.resolve({ link: invitation, revision: 1 }); // The old account's response can arrive late.
  await flush();
  assert.equal(links.getPendingCalendar(), null);
  assert.equal(links.getActiveRecordingCalendar('active'), null);
});

test('the active recording review checks attendance explicitly and can unlink the event', async t => {
  const links = withStorage(t);
  let lookupResult = { status: 'none', matches: [] };
  const link = links.calendarLinkFromEvent(event('active', {
    attendees: [{ name: 'Present person', email: null }, { name: 'Other invitee', email: null }],
  }));
  let durable = { link, revision: 1 };
  const edits = [];
  const review = createHookHarness();
  const dialog = createHookHarness();
  t.after(() => { review.unmount(); dialog.unmount(); });
  const jsx = (type, props) => ({ type, props });
  const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props
    ? [node, ...nodes(node.props.children)] : [];
  const controls = {
    button: ['Button'], checkbox: ['Checkbox'],
    dialog: ['Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle', 'DialogTrigger'],
    select: ['Select', 'SelectContent', 'SelectItem', 'SelectTrigger', 'SelectValue'],
  };
  const { RecordingCalendarReview } = loadTsModule('src/components/MeetingAttendance.tsx', {
    react: {
      ...review.react,
      useState: (...args) => current.useState(...args),
      useRef: (...args) => current.useRef(...args),
    },
    'react/jsx-runtime': { jsx, jsxs: jsx }, 'lucide-react': {},
    '@tauri-apps/api/core': {}, sonner: {},
    '@/lib/meetingCalendar': links, '@/lib/meetingCalendarStore': {},
    '@/services/recordingCalendarService': {
      findOngoingRecordingCalendar: async () => lookupResult,
      findCalendarEventsBetween: async () => ({ matches: [], unavailable: [] }),
      readRecordingCalendar: async id => { assert.equal(id, 'recording'); return durable; },
      writeRecordingCalendar: async (id, revision, link) => {
        assert.equal(id, 'recording');
        assert.equal(revision, durable.revision);
        edits.push({ id, revision, link });
        durable = { link, revision: revision + 1 };
        return durable.revision;
      },
    },
    ...Object.fromEntries(Object.entries(controls).map(([path, names]) => [
      '@/components/ui/' + path, Object.fromEntries(names.map(name => [name, name])),
    ])),
  });
  let current = review.react;
  const renderReview = () => review.render(() => RecordingCalendarReview({ disabled: false, sessionId: 'recording' }));
  renderReview();
  await flush();
  const render = () => {
    current = review.react;
    const view = renderReview();
    const child = nodes(view).find(node => node.type?.name === 'AttendanceDialog');
    assert.ok(child);
    current = dialog.react;
    return nodes(dialog.render(() => child.type(child.props)));
  };
  const find = (type, label) => render().find(node => node.type === type && (!label || node.props.children === label));
  find('Dialog').props.onOpenChange(true);
  const checkboxes = render().filter(node => node.type === 'Checkbox');
  assert.deepEqual(checkboxes.map(node => node.props.checked), [false, false]);
  checkboxes[0].props.onCheckedChange(true);
  find('Button', 'Save attendance').props.onClick();
  await flush();
  assert.deepEqual(Array.from(durable.link.attendees, person => person.attendance),
    ['present', 'unreviewed']);
  assert.equal(durable.link.reviewed, true);
  find('Dialog').props.onOpenChange(true);
  find('Button', 'Unlink event').props.onClick();
  await flush();
  assert.equal(durable.link, null);
  assert.deepEqual(edits.map(({ revision }) => revision), [1, 2]);
  assert.equal(links.getActiveRecordingCalendar('recording').link, null);
});

test('a partial calendar result can be checked again from the recording review', async t => {
  const links = withStorage(t);
  const found = links.calendarLinkFromEvent(event('found'));
  links.setActiveRecordingCalendar({
    sessionId: 'active', revision: 0, startedAt: now, link: null, status: 'unavailable', matches: [], unavailable: ['Microsoft 365'],
  });
  const review = createHookHarness();
  t.after(() => review.unmount());
  const jsx = (type, props) => ({ type, props });
  const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props
    ? [node, ...nodes(node.props.children)] : [];
  const controls = {
    button: ['Button'], checkbox: ['Checkbox'],
    dialog: ['Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle', 'DialogTrigger'],
    select: ['Select', 'SelectContent', 'SelectItem', 'SelectTrigger', 'SelectValue'],
  };
  let lookupResult = { status: 'unavailable', matches: [], unavailable: ['Microsoft 365'] };
  const { RecordingCalendarReview } = loadTsModule('src/components/MeetingAttendance.tsx', {
    react: review.react,
    'react/jsx-runtime': { jsx, jsxs: jsx }, 'lucide-react': {},
    '@tauri-apps/api/core': {}, sonner: {},
    '@/lib/meetingCalendar': links, '@/lib/meetingCalendarStore': {},
    '@/services/recordingCalendarService': { findOngoingRecordingCalendar: async when => {
      assert.equal(when, now);
      return lookupResult;
    } },
    ...Object.fromEntries(Object.entries(controls).map(([path, names]) => [
      '@/components/ui/' + path, Object.fromEntries(names.map(name => [name, name])),
    ])),
  });
  const render = () => nodes(review.render(() => RecordingCalendarReview({ disabled: false, sessionId: 'active' })));
  const text = view => view.filter(node => node.props.role === 'status').map(node => node.props.children).join('');
  assert.match(text(render()), /Could not read Microsoft 365/);
  // Still unavailable: the status stays and the action stays available.
  render().find(node => node.type === 'Button').props.onClick();
  await flush();
  assert.match(text(render()), /Could not read Microsoft 365/);
  lookupResult = { status: 'matched', matches: [found], unavailable: [] };
  render().find(node => node.type === 'Button').props.onClick();
  await flush();
  const view = render();
  assert.match(text(view), /Found a matching calendar event/);
  // The event is only a candidate until the person chooses it; nothing was linked or retitled.
  assert.equal(links.getActiveRecordingCalendar('active').link, null);
  assert.equal(links.getActiveRecordingCalendar('active').matches[0].eventId, 'found');
  assert.ok(!view.some(node => node.type === 'Button' && node.props.children === 'Check again'));
});

test('a saved meeting without a link can be linked from the current calendars, with failing sources retried', async t => {
  const links = withStorage(t);
  const candidate = links.calendarLinkFromEvent(event('candidate', { seriesId: graphSeries }));
  const lookups = [];
  const saved = [];
  const preferences = [];
  let result = { matches: [], unavailable: ['Microsoft 365'] };
  const page = createHookHarness();
  const dialog = createHookHarness();
  const picker = createHookHarness();
  t.after(() => { page.unmount(); dialog.unmount(); picker.unmount(); });
  const jsx = (type, props) => ({ type, props });
  const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props
    ? [node, ...nodes(node.props.children)] : [];
  const controls = {
    button: ['Button'], checkbox: ['Checkbox'],
    dialog: ['Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle', 'DialogTrigger'],
    select: ['Select', 'SelectContent', 'SelectItem', 'SelectTrigger', 'SelectValue'],
  };
  let current = page.react;
  const { SavedMeetingAttendance } = loadTsModule('src/components/MeetingAttendance.tsx', {
    react: {
      ...page.react,
      useState: (...args) => current.useState(...args),
      useRef: (...args) => current.useRef(...args),
    },
    'react/jsx-runtime': { jsx, jsxs: jsx }, 'lucide-react': {},
    '@tauri-apps/api/core': { invoke: async (command, { meetingId, templateId }) => {
      assert.equal(command, 'set_meeting_series_template');
      preferences.push([meetingId, templateId]);
    } }, sonner: {},
    '@/lib/meetingCalendar': links,
    '@/lib/meetingCalendarStore': {
      MEETING_SERIES_TEMPLATE_CHANGED: 'series-changed',
      getMeetingCalendar: async () => null,
      setMeetingCalendar: async (meetingId, link) => { saved.push([meetingId, link]); },
    },
    '@/services/recordingCalendarService': {
      findCalendarEventsBetween: async (...window) => { lookups.push(window); return result; },
    },
    ...Object.fromEntries(Object.entries(controls).map(([path, names]) => [
      '@/components/ui/' + path, Object.fromEntries(names.map(name => [name, name])),
    ])),
  });
  const renderPage = () => {
    current = page.react;
    return page.render(() => SavedMeetingAttendance({
      meetingId: 'saved', createdAt: '2030-04-15T11:00:00Z', durationSeconds: 3600,
      templates: [{ id: 'standard_meeting', name: 'Standard meeting' }], remembered: null,
    }));
  };
  const renderDialog = () => {
    const child = nodes(renderPage()).find(node => node.type?.name === 'AttendanceDialog');
    assert.ok(child);
    current = dialog.react;
    return { child, tree: nodes(dialog.render(() => child.type(child.props))) };
  };
  renderPage();
  await flush();
  let { child, tree } = renderDialog();
  assert.equal(child.props.link, null);
  assert.equal(child.props.label, 'Link calendar event');
  assert.equal(lookups.length, 0);
  tree.find(node => node.type === 'Dialog').props.onOpenChange(true);
  await flush();
  // The search covers the recorded time, not just the moment of saving.
  assert.deepEqual(lookups[0], [Date.parse('2030-04-15T09:50:00Z'), Date.parse('2030-04-15T11:10:00Z')]);
  ({ tree } = renderDialog());
  // The failing source is named and can be checked again.
  const sources = tree.find(node => node.type?.name === 'UnavailableSources');
  const shown = nodes(sources.type(sources.props));
  assert.match(JSON.stringify(shown.find(node => node.type === 'span').props.children), /Could not read .*Microsoft 365/);
  result = { matches: [candidate], unavailable: [] };
  shown.find(node => node.type === 'Button' && node.props.children === 'Check again').props.onClick();
  await flush();
  assert.equal(lookups.length, 2);
  ({ tree } = renderDialog());
  tree.find(node => node.type === 'Select').props.onValueChange('0');
  ({ tree } = renderDialog());
  tree.find(node => node.type === 'Button' && node.props.children === 'Link event').props.onClick();
  await flush();
  assert.equal(saved[0][0], 'saved');
  assert.equal(saved[0][1].eventId, 'candidate');
  assert.equal(saved[0][1].reviewed, true);
  assert.equal(saved[0][1].attendees[0].attendance, 'unreviewed');
  assert.equal(saved[0][1].seriesId, graphSeries);
  ({ tree } = renderDialog());
  const seriesControl = tree.find(node => node.type?.name === 'SeriesTemplatePreference');
  assert.ok(seriesControl);
  current = picker.react;
  const select = nodes(picker.render(() => seriesControl.type(seriesControl.props)))
    .find(node => node.type === 'Select');
  await select.props.onValueChange('standard_meeting');
  await select.props.onValueChange('use-default');
  assert.deepEqual(preferences, [['saved', 'standard_meeting'], ['saved', null]]);
});
