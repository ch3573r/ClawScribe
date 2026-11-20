import type { ICalendarEvent } from "./icalEvents";

export class CalendarReadError extends Error {}

export function parseCalendarInWorker(text: string, sourceId: string, from: Date, until: Date): Promise<ICalendarEvent[]> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL("./ical.worker.ts", import.meta.url));
    const finish = () => { clearTimeout(timeout); worker.terminate(); };
    const fail = (text: string) => { finish(); reject(new CalendarReadError(text)); };
    const failed = () => fail("Could not read the calendar.");
    const timeout = setTimeout(() => fail("This calendar took too long to expand. Try a smaller calendar."), 8_000);
    worker.onerror = failed;
    worker.onmessageerror = failed;
    worker.onmessage = ({ data }: MessageEvent<{ events?: ICalendarEvent[]; error?: string }>) => {
      if (typeof data?.error === "string") { fail(data.error); return; }
      if (!Array.isArray(data?.events)) { failed(); return; }
      finish();
      resolve(data.events);
    };
    try { worker.postMessage({ text, sourceId, from, until }); }
    catch { failed(); }
  });
}
