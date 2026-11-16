type SpeakerRow = { speaker?: string | null; timestamp?: number };

export interface SpeakerDetails {
  name: string | null;
  count: number;
  color: string;
}

const LANE_COLORS = [
  "hsl(var(--primary))",
  "hsl(var(--chart-2))",
  "hsl(var(--theme-warning-fg))",
  "hsl(var(--theme-error-fg))",
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
  for (const row of [...rows].sort((a, b) => time(a) - time(b))) {
    const name = speakerLabel(row.speaker);
    const existing = catalog.get(name);
    if (existing) existing.count++;
    else catalog.set(name, { name, count: 1, color: LANE_COLORS[catalog.size] ?? NEUTRAL_COLOR });
  }
  return [...catalog.values()];
}

export function getSpeakerColor(catalog: readonly SpeakerDetails[], name: string | null | undefined): string {
  return catalog.find(entry => entry.name === speakerLabel(name))?.color ?? NEUTRAL_COLOR;
}

export function getSpeakerOptions(catalog: readonly SpeakerDetails[], sourceAttributionEnabled: boolean): string[] {
  const names = catalog.flatMap(entry => entry.name ? [entry.name] : []);
  if (sourceAttributionEnabled || names.some(name => SOURCE_SPEAKERS.includes(name))) {
    for (const name of SOURCE_SPEAKERS) if (!names.includes(name)) names.push(name);
  }
  return names;
}

export function getSpeakerRename(catalog: readonly SpeakerDetails[], from: string, value: string) {
  const name = normalizeSpeakerName(value);
  return {
    name,
    count: catalog.find(entry => entry.name === speakerLabel(from))?.count ?? 0,
    merge: name && name !== speakerLabel(from) && catalog.some(entry => entry.name === name) ? name : null,
  };
}
