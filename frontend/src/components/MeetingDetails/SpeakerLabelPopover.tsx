"use client";

import { useCallback, useId, useRef, useState, type KeyboardEvent } from "react";
import { Check } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { getSpeakerColor, getSpeakerOptions, getSpeakerRename, normalizeSpeakerName, speakerLabel, type SpeakerDetails } from "@/lib/speaker-labels";

export type RenameSpeaker = (from: string | null | undefined, to: string | null) => Promise<number> | number | void;

function useSpeakerSave() {
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);
  const save = async (operation: () => unknown, complete: () => void) => {
    if (pending.current) return;
    pending.current = true;
    setSaving(true);
    try {
      await operation();
      complete();
    } catch {
      toast.error("Could not save the speaker label. Please try again.");
    } finally {
      pending.current = false;
      setSaving(false);
    }
  };
  return { saving, save };
}

export function SpeakerDot({ color }: { color: string }) {
  return <span aria-hidden="true" className="h-2 w-2 shrink-0 rounded-full" style={{ backgroundColor: color }} />;
}

function RenameSpeakerForm({ speaker, catalog, saving, onRename }: {
  speaker: string; catalog: readonly SpeakerDetails[]; saving: boolean; onRename: (name: string) => void;
}) {
  const [value, setValue] = useState(speaker);
  const id = useId();
  const selectInput = useCallback((input: HTMLInputElement | null) => { if (input) { input.focus(); input.select(); } }, []);
  const rename = getSpeakerRename(catalog, speaker, value);
  return <form className="space-y-3" onSubmit={event => { event.preventDefault(); if (rename.name && !saving) onRename(rename.name); }}>
    <label htmlFor={id} className="block text-sm font-semibold">Rename speaker</label>
    <Input id={id} aria-describedby={`${id}-help`} value={value} maxLength={64} disabled={saving}
      ref={selectInput}
      onChange={event => setValue(event.target.value)} />
    <p id={`${id}-help`} className="text-xs text-muted-foreground">
      {rename.merge ? `Merges with ${rename.merge}` : `Applies to ${rename.count} segments`}
    </p>
    <Button type="submit" size="sm" disabled={saving || !rename.name}>{rename.merge ? "Merge" : "Rename"}</Button>
  </form>;
}

