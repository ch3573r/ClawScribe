/** Date helpers for the sidebar: meeting groups, one-line dates and the "Next up" card. */

export type MeetingGroupLabel = "Today" | "This week" | "Earlier";

const startOfDay = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate());
const sameDay = (a: Date, b: Date) => startOfDay(a).getTime() === startOfDay(b).getTime();

/** Monday 00:00 of the week that contains `date`. */
function startOfWeek(date: Date): Date {
  const day = startOfDay(date);
  day.setDate(day.getDate() - ((day.getDay() + 6) % 7));
  return day;
}

/** Which group a meeting belongs to. A missing or invalid date counts as earlier. */
export function meetingGroup(createdAt: string | undefined, now = new Date()): MeetingGroupLabel {
  if (!createdAt) return "Earlier";
  const date = new Date(createdAt);
  if (!Number.isFinite(date.getTime())) return "Earlier";
  if (sameDay(date, now)) return "Today";
  return date.getTime() >= startOfWeek(now).getTime() ? "This week" : "Earlier";
}

/** Groups in display order, keeping the order of the input inside each group and skipping empty groups. */
export function groupMeetings<T extends { created_at?: string }>(items: T[], now = new Date()): { label: MeetingGroupLabel; items: T[] }[] {
  const order: MeetingGroupLabel[] = ["Today", "This week", "Earlier"];
  const groups = new Map<MeetingGroupLabel, T[]>(order.map((label) => [label, []]));
  for (const item of items) groups.get(meetingGroup(item.created_at, now))!.push(item);
  return order.map((label) => ({ label, items: groups.get(label)! })).filter((group) => group.items.length > 0);
}

/** A date short enough for a one-line row: "10:32", "Yesterday", "Nov 16" or "Nov 16, 2025". */
export function shortMeetingDate(value: string | undefined, now = new Date()): string {
  if (!value) return "Saved";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "Saved";
  if (sameDay(date, now)) return new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" }).format(date);
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (sameDay(date, yesterday)) return "Yesterday";
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    ...(date.getFullYear() !== now.getFullYear() ? { year: "numeric" as const } : {}),
  }).format(date);
}

/** "2 h 18 min", "45 min", "3 h". */
export function formatMinutes(totalMinutes: number): string {
  const minutes = Math.max(0, Math.round(totalMinutes));
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (!hours) return `${rest} min`;
  return rest ? `${hours} h ${rest} min` : `${hours} h`;
}

export interface NextUpTime {
  /** "Today 13:00", "Tomorrow 09:00" or "Mon, 23 Nov 09:00". "Now" while it runs. */
  when: string;
  /** "in 2 h 18 min", only for an event that starts later today. */
  relative: string | null;
}

export function describeNextUp(start: string, end: string | null, now = new Date()): NextUpTime | null {
  const startDate = new Date(start);
  if (!Number.isFinite(startDate.getTime())) return null;
  const endDate = end ? new Date(end) : null;
  if (startDate.getTime() <= now.getTime()) {
    return endDate && endDate.getTime() > now.getTime() ? { when: "Now", relative: null } : null;
  }
  const time = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" }).format(startDate);
  if (sameDay(startDate, now)) {
    return { when: `Today ${time}`, relative: `in ${formatMinutes(Math.max(1, Math.ceil((startDate.getTime() - now.getTime()) / 60_000)))}` };
  }
  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  if (sameDay(startDate, tomorrow)) return { when: `Tomorrow ${time}`, relative: null };
  const day = new Intl.DateTimeFormat(undefined, { weekday: "short", day: "numeric", month: "short" }).format(startDate);
  return { when: `${day} ${time}`, relative: null };
}

/** The meeting length for the card, or null when the calendar gives no usable end. */
export function eventDuration(start: string, end: string | null): string | null {
  if (!end) return null;
  const minutes = (Date.parse(end) - Date.parse(start)) / 60_000;
  return Number.isFinite(minutes) && minutes > 0 ? formatMinutes(minutes) : null;
}

/**
 * The first timed event that has not ended and starts within `days` days.
 * All-day events are skipped: they are rarely a meeting to record.
 */
export function pickNextUp<T extends { event: { start: string | null; end: string | null }; allDay: boolean }>(
  entries: T[],
  now = new Date(),
  days = 7,
): T | null {
  const limit = now.getTime() + days * 24 * 60 * 60 * 1000;
  return entries.find(({ event, allDay }) => {
    if (allDay || !event.start) return false;
    const start = Date.parse(event.start);
    const end = Date.parse(event.end ?? event.start);
    return Number.isFinite(start) && start <= limit && end >= now.getTime();
  }) ?? null;
}
