"use client";

import {
  DropdownMenuItem,
} from '@/components/ui/dropdown-menu';
import { Save, FolderOpen, Search } from 'lucide-react';
import Analytics from '@/lib/analytics';

interface SummaryUpdaterButtonGroupProps {
  isSaving: boolean;
  isDirty: boolean;
  onSave: () => Promise<void>;
  onFind?: () => void;
  onOpenFolder: () => Promise<void>;
  hasSummary: boolean;
  onReplace?: () => void;
  onRestore?: () => void;
  disabled?: boolean;
}

// Secondary summary actions share the toolbar overflow menu.
export function SummaryUpdaterButtonGroup({
  isSaving,
  isDirty,
  onSave,
  onFind,
  onOpenFolder,
  hasSummary, onReplace, onRestore, disabled,
}: SummaryUpdaterButtonGroupProps) {
  return <>
    <DropdownMenuItem disabled={isSaving || disabled || !isDirty} onSelect={() => { Analytics.trackButtonClick('save_changes', 'meeting_details'); void onSave(); }}><Save className="mr-2 h-4 w-4" />{isSaving ? 'Saving…' : 'Save'}</DropdownMenuItem>
    {onFind && <DropdownMenuItem disabled={!hasSummary || disabled} onSelect={onFind}><Search className="mr-2 h-4 w-4" />Find in summary</DropdownMenuItem>}
    {onReplace && <DropdownMenuItem disabled={!hasSummary || disabled} onSelect={onReplace}>Replace text</DropdownMenuItem>}
    {onRestore && <DropdownMenuItem disabled={isSaving || disabled} onSelect={onRestore}>Restore previous summary</DropdownMenuItem>}
    <DropdownMenuItem onSelect={() => void onOpenFolder()}><FolderOpen className="mr-2 h-4 w-4" />Open recording folder</DropdownMenuItem>
  </>;
}
