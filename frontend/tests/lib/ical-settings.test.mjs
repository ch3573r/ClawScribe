import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
const servicePath = '@/services/icalService';

function nodes(node) {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (!node || typeof node !== 'object') return [];
  return [node, ...nodes(node.props?.actions), ...nodes(node.props?.aside), ...nodes(node.props?.description), ...nodes(node.props?.title), ...nodes(node.props?.children)];
}

function text(node) {
  if (Array.isArray(node)) return node.map(text).join('');
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  return node?.props ? text(node.props.title) + text(node.props.description) + text(node.props.children) : '';
}

const controls = {
  button: ['Button'], checkbox: ['Checkbox'], input: ['Input'],
  tabs: ['Tabs', 'TabsList', 'TabsTrigger', 'TabsContent'],
  dialog: ['Dialog', 'DialogTrigger', 'DialogHeader', 'DialogDescription', 'DialogTitle', 'DialogContent', 'DialogFooter'],
  'dropdown-menu': ['DropdownMenu', 'DropdownMenuTrigger', 'DropdownMenuContent', 'DropdownMenuItem'],
};
const uiMocks = Object.fromEntries(Object.entries(controls).map(([path, names]) => [
  '@/components/ui/' + path, Object.fromEntries(names.map(name => [name, name])),
]));

function pendingStore() {
  const pending = { link: null, writable: true };
  return { pending, mock: { setPendingCalendar: value => {
    if (pending.writable) pending.link = value;
    return pending.writable;
  } } };
}

function baseMocks(hooks, service, store) {
  return {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    'lucide-react': {}, sonner: { toast: { error() {} } },
    ...uiMocks,
    '@/components/IntegrationIcons': { OutlookCalendarIcon: 'OutlookCalendarIcon' },
    '@/lib/meetingCalendar': { ...loadTsModule('src/lib/meetingCalendar.ts'), ...store.mock },
    [servicePath]: service,
  };
}

const empty = { sources: [], events: [], loading: false, issues: {}, error: null, refresh: async () => {} };
const find = (tree, type, predicate = () => true) => nodes(tree).find(n => n.type === type && predicate(n));
const event = (id, extra = {}) => ({
  id, sourceId: 'work', subject: 'Meeting ' + id, start: '2031-05-14T12:00:00Z', allDay: false, attendees: [], ...extra,
});

function addDialog(t, service = {}) {
  const hooks = createHookHarness();
  const store = pendingStore();
  const { AddCalendarDialog } = loadTsModule('src/components/ICalendarSettings.tsx', baseMocks(hooks, service, store));
  t.after(() => hooks.unmount());
  return () => hooks.render(AddCalendarDialog);
}

function useButton(t, item, store = pendingStore()) {
  const hooks = createHookHarness();
  const { UseEventButton } = loadTsModule('src/components/ICalendarSettings.tsx', baseMocks(hooks, {}, store));
  t.after(() => hooks.unmount());
  return { render: () => hooks.render(() => UseEventButton({ event: item, pending: store.pending.link })), store };
}

function calendarsPage(t, state, { service = {}, microsoft = {}, store = pendingStore() } = {}) {
  const hooks = createHookHarness();
  const listeners = { addEventListener() {}, removeEventListener() {} };
  globalThis.window = { ...listeners, setInterval: () => 0, clearInterval() {} };
  globalThis.document = { ...listeners, visibilityState: 'visible' };
  const mocks = {
    ...baseMocks(hooks, service, store),
    '@/hooks/useICalendar': { useICalendar: () => state },
    '@/hooks/usePendingCalendar': { usePendingCalendar: () => store.pending.link },
    '@/hooks/useMicrosoftExport': { useMicrosoftExport: () => ({
      connection: { state: 'not_connected' }, calendarEvents: [], currentMeeting: null, loadingCalendar: false, loadCalendar: async () => {}, ...microsoft,
    }) },
  };
  const { CalendarsSettings } = loadTsModule('src/components/settings/CalendarsSettings.tsx', mocks);
  t.after(() => hooks.unmount());
  return { render: () => hooks.render(CalendarsSettings), store };
}

test('a subscription is registered once and its private address is cleared when saved', async t => {
  const saving = deferred();
  const calls = [];
  const render = addDialog(t, { addICalendarSubscription: (...args) => { calls.push(args); return saving.promise; } });
  find(render(), 'Dialog').props.onOpenChange(true);
  find(render(), 'Input', n => n.props.type === 'text').props.onChange({ target: { value: 'Team calendar' } });
  find(render(), 'Input', n => n.props.type === 'password').props.onChange({ target: { value: 'https://example.com/calendar.ics' } });
  const submit = () => find(render(), 'form').props.onSubmit({ preventDefault() {} });
  const first = submit();
  assert.equal(find(render(), 'Button', n => n.props.type === 'submit').props.disabled, true);
  await submit();
  assert.deepEqual(calls, [['Team calendar', 'https://example.com/calendar.ics']]);
  saving.resolve();
  await first;
  assert.equal(find(render(), 'Dialog').props.open, false);
  assert.equal(find(render(), 'Input', n => n.props.type === 'password').props.value, '');
});

test('a failed file import resets the picker and never echoes an unexpected rejection', async t => {
  const file = { name: 'example.ics' };
  const imports = [];
  const render = addDialog(t, { importICalendarFile: async value => {
    imports.push(value);
    throw 'https://example.com/private.ics';
  } });
  find(render(), 'Dialog').props.onOpenChange(true);
  const input = { files: [file], value: file.name };
  await find(render(), 'Input', n => n.props.type === 'file').props.onChange({ currentTarget: input });
  assert.deepEqual(imports, [file]);
  assert.equal(input.value, '');
  assert.equal(find(render(), 'Dialog').props.open, true);
  assert.match(text(find(render(), 'p', n => n.props.role === 'alert')), /import/i);
  assert.doesNotMatch(text(render()), /private\.ics/);
});

