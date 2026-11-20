import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const lib = loadTsModule('src/lib/sidebarMeetings.ts');
// Thursday 19 November 2026, 10:00 local time. The week starts on Monday 16 November.
const now = new Date(2026, 10, 19, 10, 0);
const at = (month, day, hour = 9, minute = 0) => new Date(2026, month, day, hour, minute).toISOString();

test('meetings are grouped as today, this week and earlier, in input order', () => {
  const items = [
    { id: 'a', created_at: at(10, 19, 8) },
    { id: 'b', created_at: at(10, 16) },
    { id: 'c', created_at: at(10, 15, 23, 59) },
    { id: 'd', created_at: at(9, 10) },
    { id: 'e' },
    { id: 'f', created_at: 'not a date' },
  ];
  const groups = JSON.parse(JSON.stringify(lib.groupMeetings(items, now).map(group => [group.label, group.items.map(item => item.id)])));
  assert.deepEqual(groups, [['Today', ['a']], ['This week', ['b']], ['Earlier', ['c', 'd', 'e', 'f']]]);
  assert.deepEqual([...lib.groupMeetings([{ id: 'x', created_at: at(9, 1) }], now).map(group => group.label)], ['Earlier']);
});

test('row dates are one short token', () => {
  assert.match(lib.shortMeetingDate(at(10, 19, 8, 5), now), /\d/);
  assert.equal(lib.shortMeetingDate(at(10, 18), now), 'Yesterday');
  assert.equal(lib.shortMeetingDate(undefined, now), 'Saved');
  assert.equal(lib.shortMeetingDate('nope', now), 'Saved');
  assert.match(lib.shortMeetingDate(at(10, 5), now), /5/);
  assert.match(lib.shortMeetingDate(new Date(2025, 10, 5).toISOString(), now), /2025/);
});

test('next up shows the start, a relative time for today, and Now while it runs', () => {
  const later = lib.describeNextUp(at(10, 19, 12, 18), at(10, 19, 13), now);
  assert.match(later.when, /^Today /);
  assert.equal(later.relative, 'in 2 h 18 min');
  assert.equal(lib.describeNextUp(at(10, 19, 10, 45), null, now).relative, 'in 45 min');
  assert.equal(lib.describeNextUp(at(10, 19, 12, 0), null, now).relative, 'in 2 h');
  const tomorrow = lib.describeNextUp(at(10, 20, 9), at(10, 20, 10), now);
  assert.match(tomorrow.when, /^Tomorrow /);
  assert.equal(tomorrow.relative, null);
  assert.equal(lib.describeNextUp(at(10, 23, 9), at(10, 23, 10), now).relative, null);
  assert.deepEqual({ ...lib.describeNextUp(at(10, 19, 9, 30), at(10, 19, 10, 30), now) }, { when: 'Now', relative: null });
  assert.equal(lib.describeNextUp(at(10, 19, 8), at(10, 19, 9), now), null);
});

test('durations and the pick of the next event within seven days', () => {
  assert.equal(lib.eventDuration(at(10, 19, 13), at(10, 19, 13, 30)), '30 min');
  assert.equal(lib.eventDuration(at(10, 19, 13), at(10, 19, 14, 30)), '1 h 30 min');
  assert.equal(lib.eventDuration(at(10, 19, 13), null), null);
  const entries = [
    { id: 'ended', allDay: false, event: { start: at(10, 19, 8), end: at(10, 19, 9) } },
    { id: 'allday', allDay: true, event: { start: at(10, 19, 0), end: at(10, 20, 0) } },
    { id: 'far', allDay: false, event: { start: at(10, 30, 9), end: at(10, 30, 10) } },
    { id: 'running', allDay: false, event: { start: at(10, 19, 9, 30), end: at(10, 19, 10, 30) } },
    { id: 'soon', allDay: false, event: { start: at(10, 19, 13), end: at(10, 19, 14) } },
  ];
  assert.equal(lib.pickNextUp(entries, now, 7).id, 'running');
  assert.equal(lib.pickNextUp(entries.filter(entry => entry.id === 'far'), now, 7), null);
  assert.equal(lib.pickNextUp([], now, 7), null);
});
