import type { MeetingCalendarAttendee } from "@/lib/meetingCalendar";

type SpeakerRow = { speaker?: string | null; timestamp?: number };

export interface SpeakerVoiceOption {
  speaker: string;
  canRemember: boolean;
  suggestedName: string | null;
}

export interface SpeakerDetails {
  name: string | null;
  count: number;
  color: string;
}

const LANE_COLORS = [
  "hsl(var(--primary))",
  "hsl(var(--chart-2))",
  "hsl(var(--theme-warning-fg))",
  "hsl(var(--chart-5))",
  "hsl(var(--chart-4))",
  "hsl(var(--theme-success-fg))",
];
const NEUTRAL_COLOR = "hsl(var(--muted-foreground))";
const SOURCE_SPEAKERS = ["Me", "Participants"];

export function speakerLabel(value: string | null | undefined): string | null {
  return value?.trim().replace(/\s+/g, " ") || null;
}

export function normalizeSpeakerName(value: string): string | null {
  const name = speakerLabel(value);
  return name && name.length <= 64 ? name : null;
}

/** Include every loaded row for counts and a single first-appearance colour order. */
export function buildSpeakerCatalog(rows: readonly SpeakerRow[]): SpeakerDetails[] {
  const catalog = new Map<string | null, SpeakerDetails>();
  const time = (row: SpeakerRow) => Number.isFinite(row.timestamp) && row.timestamp! >= 0 ? row.timestamp! : Infinity;
  let named = 0;
  for (const row of [...rows].sort((a, b) => time(a) - time(b))) {
    const name = speakerLabel(row.speaker);
    const existing = catalog.get(name);
    if (existing) existing.count++;
    // Unlabelled rows never take a palette slot, so labelling one does not recolour the others.
    else catalog.set(name, { name, count: 1, color: name === null ? NEUTRAL_COLOR : LANE_COLORS[named++] ?? NEUTRAL_COLOR });
  }
  return [...catalog.values()];
}

export function getSpeakerColor(catalog: readonly SpeakerDetails[], name: string | null | undefined): string {
  return catalog.find(entry => entry.name === speakerLabel(name))?.color ?? NEUTRAL_COLOR;
}

const sameName = (left: string, right: string) => left.toLocaleLowerCase() === right.toLocaleLowerCase();

export function getConfirmedAttendeeNames(attendees: readonly MeetingCalendarAttendee[]): string[] {
  const names: string[] = [];
  for (const attendee of attendees) {
    if (attendee.attendance !== "present" || !attendee.name) continue;
    const name = normalizeSpeakerName(attendee.name);
    if (name && !names.some(existing => sameName(existing, name))) names.push(name);
  }
  return names;
}

export function getSpeakerOptions(catalog: readonly SpeakerDetails[], sourceAttributionEnabled: boolean, attendeeNames: readonly string[] = []): string[] {
  const names = catalog.flatMap(entry => entry.name ? [entry.name] : []);
  if (sourceAttributionEnabled || names.some(name => SOURCE_SPEAKERS.includes(name))) {
    for (const name of SOURCE_SPEAKERS) if (!names.includes(name)) names.push(name);
  }
  for (const name of attendeeNames) {
    if (!names.some(existing => sameName(existing, name))) names.push(name);
  }
  return names;
}

/** Resolve a typed name to an existing speaker's spelling, ignoring case, so "alice" joins "Alice". */
export function resolveSpeakerName(catalog: readonly SpeakerDetails[], value: string, sourceAttributionEnabled = false, attendeeNames: readonly string[] = []): string | null {
  const name = normalizeSpeakerName(value);
  if (!name) return null;
  return getSpeakerOptions(catalog, sourceAttributionEnabled, attendeeNames).find(option => sameName(option, name)) ?? name;
}

export function getSpeakerRename(catalog: readonly SpeakerDetails[], from: string, value: string) {
  const current = speakerLabel(from);
  const typed = normalizeSpeakerName(value);
  const other = typed ? catalog.find(entry => entry.name && entry.name !== current && sameName(entry.name, typed))?.name ?? null : null;
  const name = other ?? typed;
  return {
    name,
    count: catalog.find(entry => entry.name === current)?.count ?? 0,
    merge: other,
    changed: name !== null && name !== current,
  };
}

export function canRememberSpeaker(
  option: Pick<SpeakerVoiceOption, "canRemember"> | undefined,
  rename: ReturnType<typeof getSpeakerRename>,
): boolean {
  return Boolean(option?.canRemember && rename.changed && !rename.merge && rename.name
    && !/^(?:me|participants|unlabeled|speaker \d+)$/i.test(rename.name));
}
