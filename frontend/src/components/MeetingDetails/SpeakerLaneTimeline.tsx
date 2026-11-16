"use client";

import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { memo, useMemo, type KeyboardEvent, type MouseEvent } from "react";
import { Pause, Play } from "lucide-react";
import { TranscriptSegmentData } from "@/types";

interface SpeakerLaneTimelineProps {
  segments: TranscriptSegmentData[]; totalCount?: number; showSpeakerAttribution?: boolean;
  currentTime?: number; durationSeconds?: number; isPlaying?: boolean; isAudioReady?: boolean;
  onPlayPause?: () => void; onSeek?: (seconds: number) => void;
}

type TimedSegment = TranscriptSegmentData & { endTime: number };
interface TimelineLane {
  speaker: string;
  color: string;
  segments: TimedSegment[];
}

const LANE_COLORS = [
  "hsl(var(--primary))",
  "hsl(var(--chart-2))",
  "hsl(var(--theme-warning-fg))",
  "hsl(var(--theme-error-fg))",
  "hsl(var(--chart-4))",
  "hsl(var(--theme-success-fg))",
];
const MAX_VISIBLE_LANES = 6;

function normalizeSpeaker(speaker: string | undefined): string {
  return speaker?.trim().replace(/\s+/g, " ") || "Unlabeled";
}

function buildTimeline(segments: TranscriptSegmentData[]) {
  const timedSegments = segments.filter((segment): segment is TimedSegment =>
    Number.isFinite(segment.timestamp) && segment.timestamp >= 0 &&
    Number.isFinite(segment.endTime) && segment.endTime! > segment.timestamp
  ).sort((left, right) => left.timestamp - right.timestamp);
  const lanes = new Map<string, TimelineLane>();
  let duration = 0;
  for (const segment of timedSegments) {
    duration = Math.max(duration, segment.endTime);
    const speaker = normalizeSpeaker(segment.speaker);
    let lane = lanes.get(speaker);
    if (!lane && lanes.size < MAX_VISIBLE_LANES) {
      lane = { speaker, color: LANE_COLORS[lanes.size], segments: [] };
      lanes.set(speaker, lane);
    }
    lane?.segments.push(segment);
  }
  return { timedSegments, lanes: Array.from(lanes.values()), duration };
}

function formatDuration(seconds: number): string {
  const total = Math.max(0, Math.floor(Number.isFinite(seconds) ? seconds : 0));
  return total >= 3600
    ? Math.floor(total / 3600) + ':' + String(Math.floor(total / 60) % 60).padStart(2, '0') + ':' + String(total % 60).padStart(2, '0')
    : String(Math.floor(total / 60)).padStart(2, '0') + ':' + String(total % 60).padStart(2, '0');
}
const SegmentBars = memo(function SegmentBars({ segments, duration, played = false }: { segments: TimedSegment[]; duration: number; played?: boolean }) {
  return <>{segments.map(segment => {
    const left = Math.max(0, Math.min(100, segment.timestamp / duration * 100));
    const width = Math.max(0, Math.min(100 - left, ((segment.endTime! - segment.timestamp) / duration) * 100));
    return <span key={segment.id} data-timeline-segment={played ? undefined : segment.id} className={`absolute inset-y-2 rounded-sm border-x border-card ${played ? "bg-gradient-to-r from-primary to-primary/70" : "bg-muted-foreground/40"}`}
      style={{ left: left + '%', width: width + '%' }}
      title={played ? undefined : formatDuration(segment.timestamp) + '-' + formatDuration(segment.endTime)} />;
  })}</>;
});

// Keep the heavier segment DOM independent of playback clock updates.
const LaneBars = memo(function LaneBars({ lane, duration }: { lane: TimelineLane; duration: number }) {
  return <>{lane.segments.map(segment => {
    const left = Math.max(0, Math.min(100, segment.timestamp / duration * 100));
    const width = Math.min(100 - left, Math.max(0.3, (segment.endTime - segment.timestamp) / duration * 100));
    const wordCount = Math.max(1, segment.text.trim().split(/\s+/).filter(Boolean).length);
    return <span key={segment.id} className="absolute top-1/2 h-3 -translate-y-1/2 overflow-hidden rounded-sm"
      style={{ left: left + '%', width: width + '%', color: lane.color, backgroundColor: lane.color, opacity: Math.min(0.95, 0.48 + wordCount / 80) }}
      title={lane.speaker + ' ' + formatDuration(segment.timestamp) + '-' + formatDuration(segment.endTime)}>
      <span className="block h-full w-full opacity-55" style={{ backgroundImage: 'repeating-linear-gradient(90deg, currentColor 0 2px, transparent 2px 6px)' }} />
    </span>;
  })}</>;
});

