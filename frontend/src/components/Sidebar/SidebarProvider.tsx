'use client';

import React, { createContext, useContext, useState, useEffect } from 'react';
import { usePathname } from 'next/navigation';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { MeetingTag } from '@/lib/library';
import { clampSidebarWidth, DEFAULT_SIDEBAR_WIDTH, MAX, readStoredSidebarWidth } from '@/lib/sidebarWidth';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useTranscriptionConfig } from '@/contexts/ConfigContext';
import { useLiveTranscriptionReadiness } from '@/hooks/useLiveTranscriptionReadiness';
import { reportStartFailure, useStartRecordingSession } from '@/hooks/useRecordingStart';
import { audioSourceReadiness } from '@/lib/audioDevicePreferences';
import { recordingActionBlock, startReadinessReason } from '@/lib/recording-start-readiness';
import { allowLoadedMeetingDrafts, discardMeetingDraft } from '@/lib/meetingDrafts';


interface SidebarItem {
  id: string;
  title: string;
  created_at?: string;
  type: 'folder' | 'file';
  children?: SidebarItem[];
}

export interface CurrentMeeting {
  id: string;
  title: string;
  created_at?: string;
  updated_at?: string;
}

interface SidebarContextType {
  deleteTarget: CurrentMeeting | null;
  setDeleteTarget: (meeting: CurrentMeeting | null) => void;
  projectTags: MeetingTag[];
  projectTagsError: string | null;
  projectTagsLoading: boolean;
  refreshProjectTags: () => Promise<void>;
  currentMeeting: CurrentMeeting | null;
  setCurrentMeeting: React.Dispatch<React.SetStateAction<CurrentMeeting | null>>;
  sidebarItems: SidebarItem[];
  isCollapsed: boolean;
  sidebarWidth: number;
  setSidebarWidth: (width: number) => void;
  sidebarMaxWidth: number;
  sidebarOffset: string;
  isSidebarResizing: boolean;
  setIsSidebarResizing: (resizing: boolean) => void;
  toggleCollapse: () => void;
  meetings: CurrentMeeting[];
  /** Why the library could not be loaded, or null. Retry with refetchMeetings. */
  meetingsError: string | null;
  /** A load of the library is running (the first one, a retry or a refresh). */
  meetingsLoading: boolean;
  setMeetings: React.Dispatch<React.SetStateAction<CurrentMeeting[]>>;
  isMeetingActive: boolean;
  setIsMeetingActive: (active: boolean) => void;
  handleRecordingToggle: () => void;
  recordingActionReason: string | null;
  nextMeetingTitle: string;
  setNextMeetingTitle: React.Dispatch<React.SetStateAction<string>>;
  liveReadiness: ReturnType<typeof useLiveTranscriptionReadiness>;
  // Refetch meetings from backend
  refetchMeetings: () => Promise<void>;

}

const SidebarContext = createContext<SidebarContextType | null>(null);

export const useSidebar = () => {
  const context = useContext(SidebarContext);
  if (!context) {
    throw new Error('useSidebar must be used within a SidebarProvider');
  }
  return context;
};

