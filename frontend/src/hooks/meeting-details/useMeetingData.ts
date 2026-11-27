import { useState, useCallback, useRef, useEffect, useMemo } from 'react';
import type { EditorSummary, MeetingDetails, MeetingSummary, Summary, SummaryDataResponse } from '@/types';
import { readMeetingSummary, summaryDetails } from '@/lib/meeting-summary';
import type { SummaryJobSnapshot } from '@/lib/summary-job-status';
import { BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { CurrentMeeting, useSidebar } from '@/components/Sidebar/SidebarProvider';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { meetingDraft, reportSaveError, SummaryDraft } from '@/lib/meetingDrafts';

interface UseMeetingDataProps {
  meeting: MeetingDetails;
  summaryData: MeetingSummary | null;
  summaryJob: SummaryJobSnapshot | null;
}

export function useMeetingData({ meeting, summaryData, summaryJob }: UseMeetingDataProps) {
  const draft = useMemo(() => meetingDraft(meeting.id), [meeting.id]);
  const [meetingTitle, setMeetingTitle] = useState(draft.edits.title ?? meeting.title ?? '+ New Call');
  const [isEditingTitle, setIsEditingTitle] = useState(false);
  const [aiSummary, setAiSummary] = useState<MeetingSummary | null>(() => readMeetingSummary(draft.edits.summary).summary ?? summaryData);
  const [hasPreviousSummary, setHasPreviousSummary] = useState(false);
  const details = useMemo(() => ({
    ...summaryDetails(summaryJob?.data),
    transcriptChanged: summaryJob?.transcript_changed === true,
  }), [summaryJob?.data, summaryJob?.transcript_changed]);
  const [summaryRevision, setSummaryRevision] = useState(0);
  const [, refresh] = useState(0);
  const manualSave = useRef(false);
  const restoring = useRef(false);
  const [isRestoring, setIsRestoring] = useState(false);
  const blockNoteSummaryRef = useRef<BlockNoteSummaryViewRef>(null);
  const { setCurrentMeeting, setMeetings } = useSidebar();
  const publishedTitle = useRef({ id: meeting.id, title: meeting.title });

  useEffect(() => {
    const unsubscribe = draft.subscribe(() => refresh(value => value + 1));
    return () => { unsubscribe(); draft.detach(); };
  }, [draft]);
  useEffect(() => {
    if (!draft.edits.summary) setAiSummary(summaryData);
  }, [summaryData, draft]);
  useEffect(() => {
    setHasPreviousSummary(summaryJob?.has_previous_result === true);
  }, [summaryJob]);

  const updateSidebar = useCallback((title: string) => {
    publishedTitle.current = { id: meeting.id, title };
    setMeetings(previous => previous.map((item: CurrentMeeting) => item.id === meeting.id ? { ...item, title } : item));
    setCurrentMeeting(previous => previous?.id === meeting.id ? { ...previous, title } : previous);
  }, [meeting.id, setMeetings, setCurrentMeeting]);
  useEffect(() => {
    if (publishedTitle.current.id !== meeting.id) publishedTitle.current = { id: meeting.id, title: meeting.title };
    if (draft.state === 'saved' && publishedTitle.current.title !== meetingTitle) updateSidebar(meetingTitle);
  }, [draft.state, meeting.id, meeting.title, meetingTitle, updateSidebar]);

  const handleTitleChange = useCallback((title: string) => {
    setMeetingTitle(title);
    draft.edit({ title });
  }, [draft]);
  const handleSummaryDraft = useCallback((summary: SummaryDraft, readSummary?: () => Promise<SummaryDraft>) => {
    draft.edit({ summary, readSummary });
  }, [draft]);
  const handleSummaryChange = useCallback((summary: Summary) => {
    setAiSummary(summary);
    handleSummaryDraft({ MeetingName: meetingTitle, MeetingNotes: { sections: Object.values(summary) } });
  }, [handleSummaryDraft, meetingTitle]);
  const handleSaveSummary = useCallback(async (summary: EditorSummary) => {
    handleSummaryDraft(summary);
    await draft.flush();
    if (!draft.dirty()) setAiSummary(summary);
  }, [draft, handleSummaryDraft]);
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
      const result = await invoke<{ data: SummaryDataResponse | string | null; has_previous_result: boolean }>('api_restore_previous_summary', { meetingId: meeting.id });
      setAiSummary(readMeetingSummary(result.data).summary);
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
    saveState: draft.state, hasPreviousSummary, details, summaryRevision, blockNoteSummaryRef,
    setMeetingTitle, setIsEditingTitle, setAiSummary,
    handleTitleChange, handleSummaryChange, handleSummaryDraft, handleSaveSummary,
    handleSaveMeetingTitle: flushPendingChanges, flushPendingChanges, saveAllChanges,
    updateMeetingTitle, restorePreviousSummary,
  };
}
