'use client';
import { useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog';
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui/popover';
import { Command, CommandInput, CommandList, CommandItem, CommandEmpty } from '@/components/ui/command';
import { addTag, suggestTags, removeLastTag } from '@/lib/library';

export function ProjectTags({ meetingId }: { meetingId: string }) {
  const { projectTags, projectTagsError, projectTagsLoading, refreshProjectTags } = useSidebar();
  const tags = projectTags.filter(tag => tag.meeting_id === meetingId).map(tag => tag.tag);
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<string[]>([]);
  const [query, setQuery] = useState('');
  const [pickerOpen, setPickerOpen] = useState(false);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const suggestions = suggestTags(projectTags, draft, query);
  const normalizedQuery = query.trim().replace(/\s+/g, ' ');
  const canCreate = normalizedQuery && ![...projectTags.map(tag => tag.tag), ...draft]
    .some(tag => tag.toLowerCase() === normalizedQuery.toLowerCase());
  const add = (raw: string) => {
    try {
      setDraft(addTag(draft, raw, projectTags));
      setQuery(''); setError('');
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  };
  const save = async () => {
    if (pending.current) return;
    pending.current = true; setBusy(true);
    try {
      await invoke('set_meeting_tags', { meetingId, tags: draft });
      await refreshProjectTags(); setOpen(false); toast.success('Project tags saved');
    } catch (error) { toast.error('Could not save tags', { description: String(error) }); }
    finally { pending.current = false; setBusy(false); }
  };
  return <>
    <Button variant="outline" size="sm" className="max-w-64" disabled={projectTagsLoading} title={tags.join(', ')} onClick={() => { if (projectTagsError) { void refreshProjectTags(); return; } setDraft(tags); setQuery(''); setError(''); setPickerOpen(false); setOpen(true); }}>
      <span className="truncate">{projectTagsLoading ? 'Loading project tags…' : projectTagsError ? 'Retry loading tags' : tags.length ? `Tags: ${tags.join(', ')}` : 'Project tags'}</span>
    </Button>
    <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}><DialogContent>
      <DialogHeader><DialogTitle>Project tags</DialogTitle><DialogDescription>Choose existing tags or create new ones. Up to 20 tags, each up to 60 characters.</DialogDescription></DialogHeader>
      <ul aria-label="Selected tags" className="flex flex-wrap gap-2">
        {draft.map(tag => <li key={tag.toLowerCase()} className="inline-flex max-w-full items-center gap-1 rounded-full bg-muted py-1 pl-3 pr-1 text-sm">
          <span className="break-all">{tag}</span>
          <Button variant="ghost" size="icon" className="h-6 w-6 shrink-0 rounded-full" disabled={busy} aria-label={`Remove ${tag}`} onClick={() => { setDraft(draft.filter(value => value !== tag)); setError(''); }}>×</Button>
        </li>)}
      </ul>
      <Popover open={pickerOpen} onOpenChange={setPickerOpen}>
        <PopoverTrigger asChild><Button variant="outline" role="combobox" aria-expanded={pickerOpen} aria-label="Choose project tags" disabled={busy || draft.length >= 20} className="justify-start">{draft.length >= 20 ? 'Maximum 20 tags' : 'Choose or create tags…'}</Button></PopoverTrigger>
        <PopoverContent className="w-[var(--radix-popover-trigger-width)] p-0" align="start">
          <Command shouldFilter={false}>
            <CommandInput aria-label="Search or create a project tag" value={query} onValueChange={value => { setQuery(value); setError(''); }} disabled={busy || draft.length >= 20} placeholder="Search or create a tag…" onKeyDown={event => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === ',') { event.preventDefault(); add(query); }
              if (event.key === 'Backspace' && !query) { event.preventDefault(); setDraft(removeLastTag(draft)); setError(''); }
            }} onPaste={event => {
              const pasted = event.clipboardData.getData('text');
              if (pasted.includes(',')) { event.preventDefault(); add(pasted); }
            }} />
            <CommandList>
              <CommandEmpty>No matching tags</CommandEmpty>
              {suggestions.map(tag => <CommandItem key={tag.toLowerCase()} value={tag} onSelect={() => add(tag)}>{tag}</CommandItem>)}
              {canCreate && <CommandItem value={`create:${normalizedQuery}`} onSelect={() => add(query)}>Create &quot;{normalizedQuery}&quot;</CommandItem>}
            </CommandList>
          </Command>
          {error && <p role="alert" className="p-2 text-sm text-destructive">{error}</p>}
        </PopoverContent>
      </Popover>
      {draft.length >= 20 && <p role="status" className="text-sm text-muted-foreground">Maximum 20 tags</p>}
      {error && !pickerOpen && <p role="alert" className="text-sm text-destructive">{error}</p>}
      <DialogFooter><Button disabled={busy} onClick={save}>{busy ? 'Saving…' : 'Save tags'}</Button></DialogFooter>
    </DialogContent></Dialog>
  </>;
}
