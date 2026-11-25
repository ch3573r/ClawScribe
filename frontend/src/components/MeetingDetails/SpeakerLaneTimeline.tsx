"use client";

import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Slider } from "@/components/ui/slider";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { memo, useMemo, useState, type KeyboardEvent, type PointerEvent } from "react";
import { Bookmark, Pause, Play, Volume2, VolumeX } from "lucide-react";
import { buildSpeakerCatalog, getSpeakerColor, speakerLabel, type SpeakerDetails } from "@/lib/speaker-labels";
import { RenameSpeakerPopover, SpeakerDot, type RenameSpeaker } from "./SpeakerLabelPopover";
import { TranscriptSegmentData } from "@/types";
import { PlaybackOptions } from "./PlaybackOptions";
import type { PlayerBindings } from "@/lib/player-bindings";
import { bookmarkTime, type MeetingBookmark } from "@/lib/library";
import { usePlaybackTime, type PlaybackClock } from "@/hooks/useAudioPlayer";

interface SpeakerLaneTimelineProps {
  bindings?: PlayerBindings;
  onApplySpeakerToMatching?: RenameSpeaker;
  segments: TranscriptSegmentData[]; totalCount?: number; showSpeakerAttribution?: boolean;
  currentTime?: number; durationSeconds?: number; isPlaying?: boolean; isAudioReady?: boolean;
  playbackClock?: PlaybackClock;
  onPlayPause?: () => void; onSeek?: (seconds: number) => void;
  onSeekBy?: (seconds: number) => void;
  volume?: number; muted?: boolean;
  onVolumeChange?: (volume: number) => void; onMuteToggle?: () => void;
  highlightedSpeaker?: string | null;
  onHighlightSpeaker?: (speaker: string | null | undefined) => void;
  rate?: number; onRateChange?: (rate: number) => void;
  followPlayback?: boolean; onFollowChange?: (follow: boolean) => void;
  bookmarks?: readonly MeetingBookmark[];
}

type TimedSegment = TranscriptSegmentData & { endTime: number };
interface TimelineLane {
  speaker: string;
  label: string | null;
  color: string;
  segments: TimedSegment[];
}

const MAX_VISIBLE_SPEAKERS = 6;

