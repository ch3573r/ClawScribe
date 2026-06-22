"use client";

import { useMemo } from "react";
import { Activity } from "lucide-react";
import { TranscriptSegmentData } from "@/types";

interface SpeakerLaneTimelineProps {
  segments: TranscriptSegmentData[];
  totalCount?: number;
  loadedCount?: number;
}

interface TimelineSegment {
  id: string;
  speaker: string;
  start: number;
  end: number;
  text: string;
}

const LANE_COLORS = [
  "var(--accent)",
  "#14b8a6",
  "#f59e0b",
  "#ef4444",
  "#8b5cf6",
  "#22c55e",
  "#0ea5e9",
  "#ec4899",
];

function normalizeSpeaker(speaker: string | undefined): string {
  const label = speaker?.trim().replace(/\s+/g, " ");
  return label || "Unlabeled";
}

function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "00:00";
  const total = Math.floor(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;

  if (hours > 0) {
    return `${hours}:${minutes.toString().padStart(2, "0")}:${secs.toString().padStart(2, "0")}`;
  }

  return `${minutes.toString().padStart(2, "0")}:${secs.toString().padStart(2, "0")}`;
}

function buildTimeline(segments: TranscriptSegmentData[]) {
  const items = segments
    .map((segment) => {
      const start = segment.timestamp;
      const end = segment.endTime ?? segment.timestamp;
      return {
        id: segment.id,
        speaker: normalizeSpeaker(segment.speaker),
        start,
        end,
        text: segment.text,
      };
    })
    .filter((segment): segment is TimelineSegment =>
      Number.isFinite(segment.start) &&
      Number.isFinite(segment.end) &&
      segment.end > segment.start
    )
    .sort((left, right) => left.start - right.start);

  const speakers: string[] = [];
  const seen = new Set<string>();
  for (const item of items) {
    if (!seen.has(item.speaker)) {
      seen.add(item.speaker);
      speakers.push(item.speaker);
    }
  }

  const duration = items.reduce((max, item) => Math.max(max, item.end), 0);

  return { items, speakers, duration };
}

export function SpeakerLaneTimeline({
  segments,
  totalCount,
  loadedCount,
}: SpeakerLaneTimelineProps) {
  const { items, speakers, duration } = useMemo(() => buildTimeline(segments), [segments]);

  if (items.length === 0 || speakers.length === 0 || duration <= 0) {
    return null;
  }

  const visibleCount = loadedCount ?? segments.length;
  const countLabel = totalCount && totalCount > visibleCount
    ? `${visibleCount}/${totalCount}`
    : `${items.length}`;

  return (
    <section className="border-b border-border bg-card/70 px-4 py-3">
      <div className="mb-2 flex items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          <Activity className="h-4 w-4 shrink-0 text-accent" aria-hidden="true" />
          <h2 className="truncate text-sm font-semibold text-foreground">Speaker timeline</h2>
        </div>
        <div className="flex shrink-0 items-center gap-3 font-mono text-[11px] text-muted-foreground">
          <span>{formatDuration(duration)}</span>
          <span>{countLabel} segments</span>
        </div>
      </div>

      <div className="space-y-1.5">
        {speakers.slice(0, 6).map((speaker, index) => {
          const color = LANE_COLORS[index % LANE_COLORS.length];
          const speakerItems = items.filter((item) => item.speaker === speaker);

          return (
            <div key={speaker} className="grid grid-cols-[7.5rem_minmax(0,1fr)] items-center gap-3">
              <div className="min-w-0 truncate font-mono text-[11px] font-medium uppercase tracking-normal text-muted-foreground">
                {speaker}
              </div>
              <div className="relative h-5 overflow-hidden rounded-[3px] bg-muted/45">
                {speakerItems.map((item) => {
                  const left = Math.max(0, Math.min(100, (item.start / duration) * 100));
                  const width = Math.max(0.3, Math.min(100 - left, ((item.end - item.start) / duration) * 100));
                  const wordCount = Math.max(1, item.text.trim().split(/\s+/).filter(Boolean).length);
                  const opacity = Math.min(0.95, 0.48 + wordCount / 80);

                  return (
                    <div
                      key={item.id}
                      className="absolute top-1/2 h-3 -translate-y-1/2 overflow-hidden rounded-[2px]"
                      style={{
                        left: `${left}%`,
                        width: `${width}%`,
                        color,
                        backgroundColor: color,
                        opacity,
                      }}
                      title={`${speaker} ${formatDuration(item.start)}-${formatDuration(item.end)}`}
                    >
                      <div
                        className="h-full w-full opacity-55"
                        style={{
                          backgroundImage:
                            "repeating-linear-gradient(90deg, currentColor 0 2px, transparent 2px 6px)",
                        }}
                      />
                    </div>
                  );
                })}
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}
