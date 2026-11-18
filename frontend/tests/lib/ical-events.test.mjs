import assert from "node:assert/strict";
import test from "node:test";
import { loadTsModule } from "./load-ts-module.mjs";

const { parseICalendar: parse } = loadTsModule("src/lib/icalEvents.ts");
const calendar = (...parts) =>
  ["BEGIN:VCALENDAR", "VERSION:2.0", ...parts, "END:VCALENDAR"].join("\r\n");
const event = (...props) => ["BEGIN:VEVENT", ...props, "END:VEVENT"].join("\r\n");
const from = new Date("2026-03-20T00:00Z");
const until = new Date("2026-04-10T00:00Z");

test("reads only meeting metadata, including folded names and mailto invitees", () => {
  const text = calendar(event("UID:https://example.com/private-invite", "DTSTART:20260321T100000Z",
    "DURATION:PT30M", "SUMMARY:Review\\, phase", "  two", "DESCRIPTION:private-notes",
    "URL:https://example.com/private-join", "ATTENDEE;CN=Viewer:https://example.com/private-invite",
    "ORGANIZER;CN=Organizer:mailto:organizer@example.com", "ATTENDEE;CN=Guest:mailto:g@example.org"));
  const [meeting] = parse(text, "file-a", from, until);
  assert.equal(meeting.subject, "Review, phase two");
  assert.equal(meeting.start, "2026-03-21T10:00:00.000Z");
  assert.equal(meeting.end, "2026-03-21T10:30:00.000Z");
  assert.equal(meeting.isOnlineMeeting, false);
  assert.equal(meeting.joinUrl, null);
  assert.deepEqual(JSON.parse(JSON.stringify(meeting.organizer)), { name: "Organizer", email: "organizer@example.com" });
  assert.ok(meeting.attendees.some(a => a.name === "Guest" && a.email === "g@example.org"));
  assert.doesNotMatch(JSON.stringify(meeting), /private-|https:/);
  assert.equal(parse(text, "file-a", from, until)[0].id, meeting.id);
  assert.notEqual(parse(text, "file-b", from, until)[0].id, meeting.id);
});

test("includes overlaps and point events while treating the upper boundary as exclusive", () => {
  const text = calendar(event("UID:overlap", "DTSTART:20260319T220000Z", "DTEND:20260320T010000Z"),
    event("UID:inside", "DTSTART:20260320T000000Z"), event("UID:outside", "DTSTART:20260410T000000Z"),
    event("UID:ended", "DTSTART:20260319T200000Z", "DTEND:20260320T000000Z"));
  assert.deepEqual(Array.from(parse(text, "a", from, until), e => e.start),
    ["2026-03-19T22:00:00.000Z", "2026-03-20T00:00:00.000Z"]);
});

test("keeps all-day boundaries and floating times in the viewer's local time", () => {
  const text = calendar(event("UID:day", "DTSTART;VALUE=DATE:20260329"),
    event("UID:floating", "DTSTART:20260329T090000"));
  const [day, floating] = parse(text, "a", from, until);
  assert.equal(day.allDay, true);
  assert.equal(day.start, new Date(2026, 2, 29).toISOString());
  assert.equal(day.end, new Date(2026, 2, 30).toISOString());
  assert.equal(floating.allDay, false);
  assert.equal(floating.start, new Date(2026, 2, 29, 9).toISOString());
});

test("expands IANA recurrences through DST and merges UTC exclusions, additions and exceptions", () => {
  const text = calendar(event("UID:series", "SUMMARY:Weekly meeting", "DTSTART;TZID=Europe/Berlin:20260320T090000",
    "RRULE:FREQ=WEEKLY;COUNT=4", "DURATION:PT1H", "EXDATE:20260327T080000Z", "RDATE:20260330T070000Z"),
    event("UID:series", "RECURRENCE-ID:20260403T070000Z", "DTSTART:20260403T123000Z", "SUMMARY:Moved"),
    event("UID:series", "RECURRENCE-ID:20260410T070000Z", "STATUS:CANCELLED"));
  const meetings = parse(text, "a", from, new Date("2026-04-12T00:00Z"));
  assert.deepEqual(Array.from(meetings, m => [m.subject, m.start]), [
    ["Weekly meeting", "2026-03-20T08:00:00.000Z"], ["Weekly meeting", "2026-03-30T07:00:00.000Z"],
    ["Moved", "2026-04-03T12:30:00.000Z"],
  ]);
  assert.equal(new Set(meetings.map(m => m.id)).size, 3);
  const [original] = parse(calendar(event("UID:series", "DTSTART;TZID=Europe/Berlin:20260403T090000",
    "RRULE:FREQ=DAILY;COUNT=1")), "a", from, until);
  assert.equal(original.id, meetings[2].id);
});