function buildTimeline(segments: TranscriptSegmentData[], catalog: readonly SpeakerDetails[]) {
  const timedSegments = segments.filter((segment): segment is TimedSegment =>
    Number.isFinite(segment.timestamp) && segment.timestamp >= 0 &&
    Number.isFinite(segment.endTime) && segment.endTime! > segment.timestamp
  ).sort((left, right) => left.timestamp - right.timestamp);
  const lanes = new Map<string | null, TimelineLane>();
  let duration = 0;
  for (const segment of timedSegments) {
    duration = Math.max(duration, segment.endTime);
    const label = speakerLabel(segment.speaker);
    const speaker = label ?? "Unlabeled";
    let lane = lanes.get(label);
    if (!lane) {
      lane = { speaker, label, color: getSpeakerColor(catalog, label), segments: [] };
      lanes.set(label, lane);
    }
    lane.segments.push(segment);
  }
  const visibleLanes: TimelineLane[] = [];
  const overflowLanes: TimelineLane[] = [];
  let named = 0;
  for (const lane of lanes.values()) {
    if (lane.label === null || named++ < MAX_VISIBLE_SPEAKERS) visibleLanes.push(lane);
    else overflowLanes.push(lane);
  }
  return { timedSegments, visibleLanes, overflowLanes, duration };
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

export function SpeakerLaneTimeline({
  bindings, segments, onApplySpeakerToMatching, showSpeakerAttribution = true,
  highlightedSpeaker, onHighlightSpeaker, currentTime: initialTime = 0, playbackClock,
  durationSeconds = 0, isPlaying = false, isAudioReady = false, onPlayPause, onSeek, onSeekBy,
  volume = 1, muted = false, onVolumeChange, onMuteToggle, rate, onRateChange,
  followPlayback, onFollowChange, bookmarks = [],
}: SpeakerLaneTimelineProps) {
  const currentTime = usePlaybackTime(playbackClock) ?? initialTime;
  // Keep the rename anchor and draft mounted through optimistic label updates.
  const [renameSnapshot, setRenameSnapshot] = useState<TranscriptSegmentData[] | null>(null);
  const [overflowOpen, setOverflowOpen] = useState(false);
  const laneSegments = renameSnapshot ?? segments;
  const catalog = useMemo(() => buildSpeakerCatalog(laneSegments), [laneSegments]);
  const { timedSegments, visibleLanes, overflowLanes, duration: segmentDuration } = useMemo(() => buildTimeline(laneSegments, catalog), [laneSegments, catalog]);
  if (!isAudioReady) return null;
  const duration = Number.isFinite(durationSeconds) && durationSeconds > 0 ? durationSeconds : segmentDuration;
  const timed = timedSegments.length > 0;
  const canSeek = Boolean(onSeek && duration > 0);
  const played = duration > 0 ? Math.max(0, Math.min(100, currentTime / duration * 100)) : 0;
  const label = isPlaying ? 'Pause recording playback' : 'Play recording';
  const seekFromPointer = (event: PointerEvent<HTMLButtonElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    if (rect.width > 0) onSeek?.(Math.max(0, Math.min(duration, (event.clientX - rect.left) / rect.width * duration)));
  };
  const startScrub = (event: PointerEvent<HTMLButtonElement>) => {
    if (!canSeek || !event.isPrimary || event.button !== 0) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    seekFromPointer(event);
  };
  const updateScrub = (event: PointerEvent<HTMLButtonElement>) => {
    if (canSeek && event.currentTarget.hasPointerCapture(event.pointerId)) seekFromPointer(event);
  };
  const handleKeySeek = (event: KeyboardEvent<HTMLButtonElement>) => {
    const position = playbackClock?.getSnapshot() ?? currentTime;
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? duration : ['ArrowRight', 'ArrowUp'].includes(event.key) ? position + 5 : ['ArrowLeft', 'ArrowDown'].includes(event.key) ? position - 5 : null;
    if (next !== null && canSeek) { event.preventDefault(); onSeek?.(Math.max(0, Math.min(duration, next))); }
  };
  const seekProps = {
    type: 'button' as const, role: 'slider' as const,
    'aria-valuemin': 0, 'aria-valuemax': duration, 'aria-valuenow': Math.min(duration, Math.max(0, currentTime)),
    'aria-valuetext': formatDuration(currentTime) + ' of ' + formatDuration(duration),
    disabled: !canSeek, onPointerDown: startScrub, onPointerMove: updateScrub, onPointerUp: updateScrub, onKeyDown: handleKeySeek,
  };
  const highlight = (lane: TimelineLane) => onHighlightSpeaker?.(highlightedSpeaker === lane.label ? undefined : lane.label);
  const playhead = <span aria-hidden="true" className="pointer-events-none absolute inset-y-0 w-px bg-foreground/80" style={{ left: played + '%' }} />;
  const volumePercent = Math.round(volume * 100);
  return <section aria-label="Recording playback" className="shrink-0 border-b border-border bg-card">
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 px-5 py-3">
    <Tooltip><TooltipTrigger asChild><Button type="button" variant="outline" size="icon" className="h-9 w-9 shrink-0 rounded-full" aria-label={label} disabled={!onPlayPause} onClick={onPlayPause}>
      {isPlaying ? <Pause /> : <Play />}
    </Button></TooltipTrigger><TooltipContent>{label}</TooltipContent></Tooltip>
    {onSeekBy && <div className="flex shrink-0 items-center">
      <Button type="button" variant="ghost" size="sm" aria-label="Back 10 seconds" className="px-1.5 tabular-nums" onClick={() => onSeekBy(-10)}>−10s</Button>
      <Button type="button" variant="ghost" size="sm" aria-label="Forward 10 seconds" className="px-1.5 tabular-nums" onClick={() => onSeekBy(10)}>+10s</Button>
    </div>}
    <span className="shrink-0 font-mono text-xs tabular-nums text-primary">{formatDuration(currentTime)}</span>
    <div className="relative h-10 min-w-20 flex-1">
    <Button variant="ghost" {...seekProps} aria-label="Seek recording"
      className="relative h-10 w-full touch-none cursor-pointer overflow-hidden rounded-sm p-0 hover:bg-muted/40 disabled:opacity-100">
      <span aria-hidden className="pointer-events-none absolute inset-0">
        {timed ? <SegmentBars segments={timedSegments} duration={duration} /> : <span className="absolute inset-x-0 top-1/2 h-2 -translate-y-1/2 rounded-full bg-muted" />}
        <span className="absolute inset-0" style={{ clipPath: `inset(0 ${100 - played}% 0 0)` }}>{timed ? <SegmentBars segments={timedSegments} duration={duration} played /> : <span className="absolute inset-x-0 top-1/2 h-2 -translate-y-1/2 rounded-full bg-gradient-to-r from-primary to-primary/70" />}</span>
        <span className="absolute inset-y-1 w-px bg-foreground" style={{ left: played + '%' }} />
      </span>
    </Button>
    {duration > 0 && bookmarks.filter(mark => Number.isFinite(mark.seconds) && mark.seconds >= 0 && mark.seconds <= duration).map(mark =>
      <Tooltip key={mark.id}><TooltipTrigger asChild>
        <Button variant="ghost" type="button" disabled={!canSeek} aria-label={`Go to bookmark at ${bookmarkTime(mark.seconds)}: ${mark.label}`}
          className="absolute bottom-0 h-4 w-4 -translate-x-1/2 rounded-sm p-0 text-primary hover:bg-transparent hover:text-foreground disabled:opacity-100 [&_svg]:size-3"
          style={{ left: Math.min(100, mark.seconds / duration * 100) + '%' }} onClick={() => onSeek?.(mark.seconds)}><Bookmark className="fill-current" /></Button>
      </TooltipTrigger><TooltipContent>{bookmarkTime(mark.seconds)} · {mark.label}</TooltipContent></Tooltip>)}
    </div>
    <span className="shrink-0 font-mono text-xs tabular-nums text-muted-foreground">{formatDuration(duration)}</span>
    {(onVolumeChange || onMuteToggle) && <div className="flex shrink-0 items-center gap-1">
      {onMuteToggle && <Tooltip><TooltipTrigger asChild>
        <Button type="button" variant="ghost" size="icon" aria-label="Mute recording" aria-pressed={muted} className="h-8 w-8" onClick={onMuteToggle}>
          {muted || volume === 0 ? <VolumeX aria-hidden="true" /> : <Volume2 aria-hidden="true" />}
        </Button>
      </TooltipTrigger><TooltipContent>{muted ? 'Unmute recording' : 'Mute recording'}</TooltipContent></Tooltip>}
      {onVolumeChange && <Slider className="w-20" aria-label="Recording volume" aria-valuetext={`${volumePercent}%${muted ? ', muted' : ''}`}
        value={[volumePercent]} max={100} step={1} onValueChange={([value]) => onVolumeChange(value / 100)} />}
    </div>}
    <PlaybackOptions bindings={bindings} rate={rate} onRateChange={onRateChange} followPlayback={followPlayback} onFollowChange={onFollowChange} />
    </div>
    {showSpeakerAttribution && visibleLanes.length > 0 && <div className="space-y-1 px-5 pb-3">
      {visibleLanes.map(lane => <div key={lane.label === null ? 'unlabeled' : `speaker-${lane.label}`} className="grid grid-cols-[minmax(0,1fr)_1.5rem] items-center gap-1">
        <Button type="button" variant="ghost" aria-label={`Highlight ${lane.speaker} in transcript`} aria-pressed={highlightedSpeaker === lane.label}
          onClick={() => highlight(lane)} disabled={!onHighlightSpeaker}
          className="grid h-6 min-w-0 grid-cols-[7.5rem_minmax(0,1fr)] gap-2 rounded-sm px-1.5 py-0 hover:bg-muted/70 focus-visible:ring-inset aria-pressed:bg-accent aria-pressed:text-accent-foreground disabled:opacity-100">
          <span className="flex min-w-0 items-center gap-2 text-left text-xs" title={lane.speaker}>
            <SpeakerDot color={lane.color} /><span className="truncate">{lane.speaker}</span>
          </span>
          <span className="relative h-5 w-full overflow-hidden rounded-sm bg-muted/65">
            <LaneBars lane={lane} duration={duration} />{playhead}
          </span>
        </Button>
        {lane.label && onApplySpeakerToMatching && <RenameSpeakerPopover speaker={lane.label} catalog={catalog} onRename={onApplySpeakerToMatching}
          onOpenChange={open => setRenameSnapshot(open ? segments : null)} />}
      </div>)}
      {overflowLanes.length > 0 && <div className="grid grid-cols-[minmax(0,1fr)_1.5rem] gap-1">
        <Popover open={overflowOpen} onOpenChange={setOverflowOpen}>
          <PopoverTrigger asChild>
            <Button type="button" variant="ghost" aria-label={`+${overflowLanes.length} more speakers`}
              data-highlighted={overflowLanes.some(lane => lane.label === highlightedSpeaker) || undefined}
              className="grid h-6 grid-cols-[7.5rem_minmax(0,1fr)] gap-2 rounded-sm px-1.5 py-0 text-left text-xs hover:bg-muted/70 data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground">
              <span className="justify-self-start">+{overflowLanes.length} more</span>
              <span className="relative h-5 w-full overflow-hidden rounded-sm bg-muted/65">
                {overflowLanes.map(lane => <LaneBars key={lane.label} lane={lane} duration={duration} />)}{playhead}
              </span>
            </Button>
          </PopoverTrigger>
          <PopoverContent align="start" aria-label="More speakers" className="max-h-[min(14rem,var(--radix-popover-content-available-height))] overflow-y-auto p-2">
            {overflowLanes.map(lane => <Button key={lane.label} type="button" variant="ghost" disabled={!onHighlightSpeaker}
              aria-label={`Highlight ${lane.speaker} in transcript`} aria-pressed={highlightedSpeaker === lane.label}
              className="h-auto w-full justify-start px-2 py-1.5 text-sm aria-pressed:bg-accent aria-pressed:text-accent-foreground"
              onClick={() => { highlight(lane); setOverflowOpen(false); }}>
              <SpeakerDot color={lane.color} /><span className="truncate">{lane.speaker}</span>
            </Button>)}
          </PopoverContent>
        </Popover>
      </div>}
    </div>}
  </section>;
}
