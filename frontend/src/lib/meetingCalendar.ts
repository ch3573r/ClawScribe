"use client";

export type Attendance = "unreviewed" | "present" | "absent";

export interface MeetingCalendarAttendee {
  name: string | null;
  email: string | null;
  attendance: Attendance;
}

export interface MeetingCalendarLink {
  source: "microsoft" | "ical";
  sourceId?: string | null;
  seriesId?: string | null;
  eventId: string;
  subject: string | null;
  start?: string | null;
  attendees: MeetingCalendarAttendee[];
}

interface Invitation {
  id: string;
  subject: string | null;
  start: string | null;
  end?: string | null;
  sourceId?: string | null;
  seriesId?: string | null;
  allDay?: boolean;
  isCancelled?: boolean;
  organizer?: { name: string | null; email: string | null } | null;
  attendees: { name: string | null; email: string | null }[];
}

export function calendarLinkFromEvent(event: Invitation): MeetingCalendarLink {
  const attendees = new Map<string, MeetingCalendarAttendee>();
  for (const person of [...event.attendees, event.organizer]) {
    if (!person) continue;
    const name = person.name?.trim() || null;
    const email = person.email?.trim() || null;
    const key = email ? `email:${email.toLowerCase()}` : name ? `name:${name}` : null;
    if (key && !attendees.has(key)) attendees.set(key, { name, email, attendance: "unreviewed" });
    if (attendees.size === 500) break;
  }
  return {
    source: event.sourceId ? "ical" : "microsoft",
    ...(event.sourceId ? { sourceId: event.sourceId } : {}),
    ...(event.seriesId ? { seriesId: event.seriesId } : {}),
    eventId: event.id,
    subject: event.subject,
    start: event.start,
    attendees: [...attendees.values()],
  };
}

export function sameCalendarEvent(a: MeetingCalendarLink | null, b: MeetingCalendarLink): boolean {
  return a?.source === b.source && (a.sourceId ?? null) === (b.sourceId ?? null) && a.eventId === b.eventId;
}

export interface OngoingCalendarMatch {
  status: "matched" | "ambiguous" | "unavailable" | "none";
  matches: MeetingCalendarLink[];
}

export function matchOngoingCalendar(events: Invitation[], now: number, incomplete: boolean): OngoingCalendarMatch {
  const matches = new Map<string, MeetingCalendarLink>();
  for (const event of events) {
    if (event.allDay || event.isCancelled || !event.start || !event.end ||
        !(Date.parse(event.start) <= now && now < Date.parse(event.end))) continue;
    const link = calendarLinkFromEvent(event);
    matches.set(JSON.stringify([link.source, link.sourceId, link.eventId]), link);
  }
  return {
    status: incomplete ? "unavailable" : matches.size > 1 ? "ambiguous" : matches.size === 1 ? "matched" : "none",
    matches: [...matches.values()],
  };
}

export const LEGACY_MEETING_CALENDAR_PREFIX = "clawscribe.meetingCalendar.";
export const PENDING_CALENDAR_CHANGED = "clawscribe-pending-calendar-changed";
export const ACTIVE_CALENDAR_CHANGED = "clawscribe-active-calendar-changed";
const PENDING_KEY = "clawscribe.pendingCalendar";
const ACTIVE_KEY = "clawscribe.recordingCalendar";
const SKIP_VALUE = '"skip"';

type LegacyLink = Omit<MeetingCalendarLink, "source" | "attendees"> & {
  source?: MeetingCalendarLink["source"];
  attendees?: (Omit<MeetingCalendarAttendee, "attendance"> & { attendance?: Attendance; included?: boolean })[];
};

