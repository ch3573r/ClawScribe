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
  /** The person chose this event or saved its attendance. An automatic match stays unreviewed. */
  reviewed?: boolean;
  attendees: MeetingCalendarAttendee[];
}

/** Matches the core: any explicit choice is a review, including links saved before the flag existed. */
export function isReviewedCalendar(link: MeetingCalendarLink): boolean {
  return link.reviewed === true || link.attendees.some(person => person.attendance !== "unreviewed");
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
  status: "matched" | "upcoming" | "ambiguous" | "unavailable" | "none";
  matches: MeetingCalendarLink[];
  /** Calendar sources that could not be read, such as "Microsoft 365". */
  unavailable?: string[];
}

/** Timed, uncancelled events that overlap a window, such as the time of a saved recording. */
export function overlappingCalendarLinks(events: Invitation[], from: number, to: number): MeetingCalendarLink[] {
  const links = new Map<string, MeetingCalendarLink>();
  for (const event of events) {
    if (event.allDay || event.isCancelled || !event.start || !event.end) continue;
    const start = Date.parse(event.start);
    const end = Date.parse(event.end);
    if (!(start < end && start < to && end > from)) continue;
    const link = calendarLinkFromEvent(event);
    links.set(JSON.stringify([link.source, link.sourceId, link.eventId]), link);
  }
  return [...links.values()].sort((a, b) => (a.start ?? "").localeCompare(b.start ?? ""));
}

export const EARLY_JOIN_WINDOW_MS = 5 * 60_000;

export function matchOngoingCalendar(events: Invitation[], now: number, incomplete: boolean): OngoingCalendarMatch {
  const matches = overlappingCalendarLinks(events, now, now + EARLY_JOIN_WINDOW_MS);
  let status: OngoingCalendarMatch["status"] = "none";
  if (incomplete) status = "unavailable";
  else if (matches.length > 1) status = "ambiguous";
  else if (matches.length === 1) status = Date.parse(matches[0].start!) > now ? "upcoming" : "matched";
  return {
    status,
    matches,
  };
}

export const LEGACY_MEETING_CALENDAR_PREFIX = "clawscribe.meetingCalendar.";
export const PENDING_CALENDAR_CHANGED = "clawscribe-pending-calendar-changed";
export const ACTIVE_CALENDAR_CHANGED = "clawscribe-active-calendar-changed";
export const MICROSOFT_CALENDAR_CLEARED = "clawscribe-microsoft-calendar-cleared";
const PENDING_KEY = "clawscribe.pendingCalendar";
const SKIP_VALUE = '"skip"';
const CHOICE_GRACE_MS = 5 * 60_000;
const CHOICE_FALLBACK_MS = 60 * 60_000;

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
      ...(link.reviewed === true ? { reviewed: true } : {}),
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
  return getNextCalendarChoice().link;
}

export function getNextCalendarChoice(now = Date.now()) {
  const empty = { link: null, skip: false, version: selectionVersion, expiresAt: 0 };
  const raw = sessionValue(PENDING_KEY);
  if (!raw) return empty;
  if (raw === SKIP_VALUE) return { ...empty, skip: true };
  try {
    const stored = JSON.parse(raw);
    const link = parseCalendarLink(raw);
    // Old choices lacked an end time. Only a dated invitation can still be current.
    const expiresAt = stored?.expiresAt ?? (Date.parse(link?.start ?? "") + CHOICE_FALLBACK_MS);
    if (!link || typeof expiresAt !== "number" || !Number.isFinite(expiresAt) || now >= expiresAt) {
      window.sessionStorage.removeItem(PENDING_KEY);
      return empty;
    }
    return { link, skip: false, version: selectionVersion, expiresAt };
  } catch { return empty; }
}

export function isNextCalendarSkipped(): boolean {
  return getNextCalendarChoice().skip;
}

export function setPendingCalendar(link: MeetingCalendarLink | null, skip = false, end?: string | null): boolean {
  try {
    const store = window.sessionStorage;
    if (skip) store.setItem(PENDING_KEY, SKIP_VALUE);
    else if (link) {
      const endTime = Date.parse(end ?? "");
      const startTime = Date.parse(link.start ?? "");
      const expiresAt = Number.isFinite(endTime) ? endTime + CHOICE_GRACE_MS
        : (Number.isFinite(startTime) ? startTime : Date.now()) + CHOICE_FALLBACK_MS;
      store.setItem(PENDING_KEY, JSON.stringify({ ...link, expiresAt }));
    } else store.removeItem(PENDING_KEY);
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
  sessionId: string;
  startedAt?: number;
  revision: number;
  link: MeetingCalendarLink | null;
}

// Suggestions are transient. The choice and attendance live in the core.
const activeRecordings = new Map<string, RecordingCalendar>();
const stoppedSessions = new Set<string>();

export function getActiveRecordingCalendar(sessionId: string): RecordingCalendar | null {
  return activeRecordings.get(sessionId) ?? null;
}

export function setActiveRecordingCalendar(active: RecordingCalendar): boolean {
  if (stoppedSessions.has(active.sessionId)) return false;
  activeRecordings.set(active.sessionId, active);
  window.dispatchEvent(new Event(ACTIVE_CALENDAR_CHANGED));
  return true;
}

export function clearActiveRecordingCalendar(sessionId: string): void {
  stoppedSessions.add(sessionId);
  if (stoppedSessions.size > 32) stoppedSessions.delete(stoppedSessions.values().next().value!);
  if (activeRecordings.delete(sessionId)) window.dispatchEvent(new Event(ACTIVE_CALENDAR_CHANGED));
}

export function consumeNextCalendarChoice(choice: ReturnType<typeof getNextCalendarChoice>): void {
  // A choice changed during startup belongs to the next recording.
  if (selectionVersion === choice.version && (choice.link || choice.skip)) clearPendingCalendar();
}

/**
 * A Microsoft sign-out ends the account connection. It clears what came from the
 * account and was never confirmed: current candidates, an unreviewed selection
 * and unmigrated unreviewed copies. A link the person reviewed is their own local
 * record and stays with the meeting.
 */
export function clearMicrosoftCalendarLinks(): void {
  microsoftCalendarRevision++;
  const pending = getPendingCalendar();
  if (pending?.source === "microsoft" && !isReviewedCalendar(pending)) clearPendingCalendar();
  for (const active of activeRecordings.values()) {
    if (active.link?.source === "microsoft" || active.matches.some(link => link.source === "microsoft")) {
      setActiveRecordingCalendar({
        ...active,
        link: active.link?.source === "microsoft" && !isReviewedCalendar(active.link) ? null : active.link,
        matches: active.matches.filter(link => link.source !== "microsoft"),
      });
    }
  }
  try {
    const store = window.localStorage;
    const keys = Array.from({ length: store.length }, (_, index) => store.key(index));
    for (const key of keys) {
      if (!key?.startsWith(LEGACY_MEETING_CALENDAR_PREFIX)) continue;
      const legacy = parseCalendarLink(store.getItem(key));
      if (legacy?.source === "microsoft" && !isReviewedCalendar(legacy)) store.removeItem(key);
    }
  } catch { /* Best-effort cleanup of superseded webview storage. */ }
  window.dispatchEvent(new Event(MICROSOFT_CALENDAR_CLEARED));
}
