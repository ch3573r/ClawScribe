import { useState, useCallback, useRef, useEffect, useMemo } from 'react';
import { Summary } from '@/types';
import { BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { CurrentMeeting, useSidebar } from '@/components/Sidebar/SidebarProvider';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { meetingDraft, reportSaveError, SummaryDraft } from '@/lib/meetingDrafts';

interface UseMeetingDataProps {
  meeting: any;
  summaryData: Summary | null;
  onMeetingUpdated?: () => Promise<void>;
}

export function useMeetingData({ meeting, summaryData }: UseMeetingDataProps) {
  const draft = useMemo(() => meetingDraft(meeting.id), [meeting.id]);
  const [meetingTitle, setMeetingTitle] = useState(draft.edits.title ?? meeting.title ?? '+ New Call');
  const [isEditingTitle, setIsEditingTitle] = useState(false);
  const [aiSummary, setAiSummary] = useState<Summary | null>((draft.edits.summary as Summary) ?? summaryData);
  const [hasPreviousSummary, setHasPreviousSummary] = useState(false);
  const [summaryRevision, setSummaryRevision] = useState(0);
  const [, refresh] = useState(0);
  const manualSave = useRef(false);
  const restoring = useRef(false);
  const [isRestoring, setIsRestoring] = useState(false);
  const blockNoteSummaryRef = useRef<BlockNoteSummaryViewRef>(null);
  const { setCurrentMeeting, setMeetings } = useSidebar();

  useEffect(() => {
    const unsubscribe = draft.subscribe(() => refresh(value => value + 1));
    return () => { unsubscribe(); draft.detach(); };
  }, [draft]);
  useEffect(() => {
    if (!draft.edits.summary) setAiSummary(summaryData);
  }, [summaryData, draft]);
  useEffect(() => {
    let active = true;
    void invoke<{ has_previous_result: boolean }>('api_get_summary', { meetingId: meeting.id })
      .then(result => { if (active) setHasPreviousSummary(result.has_previous_result); }).catch(() => {});
    return () => { active = false; };
  }, [meeting.id, aiSummary]);

  const updateSidebar = useCallback((title: string) => {
    setMeetings(previous => previous.map((item: CurrentMeeting) => item.id === meeting.id ? { ...item, title } : item));
    setCurrentMeeting({ id: meeting.id, title });
  }, [meeting.id, setMeetings, setCurrentMeeting]);
  useEffect(() => {
    if (draft.state === 'saved') updateSidebar(meetingTitle);
  }, [draft.state, meetingTitle, updateSidebar]);

  const handleTitleChange = useCallback((title: string) => {
    setMeetingTitle(title);
    draft.edit({ title });
  }, [draft]);
  const formatSummary = useCallback((summary: Summary | SummaryDraft): SummaryDraft => {
    if ('markdown' in summary || 'summary_json' in summary) return summary as SummaryDraft;
    return { MeetingName: meetingTitle, MeetingNotes: { sections: Object.values(summary).map((section: any) => ({ title: section.title, blocks: section.blocks })) } };
  }, [meetingTitle]);
  const handleSummaryDraft = useCallback((summary: SummaryDraft, readSummary?: () => Promise<SummaryDraft>) => {
    draft.edit({ summary, readSummary });
  }, [draft]);
  const handleSummaryChange = useCallback((summary: Summary) => {
    setAiSummary(summary);
    handleSummaryDraft(formatSummary(summary));
  }, [handleSummaryDraft, formatSummary]);
  const handleSaveSummary = useCallback(async (summary: Summary | SummaryDraft) => {
    const formatted = formatSummary(summary);
    handleSummaryDraft(formatted);
    await draft.flush();
    if (!draft.dirty()) setAiSummary(formatted as Summary);
  }, [draft, formatSummary, handleSummaryDraft]);
  const flushPendingChanges = useCallback(() => draft.drain(), [draft]);
  const saveAllChanges = useCallback(async () => {
    if (manualSave.current) return;
    manualSave.current = true;
    try { await draft.flush(); toast.success('Changes saved successfully'); }
    catch { reportSaveError(); }
    finally { manualSave.current = false; }
  }, [draft]);
  const restorePreviousSummary = useCallback(async () => {
    if (restoring.current) return;
    restoring.current = true;
    setIsRestoring(true);
    try {
      await draft.drain();
      const result = await invoke<{ data: Summary; has_previous_result: boolean }>('api_restore_previous_summary', { meetingId: meeting.id });
      setAiSummary(result.data);
      setHasPreviousSummary(result.has_previous_result);
      setSummaryRevision(value => value + 1);
    } catch { toast.error('Could not restore the previous summary. Finish generation or retry saving your edits first.'); }
    finally { restoring.current = false; setIsRestoring(false); }
  }, [draft, meeting.id]);
  const updateMeetingTitle = useCallback((title: string) => {
    if (draft.edits.title !== undefined) return;
    setMeetingTitle(title);
    updateSidebar(title);
  }, [draft, updateSidebar]);

  return {
    transcripts: meeting.transcripts, meetingTitle, isEditingTitle,
    isTitleDirty: draft.edits.title !== undefined, aiSummary, isSaving: isRestoring || draft.state === 'saving',
    saveState: draft.state, hasPreviousSummary, summaryRevision, blockNoteSummaryRef,
    setMeetingTitle, setIsEditingTitle, setAiSummary,
    setIsSummaryDirty: (_dirty: boolean) => {},
    handleTitleChange, handleSummaryChange, handleSummaryDraft, handleSaveSummary,
    handleSaveMeetingTitle: flushPendingChanges, flushPendingChanges, saveAllChanges,
    updateMeetingTitle, restorePreviousSummary,
  };
}
