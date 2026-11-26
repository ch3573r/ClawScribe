import { getICalendarSnapshot, readICalendarsBetween, refreshICalendars, type ICalendarEvent } from "@/services/icalService";
import { microsoftExportService, type CalendarEvent } from "@/services/microsoftExportService";
import {
  EARLY_JOIN_WINDOW_MS, getActiveRecordingCalendar, matchOngoingCalendar, microsoftCalendarRevision, overlappingCalendarLinks, setActiveRecordingCalendar,
  type MeetingCalendarLink, type OngoingCalendarMatch, type RecordingCalendar,
} from "@/lib/meetingCalendar";
import { invoke } from "@tauri-apps/api/core";

const CALENDAR_START_BUDGET_MS = 1_250;
const CALENDAR_REVIEW_BUDGET_MS = 8_000;

export function readRecordingCalendar(sessionId: string): Promise<{ link: MeetingCalendarLink | null; revision: number }> {
  return invoke("get_active_recording_calendar", { sessionId });
}

export function writeRecordingCalendar(sessionId: string, revision: number, link: MeetingCalendarLink | null): Promise<number> {
  return invoke("set_active_recording_calendar", { sessionId, revision, link });
}

export async function resolveRecordingCalendar(
  initial: RecordingCalendar,
  now: number,
  microsoftRevision: number,
): Promise<void> {
  const match = await findOngoingRecordingCalendar(now).catch(() => ({ status: "unavailable" as const, matches: [] }));
  if (getActiveRecordingCalendar(initial.sessionId) !== initial) return;
  const signedOut = microsoftCalendarRevision !== microsoftRevision;
  const matches = signedOut ? match.matches.filter(link => link.source !== "microsoft") : match.matches;
  let status = signedOut ? "unavailable" as const : match.status;
  let link = status === "matched" ? matches[0] : null;
  let revision = initial.revision;
  if (link) {
    try {
      revision = await writeRecordingCalendar(initial.sessionId, revision, link);
    } catch {
      link = null;
      status = "unavailable";
    }
  }
  if (getActiveRecordingCalendar(initial.sessionId) === initial &&
      (signedOut || microsoftCalendarRevision === microsoftRevision)) {
    setActiveRecordingCalendar({ ...initial, ...match, status, matches, link, revision });
  }
}

/** Reads every connected calendar once and names the sources that could not be read. */
async function readCalendars(from: number, to: number, budgetMs: number, savedDate = false) {
  let graph: CalendarEvent[] = [];
  let graphComplete = false;
  let icalComplete = false;
  const rangeEvents: ICalendarEvent[] = [];
  let rangeCurrent = () => true;
  const controller = new AbortController();
  const microsoftRevision = microsoftCalendarRevision;
  const graphWork = (async () => {
    const connection = await microsoftExportService.connectionStatus();
    if (connection.state === "connected" && !connection.unavailableExports?.includes("Calendar lookup")) {
      graph = await microsoftExportService.listCalendarEvents(
        new Date(from).toISOString(), new Date(to).toISOString(), true,
      );
      graphComplete = true;
    } else {
      graphComplete = connection.state === "not_connected";
    }
  })();
  const icalWork = savedDate
    ? readICalendarsBetween(new Date(from), new Date(to), controller.signal, (events, isCurrent) => {
      rangeEvents.push(...events);
      rangeCurrent = isCurrent;
    }).then(complete => { icalComplete = complete; })
    : refreshICalendars().then(() => { icalComplete = true; });
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      Promise.allSettled([graphWork, icalWork]),
      new Promise<void>(resolve => { timer = setTimeout(resolve, budgetMs); }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
    controller.abort();
  }
  const ical = getICalendarSnapshot();
  const signedOut = microsoftRevision !== microsoftCalendarRevision;
  const unavailable = [
    ...(!graphComplete || signedOut ? ["Microsoft 365"] : []),
    ...(!icalComplete || (savedDate ? !rangeCurrent() : ical.loading || ical.error || Object.keys(ical.issues).length > 0) ? ["iCalendar"] : []),
  ];
  const icalEvents = savedDate ? rangeCurrent() ? rangeEvents : [] : ical.events;
  return { events: [...icalEvents, ...(signedOut ? [] : graph)], unavailable };
}

export async function findOngoingRecordingCalendar(now: number): Promise<OngoingCalendarMatch> {
  const { events, unavailable } = await readCalendars(now - 60_000, now + EARLY_JOIN_WINDOW_MS, CALENDAR_START_BUDGET_MS);
  return { ...matchOngoingCalendar(events, now, unavailable.length > 0), unavailable };
}

/** Events from the connected calendars that overlap a saved recording, for linking it afterwards. */
export async function findCalendarEventsBetween(from: number, to: number, savedDate = false): Promise<{
  matches: MeetingCalendarLink[];
  unavailable: string[];
}> {
  const { events, unavailable } = await readCalendars(from, to, CALENDAR_REVIEW_BUDGET_MS, savedDate);
  return { matches: overlappingCalendarLinks(events, from, to), unavailable };
}