export function RenameSpeakerPopover({ speaker, catalog, onRename, onOpenChange }: {
  speaker: string; catalog: readonly SpeakerDetails[]; onRename: RenameSpeaker; onOpenChange?: (open: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const destination = useRef(speaker);
  const { saving, save } = useSpeakerSave();
  return <Popover open={open} onOpenChange={next => { if (!saving) { setOpen(next); onOpenChange?.(next); } }}>
    <PopoverTrigger asChild>
      <Button ref={trigger} variant="ghost" type="button" disabled={saving} aria-label={`Rename ${speaker}`}
        data-speaker-lane-name={speaker} title={speaker}
        className="h-auto min-w-0 justify-start gap-2 px-1 py-1 text-xs font-medium text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:bg-muted data-[state=open]:bg-muted data-[state=open]:text-foreground">
        <SpeakerDot color={getSpeakerColor(catalog, speaker)} /><span className="truncate">{speaker}</span>
      </Button>
    </PopoverTrigger>
    <PopoverContent align="start" aria-label={`Rename ${speaker}`} onOpenAutoFocus={event => event.preventDefault()}
      onEscapeKeyDown={event => { if (saving) event.preventDefault(); }}
      onCloseAutoFocus={event => {
        event.preventDefault();
        const target = trigger.current?.isConnected ? trigger.current : Array.from(document.querySelectorAll<HTMLButtonElement>('[data-speaker-lane-name]')).find(button => button.dataset.speakerLaneName === destination.current);
        target?.focus();
      }}>
      <RenameSpeakerForm key={speaker} speaker={speaker} catalog={catalog} saving={saving}
        onRename={name => {
          destination.current = name;
          void save(() => onRename(speaker, name), () => { setOpen(false); onOpenChange?.(false); });
        }} />
    </PopoverContent>
  </Popover>;
}

export function SpeakerLabelPopover({ segmentId, timestamp, speaker, catalog, sourceAttributionEnabled = false, onSpeakerChange, onRename }: {
  segmentId: string; timestamp: string; speaker?: string;
  catalog: readonly SpeakerDetails[]; sourceAttributionEnabled?: boolean;
  onSpeakerChange: (id: string, name: string | null) => Promise<void> | void;
  onRename?: RenameSpeaker;
}) {
  const current = speakerLabel(speaker);
  const [open, setOpen] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [value, setValue] = useState("");
  const trigger = useRef<HTMLButtonElement>(null);
  const id = useId();
  const { saving, save } = useSpeakerSave();
  const options = getSpeakerOptions(catalog, sourceAttributionEnabled);
  const nextName = normalizeSpeakerName(value);
  const close = () => { setOpen(false); setValue(""); };
  const assign = (name: string | null) => save(() => onSpeakerChange(segmentId, name), close);
  const moveOption = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'));
    if (!buttons.length) return;
    const index = buttons.indexOf(event.target as HTMLButtonElement);
    const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
    event.preventDefault();
    buttons[next].focus();
  };
  return <Popover open={open} onOpenChange={next => { if (!saving) { setOpen(next); if (next) setRenaming(false); } }}>
    <PopoverTrigger asChild>
      <Button ref={trigger} variant="ghost" type="button" disabled={saving}
        aria-label={`Change speaker for segment at ${timestamp}, currently ${current ?? "unlabeled"}`}
        className={`mb-0.5 h-auto max-w-[11rem] justify-start gap-1.5 rounded px-1.5 py-0.5 text-[10px] font-semibold ${current ? "bg-muted text-foreground hover:bg-muted/80 hover:text-foreground" : "border border-dashed border-border text-muted-foreground hover:bg-muted hover:text-foreground"}`}>
        {current && <SpeakerDot color={getSpeakerColor(catalog, current)} />}
        <span className="truncate">{current ?? "Add speaker"}</span>
      </Button>
    </PopoverTrigger>
    <PopoverContent align="start" aria-labelledby={`${id}-title`}
      onCloseAutoFocus={event => { event.preventDefault(); trigger.current?.focus(); }}
      onEscapeKeyDown={event => { if (saving) event.preventDefault(); }}>
      <h3 id={`${id}-title`} className={`text-sm font-semibold ${renaming ? "sr-only" : "mb-2"}`}>{renaming ? "Rename speaker" : "Who said this?"}</h3>
      {renaming && current && onRename ? <RenameSpeakerForm speaker={current} catalog={catalog} saving={saving}
        onRename={name => void save(() => onRename(current, name), close)} /> : <>
        <div role="group" aria-label="Meeting speakers" className="max-h-52 space-y-1 overflow-y-auto" onKeyDown={moveOption}>
          {options.map(name => <Button key={name} variant="ghost" type="button" disabled={saving}
            aria-label={`Assign ${name}`} aria-pressed={name === current} onClick={() => void assign(name)}
            className="h-auto w-full justify-start gap-2 px-2 py-1.5 text-sm">
            <SpeakerDot color={getSpeakerColor(catalog, name)} /><span className="min-w-0 flex-1 truncate text-left">{name}</span>
            {name === current && <Check aria-hidden="true" className="h-4 w-4 shrink-0" />}
          </Button>)}
        </div>
        <form className="mt-3 space-y-1.5" onSubmit={event => { event.preventDefault(); if (nextName) void assign(nextName); }}>
          <label htmlFor={id} className="text-xs font-medium">New speaker</label>
          <div className="flex gap-2">
            <Input id={id} placeholder="New speaker" value={value} maxLength={64} disabled={saving} onChange={event => setValue(event.target.value)} />
            <Button size="sm" type="submit" disabled={saving || !nextName}>Add</Button>
          </div>
        </form>
        {current && <>
          <Separator className="my-3" />
          {onRename && <Button variant="ghost" type="button" disabled={saving} onClick={() => setRenaming(true)}
            className="h-auto w-full justify-start whitespace-normal px-2 py-1.5 text-left text-xs">Rename {current} everywhere</Button>}
          <Button variant="ghost" type="button" disabled={saving} onClick={() => void assign(null)}
            className="h-auto w-full justify-start px-2 py-1.5 text-xs text-muted-foreground">Remove label</Button>
        </>}
      </>}
    </PopoverContent>
  </Popover>;
}