export function parseCalendarLink(raw: string | null): MeetingCalendarLink | null {
  try {
    if (!raw) return null;
    const link = JSON.parse(raw) as LegacyLink | null;
    if (!link || typeof link.eventId !== "string" || !link.eventId.trim()) return null;
    return {
      source: link.source === "ical" ? "ical" : "microsoft",
      ...(link.source === "ical" ? { sourceId: link.sourceId } : {}),
      ...(link.seriesId ? { seriesId: link.seriesId } : {}),
      eventId: link.eventId,
      subject: link.subject ?? null,
      start: link.start ?? null,
      attendees: (link.attendees ?? []).map(person => ({
        name: person.name ?? null,
        email: person.email ?? null,
        // The legacy checklist defaulted to checked. That is not user confirmation.
        attendance: person.attendance === "present" ? "present" :
          person.attendance === "absent" || person.included === false ? "absent" : "unreviewed",
      })),
    };
  } catch {
    return null;
  }
}

function sessionValue(key: string): string | null {
  try { return window.sessionStorage.getItem(key); } catch { return null; }
}

let selectionVersion = 0;
export let microsoftCalendarRevision = 0;

export function getPendingCalendar(): MeetingCalendarLink | null {
  return parseCalendarLink(sessionValue(PENDING_KEY));
}

export function isNextCalendarSkipped(): boolean {
  return sessionValue(PENDING_KEY) === SKIP_VALUE;
}

export function getNextCalendarChoice() {
  return { link: getPendingCalendar(), skip: isNextCalendarSkipped(), version: selectionVersion };
}

export function setPendingCalendar(link: MeetingCalendarLink | null, skip = false): boolean {
  try {
    const store = window.sessionStorage;
    if (skip) store.setItem(PENDING_KEY, SKIP_VALUE);
    else if (link) store.setItem(PENDING_KEY, JSON.stringify(link));
    else store.removeItem(PENDING_KEY);
  } catch {
    return false;
  }
  selectionVersion++;
  window.dispatchEvent(new Event(PENDING_CALENDAR_CHANGED));
  return true;
}

export function clearPendingCalendar(): void {
  setPendingCalendar(null);
}

export interface RecordingCalendar extends OngoingCalendarMatch {
  link: MeetingCalendarLink | null;
  fallbackTitle: string;
}

let activeRecording: RecordingCalendar | null | undefined;

export function getActiveRecordingCalendar(): RecordingCalendar | null {
  if (activeRecording !== undefined) return activeRecording;
  try {
    const raw = sessionValue(ACTIVE_KEY);
    return raw ? JSON.parse(raw) as RecordingCalendar : null;
  } catch { return null; }
}

export function setActiveRecordingCalendar(active: RecordingCalendar | null) {
  activeRecording = active;
  try {
    if (active) window.sessionStorage.setItem(ACTIVE_KEY, JSON.stringify(active));
    else window.sessionStorage.removeItem(ACTIVE_KEY);
  } catch { /* The current session still has the in-memory snapshot. */ }
  window.dispatchEvent(new Event(ACTIVE_CALENDAR_CHANGED));
}

export function beginRecordingCalendar(active: RecordingCalendar, choice: ReturnType<typeof getNextCalendarChoice>) {
  setActiveRecordingCalendar(active);
  // A choice changed during lookup or startup belongs to the next recording.
  if (selectionVersion === choice.version && (choice.link || choice.skip)) clearPendingCalendar();
}

/** A Microsoft sign-out clears unfinished and unmigrated Microsoft associations. */
export function clearMicrosoftCalendarLinks(): void {
  microsoftCalendarRevision++;
  if (getPendingCalendar()?.source === "microsoft") clearPendingCalendar();
  const active = getActiveRecordingCalendar();
  if (active && (active.link?.source === "microsoft" || active.matches.some(link => link.source === "microsoft"))) {
    setActiveRecordingCalendar({
      ...active,
      link: active.link?.source === "microsoft" ? null : active.link,
      matches: active.matches.filter(link => link.source !== "microsoft"),
    });
  }
  try {
    const store = window.localStorage;
    const keys = Array.from({ length: store.length }, (_, index) => store.key(index));
    for (const key of keys) {
      if (key?.startsWith(LEGACY_MEETING_CALENDAR_PREFIX) && parseCalendarLink(store.getItem(key))?.source === "microsoft") {
        store.removeItem(key);
      }
    }
  } catch { /* Best-effort cleanup of superseded webview storage. */ }
  window.dispatchEvent(new Event("clawscribe-microsoft-calendar-cleared"));
}
