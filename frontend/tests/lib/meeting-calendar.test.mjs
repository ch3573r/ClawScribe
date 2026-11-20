import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const start = '2030-04-15T10:00:00Z';
const end = '2030-04-15T11:00:00Z';
const now = Date.parse('2030-04-15T10:15:00Z');
const event = (id, changes = {}) => ({
  id, subject: 'Planning', start, end, attendees: [{ name: 'Invitee', email: null }], ...changes,
});

test('a fresh invitation and a legacy checked invitee both require attendance review', t => {
  const before = globalThis.window;
  const data = new Map([['clawscribe.pendingCalendar', JSON.stringify({
    eventId: 'previous', subject: null,
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

test('automatic matching uses only a unique, ongoing timed event across sources', () => {
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

test('starting one recording preserves an explicit choice made for the next recording during startup', t => {
  const links = withStorage(t);
  const first = links.calendarLinkFromEvent(event('first'));
  const second = links.calendarLinkFromEvent(event('second'));
  assert.ok(links.setPendingCalendar(first));
  const original = links.getNextCalendarChoice();
  assert.ok(links.setPendingCalendar(second));
  links.beginRecordingCalendar({ link: first, status: 'none', matches: [], fallbackTitle: 'Recording' }, original);
  assert.equal(links.getActiveRecordingCalendar().link.eventId, 'first');
  assert.equal(links.getNextCalendarChoice().link.eventId, 'second');
  assert.ok(links.setPendingCalendar(null, true));
  const skip = links.getNextCalendarChoice();
  assert.equal(skip.skip, true);
  links.beginRecordingCalendar({ link: null, status: 'none', matches: [], fallbackTitle: 'Recording' }, skip);
  assert.equal(links.getNextCalendarChoice().skip, false);
  assert.equal(links.getActiveRecordingCalendar().link, null);
});

test('a failed legacy migration keeps the original attendance for retry', async t => {
  const links = withStorage(t);
  const key = links.LEGACY_MEETING_CALENDAR_PREFIX + 'saved';
  const store = globalThis.window.localStorage;
  store.setItem(key, JSON.stringify({ eventId: 'legacy', attendees: [{ name: 'Previously checked', included: true }] }));
  let fail = true;
  const received = [];
  const { migrateLegacyMeetingCalendars } = loadTsModule('src/lib/meetingCalendarStore.ts', {
    '@/lib/meetingCalendar': links,
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      assert.equal(command, 'set_meeting_calendar');
      if (fail) throw new Error('temporary database failure');
      received.push(args);
    } },
  });
  await assert.rejects(migrateLegacyMeetingCalendars(), /temporary database failure/);
  assert.ok(store.getItem(key));
  fail = false;
  await migrateLegacyMeetingCalendars();
  assert.equal(store.getItem(key), null);
  assert.equal(received[0].meetingId, 'saved');
  assert.equal(received[0].onlyIfMissing, true);
  assert.equal(received[0].link.attendees[0].attendance, 'unreviewed');
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
  assert.deepEqual(requests, [['2030-04-15T10:14:00.000Z', '2030-04-15T10:16:00.000Z', true]]);
  assert.equal(lookup.status, 'unavailable');
  assert.equal(lookup.matches[0].eventId, 'file-event');
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

test('the active recording review checks attendance explicitly and can unlink the event', async t => {
  const links = withStorage(t);
  const link = links.calendarLinkFromEvent(event('active', {
    attendees: [{ name: 'Present person', email: null }, { name: 'Other invitee', email: null }],
  }));
  links.beginRecordingCalendar({ link, status: 'matched', matches: [link], fallbackTitle: 'Recording' }, links.getNextCalendarChoice());
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
    ...Object.fromEntries(Object.entries(controls).map(([path, names]) => [
      '@/components/ui/' + path, Object.fromEntries(names.map(name => [name, name])),
    ])),
  });
  const titles = [];
  let current = review.react;
  const render = () => {
    current = review.react;
    const view = review.render(() => RecordingCalendarReview({ disabled: false, onTitleChange: title => titles.push(title) }));
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
  assert.deepEqual(Array.from(links.getActiveRecordingCalendar().link.attendees, person => person.attendance),
    ['present', 'unreviewed']);
  find('Dialog').props.onOpenChange(true);
  find('Button', 'Unlink event').props.onClick();
  await flush();
  assert.equal(links.getActiveRecordingCalendar().link, null);
  assert.deepEqual(titles, ['Planning', 'Recording']);
});
