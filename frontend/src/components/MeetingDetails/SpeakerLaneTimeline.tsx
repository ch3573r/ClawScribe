"use client";

import { memo, useMemo } from "react";
import { Activity, Pause, Play } from "lucide-react";
import { TranscriptSegmentData } from "@/types";

interface SpeakerLaneTimelineProps {
  segments: TranscriptSegmentData[];
  totalCount?: number;
  loadedCount?: number;
  currentTime?: number;
  durationSeconds?: number;
  isPlaying?: boolean;
  isAudioReady?: boolean;
  onPlayPause?: () => void;
  onSeek?: (seconds: number) => void;
}

interface TimelineSegment {
  id: string;
  speaker: string;
  start: number;
  end: number;
  opacity: number;
}

interface TimelineLane {
  speaker: string;
  color: string;
  items: TimelineSegment[];
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

const MAX_VISIBLE_LANES = 6;

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
      const wordCount = Math.max(1, segment.text.trim().split(/\s+/).filter(Boolean).length);
      return {
        id: segment.id,
        speaker: normalizeSpeaker(segment.speaker),
        start,
        end,
        opacity: Math.min(0.95, 0.48 + wordCount / 80),
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

  const lanes: TimelineLane[] = speakers.slice(0, MAX_VISIBLE_LANES).map((speaker, index) => ({
    speaker,
    color: LANE_COLORS[index % LANE_COLORS.length],
    items: items.filter((item) => item.speaker === speaker),
  }));

  return { itemCount: items.length, lanes, duration };
}

// The per-segment bars are by far the heaviest DOM in this component (one
// node per transcript row per lane). Memoized so playhead ticks reconcile
// only the thin playhead spans, not thousands of bars.
const LaneBars = memo(function LaneBars({
  lane,
  timelineDuration,
}: {
  lane: TimelineLane;
  timelineDuration: number;
}) {
  return (
    <>
      {lane.items.map((item) => {
        const left = Math.max(0, Math.min(100, (item.start / timelineDuration) * 100));
        const width = Math.max(0.3, Math.min(100 - left, ((item.end - item.start) / timelineDuration) * 100));

        return (
          <div
            key={item.id}
            className="absolute top-1/2 h-3 -translate-y-1/2 overflow-hidden rounded-[2px]"
            style={{
              left: `${left}%`,
              width: `${width}%`,
              color: lane.color,
              backgroundColor: lane.color,
              opacity: item.opacity,
            }}
            title={`${lane.speaker} ${formatDuration(item.start)}-${formatDuration(item.end)}`}
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
    </>
  );
});

export function SpeakerLaneTimeline({
  segments,
  totalCount,
  loadedCount,
  currentTime = 0,
  durationSeconds,
  isPlaying = false,
  isAudioReady = false,
  onPlayPause,
  onSeek,
}: SpeakerLaneTimelineProps) {
  const { itemCount, lanes, duration } = useMemo(() => buildTimeline(segments), [segments]);

  if (itemCount === 0 || lanes.length === 0 || duration <= 0) {
    return null;
  }

  const timelineDuration = Math.max(duration, durationSeconds ?? 0);
  const canSeek = Boolean(onSeek && timelineDuration > 0);
  const playheadLeft = Math.max(0, Math.min(100, (currentTime / timelineDuration) * 100));
  const visibleCount = loadedCount ?? segments.length;
  const countLabel = totalCount && totalCount > visibleCount
    ? `${visibleCount}/${totalCount}`
    : `${itemCount}`;

  return (
    <section className="border-b border-border bg-card/70 px-4 py-3">
      <div className="mb-2 flex items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          <Activity className="h-4 w-4 shrink-0 text-accent" aria-hidden="true" />
          <h2 className="truncate text-sm font-semibold text-foreground">Speaker timeline</h2>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          {isAudioReady && onPlayPause && (
            <button
              type="button"
              onClick={onPlayPause}
              className="inline-flex h-7 w-7 items-center justify-center rounded-[4px] border border-border bg-background text-foreground hover:bg-muted focus:outline-none focus:ring-2 focus:ring-ring"
              aria-label={isPlaying ? "Pause recording playback" : "Play recording"}
            >
              {isPlaying ? (
                <Pause className="h-3.5 w-3.5" aria-hidden="true" />
              ) : (
                <Play className="h-3.5 w-3.5" aria-hidden="true" />
              )}
            </button>
          )}
          <div className="flex items-center gap-3 font-mono text-[11px] text-muted-foreground">
            <span>{formatDuration(timelineDuration)}</span>
            <span>{countLabel} segments</span>
          </div>
        </div>
      </div>

      <div className="space-y-1.5">
        {lanes.map((lane) => (
          <div key={lane.speaker} className="grid grid-cols-[7.5rem_minmax(0,1fr)] items-center gap-3">
            <div className="min-w-0 truncate font-mono text-[11px] font-medium uppercase tracking-normal text-muted-foreground">
              {lane.speaker}
            </div>
            <button
              type="button"
              disabled={!canSeek}
              className={`relative h-5 w-full overflow-hidden rounded-[3px] bg-muted/45 text-left focus:outline-none focus:ring-2 focus:ring-ring ${canSeek ? "cursor-pointer hover:bg-muted/70" : "cursor-default"}`}
              onClick={(event) => {
                if (!onSeek) return;
                const rect = event.currentTarget.getBoundingClientRect();
                const ratio = (event.clientX - rect.left) / rect.width;
                onSeek(Math.max(0, Math.min(timelineDuration, ratio * timelineDuration)));
              }}
              aria-label={`Seek ${lane.speaker} timeline`}
            >
              <LaneBars lane={lane} timelineDuration={timelineDuration} />
              {canSeek && (
                <span
                  className="pointer-events-none absolute top-0 h-full w-px bg-foreground/80 shadow-[0_0_0_1px_rgba(255,255,255,0.22)]"
                  style={{ left: `${playheadLeft}%` }}
                  aria-hidden="true"
                />
              )}
            </button>
          </div>
        ))}
      </div>
    </section>
  );
}