export function SidebarProvider({ children }: { children: React.ReactNode }) {
  const [deleteTarget, setDeleteTarget] = useState<CurrentMeeting | null>(null);
  const [projectTags, setProjectTags] = useState<MeetingTag[]>([]);
  const [projectTagsError, setProjectTagsError] = useState<string | null>(null);
  const [projectTagsLoading, setProjectTagsLoading] = useState(true);
  const tagsVersion = React.useRef(0);
  const tagsLoaded = React.useRef(false);
  const refreshProjectTags = React.useCallback(async () => {
    const version = ++tagsVersion.current;
    setProjectTagsLoading(!tagsLoaded.current);
    setProjectTagsError(null);
    try {
      const tags = await invoke<MeetingTag[]>('list_meeting_tags');
      if (version === tagsVersion.current) { tagsLoaded.current = true; setProjectTags(tags); setProjectTagsError(null); }
    } catch { if (version === tagsVersion.current) setProjectTagsError('Could not load project tags.'); }
    finally { if (version === tagsVersion.current) setProjectTagsLoading(false); }
  }, []);
  const [currentMeeting, setCurrentMeeting] = useState<CurrentMeeting | null>(null);
  const [isCollapsed, setIsCollapsed] = useState(false);
  const [sidebarWidth, updateSidebarWidth] = useState(DEFAULT_SIDEBAR_WIDTH);
  const [sidebarMaxWidth, setSidebarMaxWidth] = useState(MAX);
  const [isSidebarResizing, setIsSidebarResizing] = useState(false);
  const setSidebarWidth = React.useCallback((width: number) => {
    updateSidebarWidth(clampSidebarWidth(width, window.innerWidth));
  }, []);
  useEffect(() => {
    setSidebarWidth(readStoredSidebarWidth());
    const resize = () => {
      updateSidebarWidth(width => clampSidebarWidth(width, window.innerWidth));
      setSidebarMaxWidth(clampSidebarWidth(MAX, window.innerWidth));
    };
    resize();
    window.addEventListener('resize', resize);
    return () => window.removeEventListener('resize', resize);
  }, [setSidebarWidth]);
  useEffect(() => {
    const compact = window.matchMedia('(max-width: 899px)');
    const update = () => setIsCollapsed(compact.matches);
    update();
    compact.addEventListener('change', update);
    return () => compact.removeEventListener('change', update);
  }, []);
  const [meetings, updateMeetings] = useState<CurrentMeeting[]>([]);
  // Set when the library could not be loaded, so a failure is not shown as an empty library.
  const [meetingsError, setMeetingsError] = useState<string | null>(null);
  const [meetingsLoading, setMeetingsLoading] = useState(false);
  const meetingVersion = React.useRef(0);
  const meetingLoadPending = React.useRef(false);
  const [isMeetingActive, setIsMeetingActive] = useState(false);
  const [nextMeetingTitle, setNextMeetingTitle] = useState('');
  const startingFromSidebar = React.useRef(false);

  // Use recording state from RecordingStateContext (single source of truth)
  const recording = useRecordingState();
  const { isRecording, recordingMode, isSavingMode } = recording;
  const { audioDeviceInventory, selectedDevices, devicePreferencesStatus, isSavingDevicePreferences, transcriptModelConfig } = useTranscriptionConfig();
  const { data: inventory, status: inventoryStatus, refresh } = audioDeviceInventory;
  useEffect(() => { void refresh(); }, [refresh]);
  const liveReadiness = useLiveTranscriptionReadiness(recordingMode === 'live', transcriptModelConfig.provider, transcriptModelConfig.model);
  const startReason = startReadinessReason({
    inventoryStatus, preferencesStatus: devicePreferencesStatus, mode: recordingMode,
    mic: inventory && audioSourceReadiness(inventory, 'Input', selectedDevices.micDevice),
    system: inventory && audioSourceReadiness(inventory, 'Output', selectedDevices.systemDevice),
    hasReadiness: Boolean(liveReadiness.data), readinessError: liveReadiness.error,
    modelIssue: liveReadiness.data?.modelIssue, savingDevices: isSavingDevicePreferences, savingMode: isSavingMode,
  });
  const recordingActionReason = recordingActionBlock(recording, startReason);
  const start = useStartRecordingSession(setIsMeetingActive);

  const pathname = usePathname();
  const sidebarOffset = isCollapsed || pathname === '/settings' ? '4rem' : `${sidebarWidth}px`;

  // Extract fetchMeetings as a reusable function
  const fetchMeetings = React.useCallback(async () => {
    const version = ++meetingVersion.current;
    meetingLoadPending.current = true;
    setMeetingsLoading(true);
    try {
      const meetings = await invoke<CurrentMeeting[]>('api_get_meetings');
      if (version !== meetingVersion.current) return;
      const transformedMeetings = meetings.map(meeting => ({
        id: meeting.id,
        title: meeting.title,
        created_at: meeting.created_at,
        updated_at: meeting.updated_at,
      }));
      allowLoadedMeetingDrafts(transformedMeetings);
      updateMeetings(transformedMeetings);
      setMeetingsError(null);
      Analytics.trackBackendConnection(true);
    } catch (error) {
      if (version !== meetingVersion.current) return;
      console.error('Error fetching meetings:', error);
      // Keep what was loaded before; an empty list would pass for an empty library.
      setMeetingsError('Could not load your meetings.');
      Analytics.trackBackendConnection(false, error instanceof Error ? error.message : 'Unknown error');
    } finally {
      if (version === meetingVersion.current) {
        meetingLoadPending.current = false;
        setMeetingsLoading(false);
      }
    }
  }, []);

  const setMeetings = React.useCallback<SidebarContextType['setMeetings']>(update => {
    updateMeetings(update);
    // A pre-mutation snapshot can be stale even if it is the newest request.
    // Replace it so changes to other rows from that refresh are still picked up.
    if (meetingLoadPending.current) void fetchMeetings();
  }, [fetchMeetings]);

  useEffect(() => {
    void fetchMeetings();
    return () => { meetingVersion.current += 1; meetingLoadPending.current = false; };
  }, [fetchMeetings]);

  useEffect(() => {
    let active = true;
    void refreshProjectTags();
    const subscription = listen('library-changed', () => {
      if (active) { void refreshProjectTags(); void fetchMeetings(); }
    });
    const deleted = listen<string>('library-meeting-deleted', ({ payload: id }) => {
      if (!active) return;
      discardMeetingDraft(id);
      setMeetings(rows => rows.filter(row => row.id !== id));
      void refreshProjectTags();
    });
    void subscription.catch(() => { if (active) setProjectTagsError('Could not watch project tags. Reopen ClawScribe to retry.'); });
    void deleted.catch(() => {});
    return () => {
      active = false;
      tagsVersion.current += 1;
      for (const listener of [subscription, deleted]) void listener.then(unlisten => unlisten()).catch(() => {});
    };
  }, [refreshProjectTags, fetchMeetings, setMeetings]);

  const sidebarItems = React.useMemo<SidebarItem[]>(() => [
    {
      id: 'meetings',
      title: 'Meetings',
      type: 'folder',
      children: meetings.map(meeting => ({
        id: meeting.id, title: meeting.title, created_at: meeting.created_at, type: 'file',
      })),
    },
  ], [meetings]);

  const toggleCollapse = React.useCallback(() => setIsCollapsed(collapsed => !collapsed), []);

  // Update current meeting when on home page
  useEffect(() => {
    if (pathname === '/') {
      setCurrentMeeting(null);
    }
  }, [pathname]);

  // Function to handle recording toggle from sidebar
  const handleRecordingToggle = React.useCallback(() => {
    if (recordingActionReason || startingFromSidebar.current) return;
    if (!isRecording) {
      startingFromSidebar.current = true;
      void start('sidebar_direct', undefined, nextMeetingTitle).then(() => {
        setNextMeetingTitle(current => current === nextMeetingTitle ? '' : current);
      }).catch(reportStartFailure).finally(() => { startingFromSidebar.current = false; });
      return;
    }
    window.dispatchEvent(new CustomEvent('stop-recording-from-sidebar'));
    Analytics.trackButtonClick('stop_recording', 'sidebar');
  }, [isRecording, nextMeetingTitle, recordingActionReason, start]);

  const value = React.useMemo<SidebarContextType>(() => ({
      deleteTarget, setDeleteTarget,
      projectTags,
      projectTagsError,
      projectTagsLoading,
      refreshProjectTags,
      currentMeeting,
      setCurrentMeeting,
      sidebarItems,
      isCollapsed,
      sidebarWidth,
      setSidebarWidth,
      sidebarMaxWidth,
      sidebarOffset,
      isSidebarResizing,
      setIsSidebarResizing,
      toggleCollapse,
      meetings,
      meetingsError,
      meetingsLoading,
      setMeetings,
      isMeetingActive,
      setIsMeetingActive,
      handleRecordingToggle,
      recordingActionReason,
      nextMeetingTitle,
      setNextMeetingTitle,
      liveReadiness,
      refetchMeetings: fetchMeetings,
  }), [
    deleteTarget, projectTags, projectTagsError, projectTagsLoading, refreshProjectTags,
    currentMeeting, sidebarItems, isCollapsed, sidebarWidth, setSidebarWidth,
    sidebarMaxWidth, sidebarOffset, isSidebarResizing, toggleCollapse, meetings,
    meetingsError, meetingsLoading, setMeetings, isMeetingActive, handleRecordingToggle, recordingActionReason, nextMeetingTitle,
    liveReadiness.data, liveReadiness.error, liveReadiness.refresh,
    fetchMeetings,
  ]);

  return (
    <SidebarContext.Provider value={value}>
      {children}
    </SidebarContext.Provider>
  );
}
