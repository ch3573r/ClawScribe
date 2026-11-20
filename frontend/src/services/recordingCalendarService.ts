import { getICalendarSnapshot, refreshICalendars } from "@/services/icalService";
import { microsoftExportService, type CalendarEvent } from "@/services/microsoftExportService";
import {
  matchOngoingCalendar, microsoftCalendarRevision, type OngoingCalendarMatch,
} from "@/lib/meetingCalendar";

const CALENDAR_START_BUDGET_MS = 1_250;

export async function findOngoingRecordingCalendar(now: number): Promise<OngoingCalendarMatch> {
  let graph: CalendarEvent[] = [];
  let graphComplete = false;
  let icalComplete = false;
  const microsoftRevision = microsoftCalendarRevision;
  const graphWork = (async () => {
    const connection = await microsoftExportService.connectionStatus();
    if (connection.state === "connected" && !connection.unavailableExports?.includes("Calendar lookup")) {
      graph = await microsoftExportService.listCalendarEvents(
        new Date(now - 60_000).toISOString(), new Date(now + 60_000).toISOString(), true,
      );
    }
    graphComplete = connection.state !== "expired";
  })();
  const icalWork = refreshICalendars().then(() => { icalComplete = true; });
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      Promise.allSettled([graphWork, icalWork]),
      new Promise<void>(resolve => { timer = setTimeout(resolve, CALENDAR_START_BUDGET_MS); }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
  const ical = getICalendarSnapshot();
  const signedOut = microsoftRevision !== microsoftCalendarRevision;
  const incomplete = !graphComplete || !icalComplete || ical.loading || Boolean(ical.error) || Object.keys(ical.issues).length > 0;
  return matchOngoingCalendar([...ical.events, ...(signedOut ? [] : graph)], now, incomplete || signedOut);
}
