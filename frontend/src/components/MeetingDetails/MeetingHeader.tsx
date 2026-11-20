'use client';

import { MoreHorizontal } from 'lucide-react';
import { useRef, useState, type ReactNode } from 'react';
import { EditableTitle } from '@/components/EditableTitle';
import { ActionIcon } from './ActionIcon';
import { ProjectTags } from './ProjectTags';
import { MeetingExportButtons } from './MeetingExportButtons';
import { ReviewedActions } from './ReviewedActions';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu';

export function MeetingHeader({ meeting, title, isEditing, onStartEditing, onFinishEditing, onChange, duration, segmentCount, disabled, getMarkdown, onOpenFolder, attendance }: {
  meeting: { id: string; created_at: string; folder_path?: string | null };
  title: string; isEditing: boolean; onStartEditing: () => void; onFinishEditing: () => void;
  onChange: (title: string) => void; duration?: number; segmentCount: number; disabled: boolean;
  getMarkdown: () => Promise<string>; onOpenFolder: () => Promise<void>;
  attendance?: ReactNode;
}) {
  const [actionsOpen, setActionsOpen] = useState(false);
  const retainActionFocusRef = useRef(false);
  const overflowRef = useRef<HTMLButtonElement>(null);
  const date = new Date(meeting.created_at);
  const seconds = Math.max(0, Math.floor(duration || 0));
  const length = seconds >= 3600
    ? `${Math.floor(seconds / 3600)}:${String(Math.floor(seconds / 60) % 60).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`
    : `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
  return <header className="flex shrink-0 items-center gap-3 border-b border-border bg-card px-5 py-4">
    <div className="min-w-0 flex-1 space-y-1">
      <EditableTitle title={title} isEditing={isEditing} onStartEditing={onStartEditing} onFinishEditing={onFinishEditing} onChange={onChange} disabled={disabled} meetingTitle />
      <div className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
        <span className="shrink-0">{Number.isNaN(date.getTime()) ? 'Meeting' : date.toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</span>
        {seconds > 0 && <><span aria-hidden>·</span><span className="font-mono">{length}</span></>}
        <span aria-hidden>·</span><span className="shrink-0">{segmentCount} segments</span>
        <span aria-hidden>·</span><ProjectTags key={meeting.id} meetingId={meeting.id} compact />
      </div>
    </div>
    {attendance}
    <MeetingExportButtons key={meeting.id} meetingId={meeting.id} meetingTitle={title} meetingCreatedAt={meeting.created_at} disabled={disabled} getMarkdown={getMarkdown} onReviewActions={() => setActionsOpen(true)} />
    <DropdownMenu><DropdownMenuTrigger asChild><ActionIcon ref={overflowRef} label="More meeting actions" className="h-9 w-9 shrink-0"><MoreHorizontal /></ActionIcon></DropdownMenuTrigger>
      <DropdownMenuContent align="end" onCloseAutoFocus={event => {
        if (retainActionFocusRef.current) event.preventDefault();
        retainActionFocusRef.current = false;
      }}>
        <DropdownMenuItem onSelect={() => { retainActionFocusRef.current = true; setActionsOpen(true); }}>Meeting actions</DropdownMenuItem>
        <DropdownMenuItem disabled={disabled} onSelect={() => { retainActionFocusRef.current = true; onStartEditing(); }}>Rename meeting</DropdownMenuItem>
        {meeting.folder_path && <DropdownMenuItem onSelect={() => void onOpenFolder()}>Open recording folder</DropdownMenuItem>}
      </DropdownMenuContent>
    </DropdownMenu>
    <ReviewedActions key={meeting.id} open={actionsOpen} onOpenChange={setActionsOpen} meetingId={meeting.id}
      getMarkdown={getMarkdown} onReturnFocus={() => overflowRef.current?.focus()} />
  </header>;
}
