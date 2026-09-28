import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const { formatSidebarMeetingDate } = loadTsModule('src/lib/meetingDates.ts');
const now = new Date(2026, 8, 28, 16, 0);

test('sidebar dates show today and yesterday with the local time', () => {
  const today = new Date(2026, 8, 28, 13, 16);
  const yesterday = new Date(2026, 8, 27, 13, 16);
  const time = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' }).format(today);
  assert.match(formatSidebarMeetingDate(today.toISOString(), now), /^Today, /);
  assert.ok(formatSidebarMeetingDate(today.toISOString(), now).endsWith(time));
  assert.match(formatSidebarMeetingDate(yesterday.toISOString(), now), /^Yesterday, /);
  assert.ok(formatSidebarMeetingDate(yesterday.toISOString(), now).endsWith(time));
});

test('sidebar dates include the year only for older years', () => {
  const currentYear = formatSidebarMeetingDate(new Date(2026, 8, 12).toISOString(), now);
  assert.match(currentYear, /12/);
  assert.doesNotMatch(currentYear, /2026|Today|Yesterday/);
  const previousYear = formatSidebarMeetingDate(new Date(2025, 8, 12).toISOString(), now);
  assert.match(previousYear, /12/);
  assert.match(previousYear, /2025/);
});

test('sidebar dates use calendar days across month and year boundaries', () => {
  assert.match(formatSidebarMeetingDate(new Date(2025, 11, 31, 23, 59).toISOString(), new Date(2026, 0, 1, 0, 1)), /^Yesterday, /);
  assert.match(formatSidebarMeetingDate(new Date(2026, 2, 31, 0, 1).toISOString(), new Date(2026, 3, 1, 23, 59)), /^Yesterday, /);
  assert.doesNotMatch(formatSidebarMeetingDate(new Date(2026, 8, 26, 23, 59).toISOString(), new Date(2026, 8, 28, 0, 1)), /Today|Yesterday/);
});

test('missing and invalid sidebar dates fall back to Saved meeting', () => {
  for (const value of [undefined, '', 'invalid date']) {
    assert.equal(formatSidebarMeetingDate(value, now), 'Saved meeting');
  }
});