export function SpeakerLaneTimeline({ segments, showSpeakerAttribution = true, currentTime = 0, durationSeconds = 0, isPlaying = false, isAudioReady = false, onPlayPause, onSeek }: SpeakerLaneTimelineProps) {
  const { timedSegments, lanes, duration: segmentDuration } = useMemo(() => buildTimeline(segments), [segments]);
  if (!isAudioReady) return null;
  const duration = Math.max(segmentDuration, Number.isFinite(durationSeconds) ? durationSeconds : 0);
  const timed = timedSegments.length > 0;
  const canSeek = Boolean(onSeek && duration > 0);
  const played = duration > 0 ? Math.max(0, Math.min(100, currentTime / duration * 100)) : 0;
  const label = isPlaying ? 'Pause recording playback' : 'Play recording';
  const handlePointerSeek = (event: MouseEvent<HTMLButtonElement>) => {
    // Keyboard activation of the button has no pointer coordinates.
    if (!canSeek || event.detail === 0) return;
    const rect = event.currentTarget.getBoundingClientRect();
    if (rect.width > 0) onSeek?.(Math.max(0, Math.min(duration, (event.clientX - rect.left) / rect.width * duration)));
  };
  const handleKeySeek = (event: KeyboardEvent<HTMLButtonElement>) => {
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? duration : ['ArrowRight', 'ArrowUp'].includes(event.key) ? currentTime + 5 : ['ArrowLeft', 'ArrowDown'].includes(event.key) ? currentTime - 5 : null;
    if (next !== null && canSeek) { event.preventDefault(); onSeek?.(Math.max(0, Math.min(duration, next))); }
  };
  const seekProps = {
    type: 'button' as const, role: 'slider' as const,
    'aria-valuemin': 0, 'aria-valuemax': duration, 'aria-valuenow': Math.min(duration, Math.max(0, currentTime)),
    'aria-valuetext': formatDuration(currentTime) + ' of ' + formatDuration(duration),
    disabled: !canSeek, onClick: handlePointerSeek, onKeyDown: handleKeySeek,
  };
  return <section aria-label="Recording playback" className="shrink-0 border-b border-border bg-card">
    <div className="flex items-center gap-4 px-5 py-4">
    <Tooltip><TooltipTrigger asChild><Button variant="outline" size="icon" className="h-11 w-11 shrink-0 rounded-full" aria-label={label} disabled={!onPlayPause} onClick={onPlayPause}>
      {isPlaying ? <Pause /> : <Play />}
    </Button></TooltipTrigger><TooltipContent>{label}</TooltipContent></Tooltip>
    <span className="shrink-0 font-mono text-xs tabular-nums text-primary">{formatDuration(currentTime)}</span>
    <Button variant="ghost" {...seekProps} aria-label="Seek recording"
      className="relative h-10 min-w-0 flex-1 overflow-hidden rounded-sm p-0 hover:bg-muted/40 disabled:opacity-100">
      <span aria-hidden className="pointer-events-none absolute inset-0">
        {timed ? <SegmentBars segments={timedSegments} duration={duration} /> : <span className="absolute inset-x-0 top-1/2 h-2 -translate-y-1/2 rounded-full bg-muted" />}
        <span className="absolute inset-0" style={{ clipPath: `inset(0 ${100 - played}% 0 0)` }}>{timed ? <SegmentBars segments={timedSegments} duration={duration} played /> : <span className="absolute inset-x-0 top-1/2 h-2 -translate-y-1/2 rounded-full bg-gradient-to-r from-primary to-primary/70" />}</span>
        <span className="absolute inset-y-1 w-px bg-foreground" style={{ left: played + '%' }} />
      </span>
    </Button>
    <span className="shrink-0 font-mono text-xs tabular-nums text-muted-foreground">{formatDuration(duration)}</span>
    </div>
    {showSpeakerAttribution && lanes.length > 0 && <div className="space-y-1.5 px-5 pb-4">
      {lanes.map(lane => <div key={lane.speaker} className="grid grid-cols-[7.5rem_minmax(0,1fr)] items-center gap-3">
        <span className="truncate font-mono text-xs font-medium text-muted-foreground" title={lane.speaker}>{lane.speaker}</span>
        <Button variant="ghost" {...seekProps} aria-label={`Seek ${lane.speaker} timeline`}
          className="relative h-5 w-full overflow-hidden rounded-sm bg-muted/45 p-0 hover:bg-muted/70 disabled:opacity-100">
          <LaneBars lane={lane} duration={duration} />
          <span aria-hidden className="pointer-events-none absolute inset-y-0 w-px bg-foreground/80" style={{ left: played + '%' }} />
        </Button>
      </div>)}
    </div>}
  </section>;
}
