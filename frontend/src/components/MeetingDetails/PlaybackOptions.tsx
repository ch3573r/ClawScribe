"use client";

import { Keyboard, LocateFixed } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Kbd } from "@/components/ui/settings-layout";
import { PLAYBACK_RATES, formatPlaybackRate } from "@/lib/playback-controls";
import { ariaPlayerChord, DEFAULT_PLAYER_BINDINGS, PLAYER_ACTIONS, playerKeycaps, type PlayerBindings } from "@/lib/player-bindings";

interface PlaybackOptionsProps {
  bindings?: PlayerBindings;
  rate?: number;
  onRateChange?: (rate: number) => void;
  followPlayback?: boolean;
  onFollowChange?: (follow: boolean) => void;
}

/** Speed, follow-along and shortcut help for the recording player. */
export function PlaybackOptions({ bindings = DEFAULT_PLAYER_BINDINGS, rate = 1, onRateChange, followPlayback = false, onFollowChange }: PlaybackOptionsProps) {
  return <div className="flex shrink-0 items-center gap-1">
    {onRateChange && <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="sm" className="h-8 px-2 font-mono tabular-nums" aria-label={`Playback speed, ${formatPlaybackRate(rate)}`} aria-keyshortcuts={`${ariaPlayerChord(bindings.slower)} ${ariaPlayerChord(bindings.faster)}`}>{formatPlaybackRate(rate)}</Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuRadioGroup value={String(rate)} onValueChange={value => onRateChange(Number(value))}>
          {PLAYBACK_RATES.map(option => <DropdownMenuRadioItem key={option} value={String(option)}>{formatPlaybackRate(option)}</DropdownMenuRadioItem>)}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>}
    {onFollowChange && <Tooltip>
      <TooltipTrigger asChild>
        <Button variant={followPlayback ? "secondary" : "ghost"} size="icon" className="h-8 w-8" aria-label="Follow playback in the transcript" aria-pressed={followPlayback} aria-keyshortcuts={ariaPlayerChord(bindings.follow)}
          onClick={() => onFollowChange(!followPlayback)}><LocateFixed /></Button>
      </TooltipTrigger>
      <TooltipContent>{followPlayback ? "Following playback" : "Follow playback"}</TooltipContent>
    </Tooltip>}
    <Popover>
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild><Button variant="ghost" size="icon" className="h-8 w-8" aria-label="Playback shortcuts"><Keyboard /></Button></PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent>Playback shortcuts</TooltipContent>
      </Tooltip>
      <PopoverContent align="end" className="max-h-[var(--radix-popover-content-available-height)] w-80 space-y-2 overflow-y-auto text-sm">
        <p className="font-medium">Playback shortcuts</p>
        <dl className="space-y-1.5">
          {PLAYER_ACTIONS.map(({ key, label }) => <div key={key} className="flex items-center justify-between gap-3">
            <dt className="text-muted-foreground">{label}</dt>
            <dd className="shrink-0"><Kbd>{playerKeycaps(bindings[key]).join(" + ")}</Kbd></dd>
          </div>)}
        </dl>
        <p className="text-xs text-muted-foreground">Shortcuts pause while you type.</p>
      </PopoverContent>
    </Popover>
  </div>;
}
