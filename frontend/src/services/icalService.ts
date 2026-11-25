"use client";

import { invoke } from "@tauri-apps/api/core";
import { CalendarReadError, parseCalendarInWorker } from "@/lib/icalWorkerClient";
import { clearPendingCalendar, getPendingCalendar } from "@/lib/meetingCalendar";
import type { ICalendarEvent as ParsedEvent } from "@/lib/icalEvents";

export interface ICalendarSource {
  id: string;
  name: string;
  kind: "file" | "subscription";
}

export interface ICalendarEvent extends ParsedEvent {
  sourceId: string;
}

interface Snapshot {
  sources: ICalendarSource[];
  events: ICalendarEvent[];
  issues: Record<string, string>;
  loading: boolean;
  error: string | null;
}

export const ICAL_REFRESH_MS = 5 * 60_000;
const FILE_LIMIT = 16 * 1024 * 1024;
const empty: Snapshot = { sources: [], events: [], issues: {}, loading: false, error: null };
let current = empty;
let generation = 0;
let lastChecked = 0;
let inflight: Promise<void> | null = null;
const listeners = new Set<() => void>();
export const getICalendarSnapshot = () => current;
export const getICalendarServerSnapshot = () => empty;
export function subscribeToICalendars(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}
function publish(value: Snapshot) {
  current = value;
  listeners.forEach(listener => listener());
}

// IPC and provider errors may include a private calendar URL. Display only known messages.
const safeErrors = new Set([
  "Enter a calendar name of 1 to 80 characters.",
  "Use an HTTPS or webcal calendar URL without a username, password, or fragment.",
  "Choose an iCalendar file or feed in UTF-8 format.",
  "Calendar files and feeds must be 16 MiB or smaller.",
  "This calendar source has already been added.",
  "You can add up to 32 calendar sources.",
  "This calendar source no longer exists.",
  "Could not read or save calendar sources. Check access to app storage.",
  "Could not access the protected calendar URL. Check your OS credential store.",
  "Could not download the calendar. Check the subscription link and try again.",
  "The calendar redirected to another address. Use the final HTTPS subscription URL.",
]);
function messageFor(error: unknown, fallback: string) {
  if (error instanceof CalendarReadError) return error.message;
  return typeof error === "string" && safeErrors.has(error) ? error : fallback;
}

function weekFrom(start: Date) {
  const end = new Date(start);
  end.setDate(end.getDate() + 7);
  return end;
}

async function readSource(source: ICalendarSource, from: Date, to: Date, force: boolean, isCurrent: () => boolean) {
  const text = await invoke<string>("read_ical_source", { id: source.id, force });
  if (!isCurrent()) return [];
  const parsed = await parseCalendarInWorker(text, source.id, from, to);
  return parsed.map(event => ({ ...event, sourceId: source.id }));
}

/** Read a saved meeting's date without replacing the rolling upcoming snapshot. */
export async function readICalendarsBetween(
  from: Date, to: Date, signal: AbortSignal,
  onEvents: (events: ICalendarEvent[], isCurrent: () => boolean) => void,
) {
  const version = generation;
  const isCurrent = () => version === generation;
  const active = () => !signal.aborted && isCurrent();
  const sources = await invoke<ICalendarSource[]>("list_ical_sources");
  const queue = [...sources.filter(source => source.kind === "file"), ...sources.filter(source => source.kind === "subscription")];
  let complete = true;
  let next = 0;
  async function readNext() {
    while (active() && next < queue.length) {
      try {
        const matches = await readSource(queue[next++], from, to, false, active);
        if (active()) onEvents(matches, isCurrent);
      } catch {
        complete = false;
      }
    }
  }
  await Promise.all(Array.from({ length: Math.min(3, queue.length) }, readNext));
  return complete && active();
}

