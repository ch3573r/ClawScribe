"use client";

// Links a recording to a Microsoft calendar event (title + invited attendees),
// for title prefill and attendee context in summaries. UI association, not
// meeting content — no DB column.
//
// Two slots:
//  - a "pending" selection for the *next* recording, in sessionStorage so it is
//    ephemeral (cleared when the app closes), and
//  - a per-meeting binding in localStorage, keyed by the saved meeting id. The
//    binding is created at record-save (when the real meeting id is known), NOT
//    lazily at summary time, so attendees can't attach to the wrong recording.
//
// Privacy: only the minimal title + invited-attendee names/emails are stored
// (no Teams join URL), and everything is cleared on Microsoft sign-out.

export interface MeetingCalendarLink {
  eventId: string;
  subject: string | null;
  attendees: MeetingCalendarAttendee[];
}

export interface MeetingCalendarAttendee {
  name: string | null;
  email: string | null;
  /** True when the user wants this invitee included in the meeting summary. */
  included: boolean;
}

const MEETING_PREFIX = "clawscribe.meetingCalendar.";
const PENDING_KEY = "clawscribe.pendingCalendar";
const ACTIVE_KEY = "clawscribe.activeRecordingCalendar";

function read(store: Storage | undefined, key: string): MeetingCalendarLink | null {
  try {
    const raw = store?.getItem(key);
    return raw ? normalizeLink(JSON.parse(raw) as MeetingCalendarLink) : null;
  } catch {
    return null;
  }
}

function normalizeLink(link: MeetingCalendarLink): MeetingCalendarLink {
  return {
    eventId: link.eventId,
    subject: link.subject ?? null,
    attendees: (link.attendees ?? []).map((a) => ({
      name: a.name ?? null,
      email: a.email ?? null,
      included: a.included !== false,
    })),
  };
}

function write(store: Storage | undefined, key: string, link: MeetingCalendarLink | null): void {
  try {
    if (!store) return;
    if (link) store.setItem(key, JSON.stringify(link));
    else store.removeItem(key);
  } catch {
    // Best-effort; the app works without the association.
  }
}

const session = (): Storage | undefined =>
  typeof window === "undefined" ? undefined : window.sessionStorage;
const local = (): Storage | undefined =>
  typeof window === "undefined" ? undefined : window.localStorage;

/** The calendar event chosen for the next recording (title prefill source). */
export function getPendingCalendar(): MeetingCalendarLink | null {
  return read(session(), PENDING_KEY);
}
export function setPendingCalendar(link: MeetingCalendarLink | null): void {
  write(session(), PENDING_KEY, link);
}
export function clearPendingCalendar(): void {
  write(session(), PENDING_KEY, null);
}

/**
 * Freeze `snapshot` (read once *before* the async record-start) as the active
 * recording's calendar event, so the title and the attendee binding always come
 * from the same event even if "Use for next recording" changes during startup.
 * Writing the snapshot (possibly null) also clears any stale active slot left by
 * a discarded recording. Pending is consumed only if it's still that same
 * snapshot, so a change made during startup (meant for the next recording) is
 * preserved.
 */
export function beginRecordingCalendar(snapshot: MeetingCalendarLink | null): void {
  write(session(), ACTIVE_KEY, snapshot);
  const pending = getPendingCalendar();
  if (snapshot && pending && pending.eventId === snapshot.eventId) {
    clearPendingCalendar();
  }
}

/** Consume (read + clear) the active recording's event, to bind to its meeting. */
export function takeActiveRecordingCalendar(): MeetingCalendarLink | null {
  const active = read(session(), ACTIVE_KEY);
  write(session(), ACTIVE_KEY, null);
  return active;
}

/** The calendar event bound to a specific saved meeting. */
export function getMeetingCalendar(meetingId: string): MeetingCalendarLink | null {
  if (!meetingId) return null;
  return read(local(), MEETING_PREFIX + meetingId);
}
export function setMeetingCalendar(
  meetingId: string,
  link: MeetingCalendarLink | null,
): void {
  if (!meetingId) return;
  write(local(), MEETING_PREFIX + meetingId, link);
}

/** Drop all calendar associations + the pending selection (on MS sign-out). */
export function clearAllCalendarLinks(): void {
  clearPendingCalendar();
  write(session(), ACTIVE_KEY, null);
  const store = local();
  if (!store) return;
  try {
    const keys: string[] = [];
    for (let i = 0; i < store.length; i++) {
      const k = store.key(i);
      if (k && k.startsWith(MEETING_PREFIX)) keys.push(k);
    }
    keys.forEach((k) => store.removeItem(k));
  } catch {
    // best-effort
  }
}

/** Human-readable attendee list for prompts/UI, capped for length. */
export function attendeeNames(link: MeetingCalendarLink, max = 25): string[] {
  return link.attendees
    .filter((a) => a.included !== false)
    .map((a) => (a.name?.trim() || a.email?.trim() || "").trim())
    .filter((s) => s.length > 0)
    .slice(0, max);
}

/** Markdown checklist for summary prompts, preserving checked-off absences. */
export function attendeeChecklist(link: MeetingCalendarLink, max = 25): string[] {
  return link.attendees
    .map((a) => ({
      label: (a.name?.trim() || a.email?.trim() || "").trim(),
      included: a.included !== false,
    }))
    .filter((a) => a.label.length > 0)
    .slice(0, max)
    .map((a) => `- [${a.included ? "x" : " "}] ${a.label}`);
}
