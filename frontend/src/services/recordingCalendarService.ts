import { getICalendarSnapshot, refreshICalendars } from "@/services/icalService";
import { microsoftExportService, type CalendarEvent } from "@/services/microsoftExportService";
import {
  matchOngoingCalendar, microsoftCalendarRevision, overlappingCalendarLinks,
  type MeetingCalendarLink, type OngoingCalendarMatch,
} from "@/lib/meetingCalendar";

const CALENDAR_START_BUDGET_MS = 1_250;
const CALENDAR_REVIEW_BUDGET_MS = 8_000;

/** Reads every connected calendar once and names the sources that could not be read. */
async function readCalendars(from: number, to: number, budgetMs: number) {
  let graph: CalendarEvent[] = [];
  let graphComplete = false;
  let icalComplete = false;
  const microsoftRevision = microsoftCalendarRevision;
  const graphWork = (async () => {
    const connection = await microsoftExportService.connectionStatus();
    if (connection.state === "connected" && !connection.unavailableExports?.includes("Calendar lookup")) {
      graph = await microsoftExportService.listCalendarEvents(
        new Date(from).toISOString(), new Date(to).toISOString(), true,
      );
    }
    graphComplete = connection.state !== "expired";
  })();
  const icalWork = refreshICalendars().then(() => { icalComplete = true; });
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      Promise.allSettled([graphWork, icalWork]),
      new Promise<void>(resolve => { timer = setTimeout(resolve, budgetMs); }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
  const ical = getICalendarSnapshot();
  const signedOut = microsoftRevision !== microsoftCalendarRevision;
  const unavailable = [
    ...(!graphComplete || signedOut ? ["Microsoft 365"] : []),
    ...(!icalComplete || ical.loading || ical.error || Object.keys(ical.issues).length > 0 ? ["iCalendar"] : []),
  ];
  return { events: [...ical.events, ...(signedOut ? [] : graph)], unavailable };
}

export async function findOngoingRecordingCalendar(now: number): Promise<OngoingCalendarMatch> {
  const { events, unavailable } = await readCalendars(now - 60_000, now + 60_000, CALENDAR_START_BUDGET_MS);
  return { ...matchOngoingCalendar(events, now, unavailable.length > 0), unavailable };
}

/** Events from the connected calendars that overlap a saved recording, for linking it afterwards. */
export async function findCalendarEventsBetween(from: number, to: number): Promise<{
  matches: MeetingCalendarLink[];
  unavailable: string[];
}> {
  const { events, unavailable } = await readCalendars(from, to, CALENDAR_REVIEW_BUDGET_MS);
  return { matches: overlappingCalendarLinks(events, from, to), unavailable };
}
