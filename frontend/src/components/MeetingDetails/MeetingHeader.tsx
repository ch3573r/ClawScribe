'use client';

import { MoreHorizontal } from 'lucide-react';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { toast } from 'sonner';
import { EditableTitle } from '@/components/EditableTitle';
import { ActionIcon } from './ActionIcon';
import { ProjectTags } from './ProjectTags';
import { MeetingExportButtons } from './MeetingExportButtons';
import { ReviewedActions } from './ReviewedActions';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu';
import { formatArchiveDate } from '@/lib/archive-review';
import type { FollowUpDestination } from '@/lib/meeting-follow-through';
import { FollowUpEmail } from './FollowUpEmail';
import { SavedSeriesReview } from './SeriesPreparation';

export function MeetingHeader({ meeting, title, isEditing, onStartEditing, onFinishEditing, onChange, duration, segmentCount, disabled, getMarkdown, onOpenFolder, attendance, initialActionId, initialSeriesOpen, seriesDestination }: {
  meeting: { id: string; created_at: string; folder_path?: string | null };
  title: string; isEditing: boolean; onStartEditing: () => void; onFinishEditing: () => void;
  onChange: (title: string) => void; duration?: number; segmentCount: number; disabled: boolean;
  getMarkdown: () => Promise<string>; onOpenFolder: () => Promise<void>;
  attendance?: ReactNode;
  initialActionId?: string | null;
  initialSeriesOpen?: boolean;
  seriesDestination?: FollowUpDestination | null;
}) {
  const [actionsOpen, setActionsOpen] = useState(!!initialActionId);
  const [seriesOpen, setSeriesOpen] = useState(!!initialSeriesOpen);
  const [emailOpen, setEmailOpen] = useState(false);
  const retainActionFocusRef = useRef(false);
  const overflowRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (initialActionId) setActionsOpen(true);
  }, [initialActionId]);
  useEffect(() => {
    if (initialSeriesOpen) setSeriesOpen(true);
  }, [initialSeriesOpen]);
  const seconds = Math.max(0, Math.floor(duration || 0));
  const length = seconds >= 3600
    ? `${Math.floor(seconds / 3600)}:${String(Math.floor(seconds / 60) % 60).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`
    : `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
  return <header className="flex shrink-0 items-center gap-3 border-b border-border bg-card px-5 py-4">
    <div className="min-w-0 flex-1 space-y-1">
      <EditableTitle title={title} isEditing={isEditing} onStartEditing={onStartEditing} onFinishEditing={onFinishEditing} onChange={onChange} disabled={disabled} meetingTitle />
      <div className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
        <span className="shrink-0">{formatArchiveDate(meeting.created_at)}</span>
        {seconds > 0 && <><span aria-hidden>·</span><span className="font-mono">{length}</span></>}
        <span aria-hidden>·</span><span className="shrink-0">{segmentCount} segments</span>
        <span aria-hidden>·</span><ProjectTags key={meeting.id} meetingId={meeting.id} compact />
      </div>
    </div>
    {attendance}
    <MeetingExportButtons key={meeting.id} meetingId={meeting.id} meetingTitle={title} meetingCreatedAt={meeting.created_at} disabled={disabled} getMarkdown={getMarkdown}
      onReviewActions={() => setActionsOpen(true)} onReviewEmail={() => setEmailOpen(true)} seriesDestination={seriesDestination} />
    <DropdownMenu><DropdownMenuTrigger asChild><ActionIcon ref={overflowRef} label="More meeting actions" className="h-9 w-9 shrink-0"><MoreHorizontal /></ActionIcon></DropdownMenuTrigger>
      <DropdownMenuContent align="end" onCloseAutoFocus={event => {
        if (retainActionFocusRef.current) event.preventDefault();
        retainActionFocusRef.current = false;
      }}>
        <DropdownMenuItem onSelect={() => { retainActionFocusRef.current = true; setActionsOpen(true); }}>Meeting actions</DropdownMenuItem>
        <DropdownMenuItem onSelect={() => { retainActionFocusRef.current = true; setSeriesOpen(true); }}>Recurring meeting</DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void navigator.clipboard.writeText(meeting.id)
          .then(() => toast.success('Meeting reference copied.'), () => toast.error('Could not copy the meeting reference.'))}>Copy meeting reference</DropdownMenuItem>
        <DropdownMenuItem disabled={disabled} onSelect={() => { retainActionFocusRef.current = true; onStartEditing(); }}>Rename meeting</DropdownMenuItem>
        {meeting.folder_path && <DropdownMenuItem onSelect={() => void onOpenFolder()}>Open recording folder</DropdownMenuItem>}
      </DropdownMenuContent>
    </DropdownMenu>
    <ReviewedActions key={meeting.id} open={actionsOpen} onOpenChange={setActionsOpen} meetingId={meeting.id} targetActionId={initialActionId}
      getMarkdown={getMarkdown} onReturnFocus={() => overflowRef.current?.focus()} />
    {seriesOpen && <SavedSeriesReview meetingId={meeting.id} close={() => { setSeriesOpen(false); overflowRef.current?.focus(); }} />}
    {emailOpen && <FollowUpEmail meetingId={meeting.id} close={() => setEmailOpen(false)} />}
  </header>;
}
