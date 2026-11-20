"use client";

import { useEffect, useRef, type KeyboardEvent } from "react";
import { ChevronDown, ChevronUp, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { MAX_FIND_QUERY_LENGTH } from "@/lib/transcript-find";

interface TranscriptFindProps {
  query: string;
  onQueryChange: (query: string) => void;
  /** Position of the current match, or -1 when none is selected. */
  current: number;
  total: number;
  complete?: boolean;
  /** Increments each time the bar is asked to take focus. */
  focusRequest: number;
  onStep: (direction: 1 | -1) => void;
  onClose: () => void;
}

/** Find bar for the transcript. Enter goes to the next passage, Shift+Enter to the previous one. */
export function TranscriptFind({ query, onQueryChange, current, total, complete = true, focusRequest, onStep, onClose }: TranscriptFindProps) {
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => { input.current?.focus(); input.current?.select(); }, [focusRequest]);
  const searching = query.trim().length > 0;
  const count = total === 0 ? "No matches" : current >= 0 ? `${current + 1} of ${total}` : `${total} ${total === 1 ? "match" : "matches"}`;
  const status = !searching ? "" : complete
    ? (total > 0 && current < 0 ? `${total} ${total === 1 ? "passage" : "passages"}` : count)
    : `${count} in loaded passages`;
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "Enter") { event.preventDefault(); onStep(event.shiftKey ? -1 : 1); }
    else if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onClose(); }
    else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") { event.preventDefault(); input.current?.select(); }
  };
  return <div role="search" aria-label="Find in transcript" className="flex shrink-0 items-center gap-1 border-b border-border px-3 py-2">
    <Input ref={input} aria-label="Find in transcript" placeholder="Find in transcript" autoComplete="off" maxLength={MAX_FIND_QUERY_LENGTH}
      className="h-8 min-w-0 flex-1 text-sm" value={query} onChange={event => onQueryChange(event.target.value)} onKeyDown={onKeyDown} />
    <span role="status" aria-live="polite" className="min-w-[4.5rem] shrink-0 text-right text-xs tabular-nums text-muted-foreground">{status}</span>
    <Tooltip><TooltipTrigger asChild>
      <Button variant="ghost" size="icon" className="h-8 w-8" aria-label="Previous match" disabled={total === 0} onClick={() => onStep(-1)}><ChevronUp /></Button>
    </TooltipTrigger><TooltipContent>Previous match (Shift+Enter)</TooltipContent></Tooltip>
    <Tooltip><TooltipTrigger asChild>
      <Button variant="ghost" size="icon" className="h-8 w-8" aria-label="Next match" disabled={total === 0} onClick={() => onStep(1)}><ChevronDown /></Button>
    </TooltipTrigger><TooltipContent>Next match (Enter)</TooltipContent></Tooltip>
    <Tooltip><TooltipTrigger asChild>
      <Button variant="ghost" size="icon" className="h-8 w-8" aria-label="Close find" onClick={onClose}><X /></Button>
    </TooltipTrigger><TooltipContent>Close (Esc)</TooltipContent></Tooltip>
  </div>;
}
