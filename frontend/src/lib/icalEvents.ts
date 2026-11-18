import ICAL from "ical.js";
import type { CalendarAttendee, CalendarEvent } from "@/services/microsoftExportService";

export interface ICalendarEvent extends CalendarEvent {
  start: string;
  end: string;
  allDay: boolean;
}

const errors = {
  invalid: "The calendar contains invalid iCalendar data.",
  unsupported: "The calendar uses an unsupported time zone or recurrence format.",
  size: "The calendar exceeds the 2 MiB size limit.",
  complexity: "The calendar contains too many events or recurrences to display.",
} as const;

export class ICalendarParseError extends Error {
  constructor(reason: keyof typeof errors) {
    super(errors[reason]);
    this.name = "ICalendarParseError";
  }
}

const MAX_BYTES = 2 * 1024 * 1024;
const DAY = 86_400_000;

// ical.js resolves embedded VTIMEZONEs within this component tree. For feeds
// that only supply an IANA name, resolve the offset with the platform tzdata.
// The cache belongs to this one calendar, including its embedded definitions.
class Calendar extends ICAL.Component {
  private zones = new Map<string, ICAL.Timezone>();

  getTimeZoneByID(id: string): ICAL.Timezone {
    const known = this.zones.get(id) ?? super.getTimeZoneByID(id);
    if (known) return known;
    let formatter: Intl.DateTimeFormat;
    try {
      formatter = new Intl.DateTimeFormat("en-US", { timeZone: id, timeZoneName: "longOffset" });
    } catch {
      throw new ICalendarParseError("unsupported");
    }
    const offsetAt = (instant: number) => {
      const name = formatter.formatToParts(instant).find(p => p.type === "timeZoneName")?.value;
      if (name === "GMT") return 0;
      const offset = /^GMT([+-])(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(name ?? "");
      if (!offset) throw new ICalendarParseError("unsupported");
      const seconds = Number(offset[2]) * 3600 + Number(offset[3]) * 60 + Number(offset[4] ?? 0);
      return offset[1] === "-" ? -seconds : seconds;
    };
    const zone = new ICAL.Timezone({ tzid: id });
    zone.utcOffset = t => {
      const wall = Date.UTC(t.year, t.month - 1, t.day, t.hour, t.minute, t.second);
      // RFC 5545 uses the earlier occurrence in a fold and the pre-gap offset
      // for a nonexistent clock time. Test that candidate first.
      const before = offsetAt(wall - DAY);
      if (offsetAt(wall - before * 1000) === before) return before;
      const after = offsetAt(wall + DAY);
      return offsetAt(wall - after * 1000) === after ? after : before;
    };
    this.zones.set(id, zone);
    return zone;
  }
}

function checkedDate(value: unknown, raw: unknown): ICAL.Time {
  // Reject out-of-range dates that the library would otherwise normalize.
  if (!(value instanceof ICAL.Time) || value.toString() !== raw) {
    throw new ICalendarParseError("invalid");
  }
  return value;
}

function dateValue(property: ICAL.Property | null): ICAL.Time {
  if (!property) throw new ICalendarParseError("invalid");
  return checkedDate(property.getFirstValue(), property.toJSON()[3]);
}

function instanceKey(date: ICAL.Time): string {
  return date.isDate || date.zone === ICAL.Timezone.localTimezone
    ? date.toString()
    : String(date.toUnixTime());
}

// A stable, opaque grouping key prevents raw vendor UIDs (which can be
// emails or invitation URLs) from entering a saved meeting association.
function seriesKey(sourceId: string, uid: string): string {
  let hash = BigInt("0xcbf29ce484222325");
  const prime = BigInt("0x100000001b3");
  const input = JSON.stringify([sourceId, uid]);
  for (let i = 0; i < input.length; i++) {
    hash = BigInt.asUintN(64, (hash ^ BigInt(input.charCodeAt(i))) * prime);
  }
  return "ics:" + hash.toString(16);
}

function singleLine(value: unknown, limit: number): string | null {
  if (typeof value !== "string") return null;
  const cleaned = value.replace(/[\u0000-\u001f\u007f]+/g, " ").trim();
  return cleaned.length > limit ? cleaned.slice(0, limit - 1) + "…" : cleaned || null;
}

function attendee(property: ICAL.Property | null): CalendarAttendee | null {
  if (!property) return null;
  const name = singleLine(property.getParameter("cn"), 256);
  const address = property.getFirstValue();
  const email = typeof address === "string" && address.length <= 261 && /^mailto:[^\s@?&#]+@[^\s@?&#/:]+$/i.test(address)
    ? address.slice(7) : null;
  return name || email ? { name, email } : null;
}

type MeetingFields = Pick<ICalendarEvent, "subject" | "organizer" | "attendees" | "isOnlineMeeting" | "joinUrl">;

function fields(event: ICAL.Component, fallback?: MeetingFields): MeetingFields {
  return {
    subject: singleLine(event.getFirstPropertyValue("summary"), 1024) ?? fallback?.subject ?? null,
    organizer: attendee(event.getFirstProperty("organizer")) ?? fallback?.organizer ?? null,
    attendees: event.hasProperty("attendee")
      ? event.getAllProperties("attendee").map(attendee).filter((person): person is CalendarAttendee => person !== null)
      : fallback?.attendees ?? [],
    isOnlineMeeting: false,
    joinUrl: null,
  };
}

function occurrenceEnd(start: ICAL.Time, end: ICAL.Time | null, duration: unknown) {
  let days = start.isDate ? 1 : 0;
  let milliseconds = 0;
  if (end) {
    if (duration || end.isDate !== start.isDate) throw new ICalendarParseError("invalid");
    if (start.isDate) days = end.subtractDate(start).toSeconds() / 86400;
    else milliseconds = end.toJSDate().getTime() - start.toJSDate().getTime();
  } else if (duration) {
    if (!(duration instanceof ICAL.Duration) || duration.isNegative) throw new ICalendarParseError("invalid");
    days = duration.days + duration.weeks * 7;
    milliseconds = (duration.hours * 3600 + duration.minutes * 60 + duration.seconds) * 1000;
  }
  if (days < 0 || milliseconds < 0 || (start.isDate && milliseconds)) throw new ICalendarParseError("invalid");
  // Calendar days are nominal across DST. Timed DTEND supplies an exact elapsed duration.
  return (occurrence: ICAL.Time) => {
    const lastDay = days ? occurrence.clone().adjust(days, 0, 0, 0) : occurrence;
    return lastDay.toJSDate().getTime() + milliseconds;
  };
}

function eventDuration(event: ICAL.Component, start: ICAL.Time) {
  const end = event.getFirstProperty("dtend");
  return occurrenceEnd(start, end && dateValue(end), event.getFirstPropertyValue("duration"));
}

function additionalDates(event: ICAL.Component) {
  const dates = new Map<string, { date: ICAL.Time; end?: number }>();
  for (const property of event.getAllProperties("rdate")) {
    property.getValues().forEach((value, i) => {
      const raw = property.toJSON()[3 + i];
      if (value instanceof ICAL.Period) {
        const date = checkedDate(value.start, raw[0]);
        const end = occurrenceEnd(date, value.end && checkedDate(value.end, raw[1]), value.duration)(date);
        dates.set(instanceKey(date), { date, end });
      } else {
        const date = checkedDate(value, raw);
        const key = instanceKey(date);
        if (!dates.has(key)) dates.set(key, { date });
      }
    });
  }
  return dates;
}

function cancelled(event: ICAL.Component) {
  return String(event.getFirstPropertyValue("status")).toUpperCase() === "CANCELLED";
}

function groupEvents(calendar: Calendar) {
  const series = new Map<string, Map<string, ICAL.Component>>();
  for (const event of calendar.getAllSubcomponents("vevent")) {
    const uid = event.getFirstPropertyValue("uid");
    if (typeof uid !== "string" || !uid.trim()) throw new ICalendarParseError("invalid");
    const recurrence = event.getFirstProperty("recurrence-id");
    if (recurrence?.getParameter("range") || event.hasProperty("exrule")) throw new ICalendarParseError("unsupported");
    const key = recurrence ? instanceKey(dateValue(recurrence)) : "master";
    let group = series.get(uid);
    if (!group) {
      group = new Map();
      series.set(uid, group);
    }
    const previous = group.get(key);
    if (!previous || Number(event.getFirstPropertyValue("sequence")) >= Number(previous.getFirstPropertyValue("sequence"))) {
      group.set(key, event);
    }
  }
  return series;
}

function expand(calendar: Calendar, sourceId: string, from: number, until: number) {
  const meetings = new Map<string, ICalendarEvent>();
  let steps = 0;
  for (const [uid, group] of groupEvents(calendar)) {
    const master = group.get("master");
    if (master && cancelled(master)) continue;
    const prefix = seriesKey(sourceId, uid);
    const add = (key: string, date: ICAL.Time, end: number, metadata: MeetingFields) => {
      const start = date.toJSDate();
      if (+start >= until || (end > +start ? end <= from : +start < from)) return;
      const id = prefix + ":" + key;
      meetings.set(id, { ...metadata, id, allDay: date.isDate, start: start.toISOString(), end: new Date(end).toISOString() });
      if (meetings.size > 2000) throw new ICalendarParseError("complexity");
    };
    // Materialize detached instances independently. A moved event may enter
    // this window even when its original recurrence is beyond the upper bound.
    const metadata = master ? fields(master) : undefined;
    for (const [key, event] of group) {
      if (key === "master" || cancelled(event)) continue;
      const date = dateValue(event.getFirstProperty("dtstart"));
      add(key, date, eventDuration(event, date)(date), fields(event, metadata));
    }
    if (!master || !metadata) continue;
    const start = dateValue(master.getFirstProperty("dtstart"));
    const end = eventDuration(master, start);
    const excluded = new Set<string>();
    for (const property of master.getAllProperties("exdate")) {
      property.getValues().forEach((value, i) => excluded.add(instanceKey(checkedDate(value, property.toJSON()[3 + i]))));
    }
    const extra = additionalDates(master);
    const recurring = master.hasProperty("rrule") || master.hasProperty("rdate");
    const include = (date: ICAL.Time) => {
      const key = instanceKey(date);
      if (!group.has(key) && !excluded.has(key)) {
        add(recurring ? key : "single", date, extra.get(key)?.end ?? end(date), metadata);
      }
    };
    include(start);
    for (const property of master.getAllProperties("rrule")) {
      const rule = property.getFirstValue();
      if (!(rule instanceof ICAL.Recur) || !rule.freq) throw new ICalendarParseError("invalid");
      const iterator = rule.iterator(start);
      for (let date = iterator.next(); date; date = iterator.next()) {
        if (++steps > 50_000) throw new ICalendarParseError("complexity");
        if (+date.toJSDate() >= until) break;
        include(date);
      }
    }
    for (const { date } of extra.values()) include(date);
  }
  return Array.from(meetings.values()).sort((a, b) => a.start.localeCompare(b.start) || a.id.localeCompare(b.id));
}

/** Read only VEVENT metadata for occurrences overlapping the half-open window. */
export function parseICalendar(text: string, sourceId: string, from: Date, until: Date): ICalendarEvent[] {
  try {
    if (text.length > MAX_BYTES || new globalThis.TextEncoder().encode(text).byteLength > MAX_BYTES) {
      throw new ICalendarParseError("size");
    }
    if (!(+from < +until)) throw new ICalendarParseError("invalid");
    const calendar = new Calendar(ICAL.parse(text.trim()));
    if (calendar.name !== "vcalendar" || calendar.getFirstPropertyValue("version") !== "2.0") {
      throw new ICalendarParseError("invalid");
    }
    if (String(calendar.getFirstPropertyValue("method")).toUpperCase() === "CANCEL") return [];
    return expand(calendar, sourceId, +from, +until);
  } catch (error) {
    // Library parser errors may quote the entire offending line.
    if (error instanceof ICalendarParseError) throw error;
    throw new ICalendarParseError("invalid");
  }
}
