"use client";

import {
  DropdownMenuItem,
} from '@/components/ui/dropdown-menu';
import { Save, Search } from 'lucide-react';

interface SummaryUpdaterButtonGroupProps {
  isSaving: boolean;
  isDirty: boolean;
  onSave: () => Promise<void>;
  onFind?: () => void;
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
  hasSummary, onReplace, onRestore, disabled,
}: SummaryUpdaterButtonGroupProps) {
  return <>
    <DropdownMenuItem disabled={isSaving || disabled || !isDirty} onSelect={() => { void onSave(); }}><Save className="mr-2 h-4 w-4" />{isSaving ? 'Saving…' : 'Save'}</DropdownMenuItem>
    {onFind && <DropdownMenuItem disabled={!hasSummary || disabled} onSelect={onFind}><Search className="mr-2 h-4 w-4" />Find in summary</DropdownMenuItem>}
    {onReplace && <DropdownMenuItem disabled={!hasSummary || disabled} onSelect={onReplace}>Replace text</DropdownMenuItem>}
    {onRestore && <DropdownMenuItem disabled={isSaving || disabled} onSelect={onRestore}>Restore previous summary</DropdownMenuItem>}
  </>;
}
