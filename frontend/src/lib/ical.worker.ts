import { ICalendarParseError, parseICalendar } from "./icalEvents";

self.onmessage = ({ data }: MessageEvent<{ text: string; sourceId: string; from: Date; until: Date }>) => {
  try {
    self.postMessage({ events: parseICalendar(data.text, data.sourceId, data.from, data.until) });
  } catch (error) {
    self.postMessage({ error: error instanceof ICalendarParseError ? error.message : "Could not parse the iCalendar data." });
  }
};