test('an all-day event can be chosen explicitly and its invitees still need review', t => {
  const item = {
    key: 'ical:work:meeting', source: 'ical', sourceId: 'work', sourceName: 'Team calendar', id: 'meeting',
    subject: 'Planning', start: new Date(2031, 4, 14).toISOString(), allDay: true,
    attendees: [{ name: 'First', email: null }, { name: 'Second', email: null }],
  };
  const app = useButton(t, item);
  const confirm = () => find(app.render(), 'Button');
  app.store.pending.writable = false;
  confirm().props.onClick();
  assert.equal(app.store.pending.link, null);
  assert.equal(confirm().props.disabled, false);
  app.store.pending.writable = true;
  confirm().props.onClick();
  assert.deepEqual(JSON.parse(JSON.stringify(app.store.pending.link)), {
    source: 'ical', sourceId: 'work', eventId: 'meeting', subject: 'Planning', start: item.start,
    attendees: [{ name: 'First', email: null, attendance: 'unreviewed' }, { name: 'Second', email: null, attendance: 'unreviewed' }],
  });
  assert.equal(confirm().props.disabled, true);
  app.store.pending.link = { ...app.store.pending.link, source: 'microsoft' };
  assert.equal(confirm().props.disabled, false);
});

test('a meeting without invitees is set directly and a Microsoft meeting stays a Microsoft selection', t => {
  const item = { key: 'microsoft:m1', source: 'microsoft', sourceName: 'Microsoft 365', id: 'm1', subject: 'Standup', start: null, allDay: false, attendees: [] };
  const app = useButton(t, item);
  const use = () => find(app.render(), 'Button');
  assert.equal(text(use()), 'Use');
  use().props.onClick();
  assert.deepEqual(JSON.parse(JSON.stringify(app.store.pending.link)), { source: 'microsoft', eventId: 'm1', subject: 'Standup', start: null, attendees: [] });
  assert.equal(text(use()), 'Selected');
  assert.equal(use().props.disabled, true);
});

test('a broken subscription shows its problem, can be refreshed and removed, and hides the empty result', async t => {
  const removal = deferred();
  const removed = [];
  let refreshed = 0;
  const state = {
    ...empty,
    sources: [{ id: 'work', name: 'Work calendar', kind: 'subscription' }],
    issues: { work: 'Check the subscription link and try again.' },
    refresh: async () => { refreshed++; },
  };
  const app = calendarsPage(t, state, { service: { removeICalendarSource: id => { removed.push(id); return removal.promise; } } });
  let tree = app.render();
  assert.match(text(tree), /Check the subscription link/);
  assert.doesNotMatch(text(tree), /No calendars yet/);
  const refresh = nodes(tree).find(n => n.props?.label === 'Refresh calendars');
  assert.ok(refresh, 'Refresh calendars action missing');
  refresh.props.onClick();
  assert.equal(refreshed, 1);
  find(tree, 'DropdownMenuItem').props.onSelect();
  tree = app.render();
  assert.deepEqual(removed, ['work']);
  assert.equal(find(tree, 'Button', n => n.props['aria-label'] === 'Manage Work calendar').props.disabled, true);
  assert.match(text(tree), /Removing calendar/);
  removal.resolve();
  await flush();
  assert.equal(find(app.render(), 'Button', n => n.props['aria-label'] === 'Manage Work calendar').props.disabled, false);
});

test('with nothing connected the page shows an empty state and still offers Add calendar', t => {
  const tree = calendarsPage(t, empty).render();
  assert.match(text(tree), /No calendars yet/);
  assert.ok(nodes(tree).some(n => n.type?.name === 'AddCalendarDialog'));
});

test('every upcoming meeting is reachable while the list shows eight at a time', t => {
  const events = Array.from({ length: 20 }, (_, i) => event('event-' + i, { start: new Date(2031, 4, 14, 8, i).toISOString() }));
  const app = calendarsPage(t, { ...empty, sources: [{ id: 'work', name: 'Work calendar', kind: 'file' }], events });
  const rows = tree => nodes(tree).filter(n => n.type?.name === 'UseEventButton');
  assert.equal(rows(app.render()).length, 8);
  const more = () => find(app.render(), 'Button', n => text(n) === 'Show more');
  more().props.onClick();
  more().props.onClick();
  assert.equal(rows(app.render()).length, 20);
  assert.equal(more(), undefined);
});

test('Microsoft and iCalendar meetings are merged in time order and the current meeting is not lost', t => {
  const microsoft = {
    connection: { state: 'connected', userDisplayName: 'Example user' },
    calendarEvents: [{ id: 'late', subject: 'Late', start: '2031-05-14T15:00:00Z', attendees: [] }],
    currentMeeting: { id: 'now', subject: 'Now', start: '2031-05-14T08:00:00Z', attendees: [] },
  };
  const app = calendarsPage(t, { ...empty, sources: [{ id: 'work', name: 'Work calendar', kind: 'file' }], events: [event('mid', { start: '2031-05-14T12:00:00Z' })] }, { microsoft });
  const tree = app.render();
  assert.deepEqual(nodes(tree).filter(n => n.type?.name === 'UseEventButton').map(n => n.props.event.id), ['now', 'mid', 'late']);
  assert.match(text(tree), /Microsoft 365 calendar/);
  assert.match(text(tree), /Example user · includes invitees/);
});