async function load(version: number, force: boolean) {
  try {
    const sources = await invoke<ICalendarSource[]>("list_ical_sources");
    if (version !== generation) return;
    const sourceIds = new Set(sources.map(s => s.id));
    publish({ ...current, sources, events: current.events.filter(ev => sourceIds.has(ev.sourceId)),
      issues: Object.fromEntries(Object.entries(current.issues).filter(([id]) => sourceIds.has(id))) });
    // Include a brief overlap so a lookup started during a refresh can still
    // evaluate appointments against its original recording-start time.
    const now = new Date(Date.now() - 60_000);
    const until = weekFrom(now);
    // A slow subscription should never delay an imported meeting.
    const queue = [...sources.filter(s => s.kind === "file"), ...sources.filter(s => s.kind === "subscription")];
    let next = 0;
    async function readNext() {
      while (version === generation && next < queue.length) {
        const source = queue[next++];
        let events: ICalendarEvent[] = [];
        let issue: string | null = null;
        try {
          events = await readSource(source, now, until, force, () => version === generation);
        } catch (error) {
          issue = messageFor(error, "Could not read this calendar. Check its format or connection and refresh.");
        }
        if (version !== generation) return;
        const issues = { ...current.issues };
        if (issue) issues[source.id] = issue;
        else delete issues[source.id];
        const combined = current.events.filter(event => event.sourceId !== source.id).concat(events);
        combined.sort((a, b) => a.start.localeCompare(b.start) || a.id.localeCompare(b.id));
        publish({ ...current, events: combined, issues });
      }
    }
    // Bound concurrent downloads and parser workers even if many calendars are configured.
    await Promise.all(Array.from({ length: Math.min(3, sources.length) }, readNext));
    if (version !== generation) return;
    publish({ ...current, loading: false });
  } catch (error) {
    if (version === generation) publish({ ...empty, error: messageFor(error, "Could not load calendar sources. Try again.") });
  }
}

export function refreshICalendars(force = false): Promise<void> {
  if (inflight) return inflight;
  if (!force && Date.now() - lastChecked < ICAL_REFRESH_MS) return Promise.resolve();
  lastChecked = Date.now();
  const version = generation;
  const pending = load(version, force).finally(() => {
    if (version === generation) {
      inflight = null;
    }
  });
  inflight = pending;
  publish({ ...current, loading: true, error: null });
  return pending;
}

function changed(sources: ICalendarSource[]) {
  generation++;
  inflight = null;
  lastChecked = 0;
  const ids = new Set(sources.map(s => s.id));
  publish({ ...current, sources, events: current.events.filter(ev => ids.has(ev.sourceId)),
    issues: Object.fromEntries(Object.entries(current.issues).filter(([id]) => ids.has(id))) });
  void refreshICalendars();
}

async function add(command: string, args: Record<string, string>) {
  let source: ICalendarSource;
  try { source = await invoke<ICalendarSource>(command, args); }
  catch (error) { throw new Error(messageFor(error, "Could not add the calendar. Check the source and try again.")); }
  changed([...current.sources.filter(s => s.id !== source.id), source]);
}

export async function importICalendarFile(file: File) {
  if (file.size > FILE_LIMIT) throw new Error("Calendar files and feeds must be 16 MiB or smaller.");
  let text: string;
  try {
    text = new globalThis.TextDecoder("utf-8", { fatal: true }).decode(await file.arrayBuffer());
    const now = new Date();
    await parseCalendarInWorker(text, "import", now, weekFrom(now));
  } catch (error) {
    throw new Error(messageFor(error, "Could not import this calendar. Choose a valid UTF-8 .ics file."));
  }
  const name = file.name.replace(/\.ics$/i, "").trim().slice(0, 80) || "Imported calendar";
  await add("add_ical_file", { name, content: text });
}

export async function addICalendarSubscription(name: string, url: string) {
  await add("add_ical_subscription", { name: name.trim(), url: url.trim() });
}

export async function removeICalendarSource(id: string) {
  try { await invoke("remove_ical_source", { id }); }
  catch (error) { throw new Error(messageFor(error, "Could not remove the calendar. Try again.")); }
  const pending = getPendingCalendar();
  if (pending?.source === "ical" && pending.sourceId === id) clearPendingCalendar();
  changed(current.sources.filter(s => s.id !== id));
}
