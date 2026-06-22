"use client";

import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Copy, Save, Loader2, FolderOpen, MoreHorizontal, Search } from 'lucide-react';
import Analytics from '@/lib/analytics';

interface SummaryUpdaterButtonGroupProps {
  isSaving: boolean;
  isDirty: boolean;
  onSave: () => Promise<void>;
  onCopy: () => Promise<void>;
  onFind?: () => void;
  onOpenFolder: () => Promise<void>;
  hasSummary: boolean;
}

// Save stays a first-class button (highlighted when there are unsaved edits);
// the secondary actions (copy, open folder) live behind a ⋯ menu so the meeting
// toolbar isn't a wall of buttons.
export function SummaryUpdaterButtonGroup({
  isSaving,
  isDirty,
  onSave,
  onCopy,
  onFind,
  onOpenFolder,
  hasSummary,
}: SummaryUpdaterButtonGroupProps) {
  return (
    <div className="flex items-center gap-2">
      <Button
        variant="outline"
        size="sm"
        className={isDirty ? 'border-primary/40 text-primary' : ''}
        title={isSaving ? 'Saving…' : isDirty ? 'Save changes' : 'All changes saved'}
        onClick={() => {
          Analytics.trackButtonClick('save_changes', 'meeting_details');
          onSave();
        }}
        disabled={isSaving}
      >
        {isSaving ? <Loader2 className="animate-spin" /> : <Save />}
        <span className="hidden lg:inline">{isSaving ? 'Saving…' : 'Save'}</span>
      </Button>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm" disabled={!hasSummary} title="More actions">
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-48">
          <DropdownMenuItem
            onClick={() => {
              Analytics.trackButtonClick('copy_summary', 'meeting_details');
              onCopy();
            }}
          >
            <Copy className="mr-2 h-4 w-4" />
            Copy summary
          </DropdownMenuItem>
          {onFind && (
            <DropdownMenuItem
              onClick={() => {
                Analytics.trackButtonClick('find_summary', 'meeting_details');
                onFind();
              }}
            >
              <Search className="mr-2 h-4 w-4" />
              Find in summary
            </DropdownMenuItem>
          )}
          <DropdownMenuItem
            onClick={() => {
              Analytics.trackButtonClick('open_folder', 'meeting_details');
              onOpenFolder();
            }}
          >
            <FolderOpen className="mr-2 h-4 w-4" />
            Open meeting folder
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
