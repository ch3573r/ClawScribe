'use client';

import React, { createContext, useContext, useState, useEffect } from 'react';
import { usePathname, useRouter } from 'next/navigation';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { MeetingTag } from '@/lib/library';
import { clampSidebarWidth, DEFAULT_SIDEBAR_WIDTH, MAX, readStoredSidebarWidth } from '@/lib/sidebarWidth';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useSummaryPolling } from '@/hooks/useSummaryPolling';


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

interface SidebarContextType extends ReturnType<typeof useSummaryPolling> {
  projectTags: MeetingTag[];
  projectTagsError: string | null;
  projectTagsLoading: boolean;
  refreshProjectTags: () => Promise<void>;
  currentMeeting: CurrentMeeting | null;
  setCurrentMeeting: (meeting: CurrentMeeting | null) => void;
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
  setServerAddress: (address: string) => void;
  serverAddress: string;
  transcriptServerAddress: string;
  setTranscriptServerAddress: (address: string) => void;
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
  const [currentMeeting, setCurrentMeeting] = useState<CurrentMeeting | null>({ id: 'intro-call', title: '+ New Call' });
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
  const [meetings, setMeetings] = useState<CurrentMeeting[]>([]);
  // Set when the library could not be loaded, so a failure is not shown as an empty library.
  const [meetingsError, setMeetingsError] = useState<string | null>(null);
  const [meetingsLoading, setMeetingsLoading] = useState(false);
  const meetingLoads = React.useRef(0);
  const [isMeetingActive, setIsMeetingActive] = useState(false);
  const [serverAddress, setServerAddress] = useState('');
  const [transcriptServerAddress, setTranscriptServerAddress] = useState('');
  const { activeSummaryPolls, startSummaryPolling, stopSummaryPolling, attachSummaryPolling } = useSummaryPolling();

  // Use recording state from RecordingStateContext (single source of truth)
  const { isRecording } = useRecordingState();

  const pathname = usePathname();
  const sidebarOffset = isCollapsed || pathname === '/settings' ? '4rem' : `${sidebarWidth}px`;
  const router = useRouter();

  // Extract fetchMeetings as a reusable function
  const fetchMeetings = React.useCallback(async () => {
    if (serverAddress) {
      meetingLoads.current++;
      setMeetingsLoading(true);
      try {
        const meetings = await invoke<CurrentMeeting[]>('api_get_meetings');
        const transformedMeetings = meetings.map(meeting => ({
          id: meeting.id,
          title: meeting.title,
          created_at: meeting.created_at,
          updated_at: meeting.updated_at,
        }));
        setMeetings(transformedMeetings);
        setMeetingsError(null);
        Analytics.trackBackendConnection(true);
      } catch (error) {
        console.error('Error fetching meetings:', error);
        // Keep what was loaded before; an empty list would pass for an empty library.
        setMeetingsError('Could not load your meetings.');
        Analytics.trackBackendConnection(false, error instanceof Error ? error.message : 'Unknown error');
      } finally {
        meetingLoads.current--;
        if (meetingLoads.current === 0) setMeetingsLoading(false);
      }
    }
  }, [serverAddress]);

  useEffect(() => {
    fetchMeetings();
  }, [serverAddress, fetchMeetings]);

  useEffect(() => {
    void refreshProjectTags();
    const subscription = listen('library-changed', () => { void refreshProjectTags(); void fetchMeetings(); });
    void subscription.catch(() => setProjectTagsError('Could not watch project tags. Reopen ClawScribe to retry.'));
    return () => { tagsVersion.current += 1; void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, [refreshProjectTags, fetchMeetings]);

  useEffect(() => {
    const fetchSettings = async () => {
      setServerAddress('http://localhost:5167');
      setTranscriptServerAddress('http://127.0.0.1:8178/stream');
    };
    fetchSettings();
  }, []);

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

  const toggleCollapse = () => {
    setIsCollapsed(!isCollapsed);
  };

  // Update current meeting when on home page
  useEffect(() => {
    if (pathname === '/') {
      setCurrentMeeting({ id: 'intro-call', title: '+ New Call' });
    }
  }, [pathname]);

  // Function to handle recording toggle from sidebar
  const handleRecordingToggle = () => {
    if (!isRecording) {
      // Check if already on home page
      if (pathname === '/') {
        // Already on home - trigger recording directly via custom event
        console.log('Triggering recording from sidebar (already on home page)');
        window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
      } else {
        // Not on home - navigate and use auto-start mechanism
        console.log('Navigating to home page with auto-start flag');
        sessionStorage.setItem('autoStartRecording', 'true');
        router.push('/');
      }

      // Track recording initiation from sidebar
      Analytics.trackButtonClick('start_recording', 'sidebar');
    } else {
      // Already recording - clicking the indicator stops the recording. The
      // app-wide RecordingPostProcessingProvider handles it, so it works from
      // any page without navigating.
      console.log('Stopping recording from sidebar');
      window.dispatchEvent(new CustomEvent('stop-recording-from-sidebar'));
      Analytics.trackButtonClick('stop_recording', 'sidebar');
    }
    // Starting is handled by the Home component; stopping by the provider.
  };

  return (
    <SidebarContext.Provider value={{
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
      setServerAddress,
      serverAddress,
      transcriptServerAddress,
      setTranscriptServerAddress,
      activeSummaryPolls,
      startSummaryPolling,
      stopSummaryPolling,
      attachSummaryPolling,
      refetchMeetings: fetchMeetings,

    }}>
      {children}
    </SidebarContext.Provider>
  );
}
