import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
const servicePath = '@/services/icalService';

function nodes(node) {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (!node || typeof node !== 'object') return [];
  return [node, ...nodes(node.props?.actions), ...nodes(node.props?.children)];
}

function text(node) {
  if (Array.isArray(node)) return node.map(text).join('');
  if (typeof node === 'string') return node;
  return node?.props ? text(node.props.children) : '';
}

function settings(t, state, service = {}) {
  const hooks = createHookHarness();
  let active = hooks;
  const pending = { link: null, writable: true };
  const controls = {
    button: ['Button'], checkbox: ['Checkbox'], input: ['Input'], 'page-section': ['PageSection'],
    select: ['Select', 'SelectContent', 'SelectItem', 'SelectTrigger', 'SelectValue'],
    tabs: ['Tabs', 'TabsList', 'TabsTrigger', 'TabsContent'],
    dialog: ['Dialog', 'DialogTrigger', 'DialogHeader', 'DialogDescription', 'DialogTitle', 'DialogContent'],
    'dropdown-menu': ['DropdownMenu', 'DropdownMenuTrigger', 'DropdownMenuContent', 'DropdownMenuItem'],
  };
  const components = Object.fromEntries(Object.entries(controls).map(([path, names]) => [
    '@/components/ui/' + path, Object.fromEntries(names.map(name => [name, name])),
  ]));
  const { ICalendarSettings } = loadTsModule('src/components/ICalendarSettings.tsx', {
    react: new Proxy({}, { get: (_, name) => active.react[name] }),
    'react/jsx-runtime': { jsx, jsxs: jsx },
    'lucide-react': {},
    ...components,
    '@/hooks/useICalendar': { useICalendar: () => state },
    '@/hooks/usePendingCalendar': { usePendingCalendar: () => pending.link },
    '@/lib/meetingCalendar': { setPendingCalendar: value => {
      if (pending.writable) pending.link = value;
      return pending.writable;
    } },
    [servicePath]: service,
  });
  t.after(() => hooks.unmount());
  const render = () => hooks.render(ICalendarSettings);
  const child = name => {
    const childHooks = createHookHarness();
    const childRender = () => {
      const child = nodes(render()).find(n => n.type?.name === name);
      assert.ok(child, 'Expected ' + name);
      active = childHooks;
      try { return childHooks.render(() => child.type(child.props)); }
      finally { active = hooks; }
    };
    t.after(() => childHooks.unmount());
    return childRender;
  };
  return { render, child, pending };
}

const empty = { sources: [], events: [], loading: false, issues: {}, error: null, refresh: async () => {} };
const find = (tree, type, predicate = () => true) => nodes(tree).find(n => n.type === type && predicate(n));

test('a subscription is registered once and its private address is cleared when saved', async t => {
  const saving = deferred();
  const calls = [];
  const app = settings(t, empty, { addICalendarSubscription: (...args) => { calls.push(args); return saving.promise; } });
  const render = app.child('AddCalendarDialog');
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
  const app = settings(t, empty, { importICalendarFile: async value => {
    imports.push(value);
    throw 'https://example.com/private.ics';
  } });
  const render = app.child('AddCalendarDialog');
  find(render(), 'Dialog').props.onOpenChange(true);
  const input = { files: [file], value: file.name };
  await find(render(), 'Input', n => n.props.type === 'file').props.onChange({ currentTarget: input });
  assert.deepEqual(imports, [file]);
  assert.equal(input.value, '');
  assert.equal(find(render(), 'Dialog').props.open, true);
  assert.match(text(find(render(), 'p', n => n.props.role === 'alert')), /import/i);
  assert.doesNotMatch(text(render()), /private\.ics/);
});

test('an all-day meeting shows its calendar name and binds the reviewed invitee choices', t => {
  // Local midnight may fall on a different UTC date.
  const day = new Date(2031, 4, 14);
  const state = {
    ...empty, sources: [{ id: 'work', name: 'Team calendar', kind: 'file' }],
    events: [{ id: 'meeting', sourceId: 'work', subject: 'Planning', start: day.toISOString(), allDay: true,
      attendees: [{ name: 'First', email: null }, { name: 'Second', email: null }] }],
  };
  const app = settings(t, state);
  const label = text(find(app.render(), 'SelectItem'));
  assert.match(label, /Team calendar/);
  assert.match(label, /All day/);
  assert.ok(label.includes(day.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })));
  const review = app.child('EventReview');
  nodes(review()).filter(n => n.type === 'Checkbox')[1].props.onCheckedChange(false);
  app.pending.writable = false;
  find(review(), 'Button').props.onClick();
  assert.match(text(review()), /Could not save the meeting selection/);
  assert.equal(find(review(), 'Button').props.disabled, false);
  app.pending.writable = true;
  find(review(), 'Button').props.onClick();
  assert.deepEqual(JSON.parse(JSON.stringify(app.pending.link)), {
    source: 'ical', sourceId: 'work', eventId: 'meeting', subject: 'Planning',
    attendees: [{ name: 'First', email: null, included: true }, { name: 'Second', email: null, included: false }],
  });
  assert.equal(find(review(), 'Button').props.disabled, true);
  app.pending.link = { ...app.pending.link, source: 'microsoft' };
  assert.equal(find(review(), 'Button').props.disabled, false);
});

test('a broken subscription can be refreshed and removed without displaying an empty meeting result', async t => {
  const removal = deferred();
  const removed = [];
  let refreshed = 0;
  const state = {
    ...empty,
    sources: [{ id: 'work', name: 'Work calendar', kind: 'subscription' }],
    issues: { work: 'Check the subscription link and try again.' },
    refresh: async () => { refreshed++; },
  };
  const app = settings(t, state, { removeICalendarSource: id => { removed.push(id); return removal.promise; } });
  let tree = app.render();
  assert.match(text(tree), /Check the subscription link/);
  assert.doesNotMatch(text(tree), /No meetings found/);
  find(tree, 'Button', n => text(n) === 'Refresh').props.onClick();
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

test('every upcoming meeting is reachable while the menu renders one page at a time', t => {
  const events = Array.from({ length: 52 }, (_, i) => ({
    id: 'event-' + i, sourceId: 'work', subject: 'Meeting ' + i, start: '2031-05-14T12:00:00Z', allDay: false, attendees: [],
  }));
  const app = settings(t, { ...empty, sources: [{ id: 'work', name: 'Work calendar', kind: 'file' }], events });
  let tree = app.render();
  assert.equal(nodes(tree).filter(n => n.type === 'SelectItem').length, 50);
  find(tree, 'Button', n => text(n) === 'Next').props.onClick();
  tree = app.render();
  assert.equal(nodes(tree).filter(n => n.type === 'SelectItem').length, 2);
  find(tree, 'Select').props.onValueChange('event-51');
  find(app.child('EventReview')(), 'Button').props.onClick();
  assert.equal(app.pending.link.eventId, 'event-51');
});