test("shows an instance moved into the window even when its original date is outside", () => {
  const text = calendar(event("UID:shift", "DTSTART:20260610T100000Z", "RRULE:FREQ=WEEKLY;COUNT=3"),
    event("UID:shift", "RECURRENCE-ID:20260617T100000Z", "DTSTART:20260322T100000Z"));
  assert.equal(parse(text, "a", from, until)[0].start, "2026-03-22T10:00:00.000Z");
});

test("resolves an embedded VTIMEZONE before IANA without leaking it to the next source", () => {
  const zone = ["BEGIN:VTIMEZONE", "TZID:Europe/Berlin", "BEGIN:STANDARD", "DTSTART:19700101T000000",
    "TZOFFSETFROM:+0300", "TZOFFSETTO:+0300", "END:STANDARD", "END:VTIMEZONE"].join("\r\n");
  const meeting = event("UID:custom", "DTSTART;TZID=Europe/Berlin:20260320T090000");
  assert.equal(parse(calendar(zone, meeting), "custom", from, until)[0].start, "2026-03-20T06:00:00.000Z");
  assert.equal(parse(calendar(meeting), "standard", from, until)[0].start, "2026-03-20T08:00:00.000Z");
});

test("handles ambiguous and nonexistent IANA wall times according to RFC 5545", () => {
  const text = calendar(event("UID:spring", "DTSTART;TZID=America/New_York:20260308T023000"),
    event("UID:fall", "DTSTART;TZID=America/New_York:20261101T013000"));
  assert.deepEqual(Array.from(parse(text, "a", new Date("2026-01-01"), new Date("2027-01-01")), e => e.start),
    ["2026-03-08T07:30:00.000Z", "2026-11-01T05:30:00.000Z"]);
});

test("preserves distinct meeting IDs and chooses the latest revision even if it appears first", () => {
  const text = calendar(event("UID:first", "SEQUENCE:1", "DTSTART:20260321T110000Z", "SUMMARY:Updated"),
    event("UID:first", "SEQUENCE:0", "DTSTART:20260321T100000Z", "SUMMARY:Old"),
    event("UID:second", "DTSTART:20260321T110000Z"), event("UID:cancel", "STATUS:CANCELLED"));
  const meetings = parse(text, "a", from, until);
  assert.equal(meetings.length, 2);
  assert.notEqual(meetings[0].id, meetings[1].id);
  assert.ok(meetings.every(m => m.start === "2026-03-21T11:00:00.000Z"));
  assert.ok(meetings.some(m => m.subject === "Updated"));
});

test("RDATE-only series include DTSTART, deduplicate dates, and keep explicit period lengths", () => {
  const text = calendar(event("UID:rdates", "DTSTART:20260321T080000Z", "DTEND:20260321T090000Z",
    "RDATE;VALUE=PERIOD:20260323T080000Z/20260323T110000Z", "EXDATE:20260322T080000Z",
    "RDATE:20260322T080000Z,20260323T080000Z"));
  assert.deepEqual(Array.from(parse(text, "a", from, until), e => [e.start, e.end]), [
    ["2026-03-21T08:00:00.000Z", "2026-03-21T09:00:00.000Z"],
    ["2026-03-23T08:00:00.000Z", "2026-03-23T11:00:00.000Z"],
  ]);
});

test("keeps elapsed period hours across the spring clock change", () => {
  const text = calendar(event("UID:period", "DTSTART;TZID=Europe/Berlin:20260321T090000",
    "RDATE;VALUE=PERIOD;TZID=Europe/Berlin:20260329T013000/PT2H"));
  const [, period] = parse(text, "a", from, until);
  assert.equal(period.start, "2026-03-29T00:30:00.000Z");
  assert.equal(period.end, "2026-03-29T02:30:00.000Z");
});

test("a large recurring title stays bounded when returned from the worker", () => {
  const text = calendar(event("UID:big-title", "SUMMARY:" + "c".repeat(1_000_000),
    "DTSTART:20260321T080000Z", "RRULE:FREQ=HOURLY;COUNT=300"));
  const meetings = parse(text, "a", from, until);
  assert.equal(meetings.length, 300);
  assert.ok(meetings.every(meeting => meeting.subject.length <= 1024));
});

test("rejects unreadable input, unknown zones and excessive expansions with safe errors", () => {
  for (const text of ["private-description", calendar(event("UID:a", "DTSTART;TZID=Private/Secret:20260321T090000")),
    calendar(event("UID:a", "DTSTART:20260338T100000Z")), calendar(event("UID:a", "DTSTART:20260321T090000Z",
      "RRULE:FREQ=SECONDLY;COUNT=100000"))]) {
    assert.throws(() => parse(text, "a", from, until),
      error => /calendar/i.test(error.message) && !/private|secret|20260338/i.test(error.message));
  }
});
