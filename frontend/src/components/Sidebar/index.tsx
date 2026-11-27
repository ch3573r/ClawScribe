"use client";

import React, { useState, useMemo, useEffect, useCallback, useRef } from "react";
import {
  ArrowRight,
  File,
  Settings,
  ChevronRightCircle,
  PanelLeftClose,
  Home,
  Trash2,
  Mic,
  Square,
  Pencil,
  Play,
  NotebookPen,
  SearchIcon,
  X,
  Upload,
} from "lucide-react";
import { useRouter, usePathname } from "next/navigation";
import { useSidebar } from "./SidebarProvider";
import { ConfirmationModal } from "../ConfirmationModel/confirmation-modal";
import { meetingDraft } from "@/lib/meetingDrafts";
import Analytics from "@/lib/analytics";
import { groupMeetings, shortMeetingDate } from "@/lib/sidebarMeetings";
import { Meta, StatusText } from "@/components/ui/settings-layout";
import { NextUpCard } from "./NextUpCard";
import { SidebarImportStatus } from "./SidebarImportStatus";
import { useActiveGlobalShortcuts } from "@/hooks/useActiveGlobalShortcuts";
import { shortcutHint } from "@/lib/shortcuts";
import { clampSidebarWidth, DEFAULT_SIDEBAR_WIDTH, MIN, MAX, storeSidebarWidth } from "@/lib/sidebarWidth";
import { invoke } from "@tauri-apps/api/core";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { toast } from "sonner";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import { useImportDialog } from "@/contexts/ImportDialogContext";
import { useSummaryConfig } from "@/contexts/ConfigContext";
import { useResultNotes } from "@/hooks/useResultNotes";
import { useLibrarySearch } from "@/hooks/useLibrarySearch";
import { openMatchScope, savedPassageUrl, sourceLabel, supportsPersonalExcerpts, type SavedHit } from "@/lib/archive-search";
import { SearchSnippet } from "@/components/Archive/SearchSnippet";
import { transcriptPassageUrl } from "@/lib/knowledge-navigation";

import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog";
import { VisuallyHidden } from "@/components/ui/visually-hidden";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";

import Logo from "../Logo";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "../ui/input-group";

declare global {
  interface Window {
    openSettings?: () => void;
  }
}

interface SidebarItem {
  id: string;
  title: string;
  created_at?: string;
  type: "folder" | "file";
  children?: SidebarItem[];
}

const Sidebar: React.FC = () => {
  const router = useRouter();
  const { modelConfig } = useSummaryConfig();
  const pathname = usePathname();
  const {
    deleteTarget, setDeleteTarget,
    currentMeeting,
    setCurrentMeeting,
    sidebarItems,
    isCollapsed,
    sidebarWidth,
    setSidebarWidth,
    sidebarMaxWidth,
    isSidebarResizing,
    setIsSidebarResizing,
    toggleCollapse,
    handleRecordingToggle,
    recordingActionReason,
    meetings,
    meetingsError,
    meetingsLoading,
    refetchMeetings,
    setMeetings,
  } = useSidebar();

  const resizeDrag = useRef<{
    pointerId: number; startX: number; startWidth: number; width: number;
    handle: HTMLDivElement; userSelect: string;
  } | null>(null);
  const stopResize = useCallback((save: boolean) => {
    const drag = resizeDrag.current;
    if (!drag) return;
    resizeDrag.current = null;
    document.body.style.userSelect = drag.userSelect;
    setIsSidebarResizing(false);
    if (save) storeSidebarWidth(clampSidebarWidth(drag.width, window.innerWidth));
    if (drag.handle.hasPointerCapture(drag.pointerId)) drag.handle.releasePointerCapture(drag.pointerId);
  }, [setIsSidebarResizing]);
  useEffect(() => () => stopResize(false), [stopResize, isCollapsed, pathname]);

  const saveWidth = (width: number) => {
    const clamped = clampSidebarWidth(width, window.innerWidth);
    setSidebarWidth(clamped);
    storeSidebarWidth(clamped);
  };

  // Get recording state from RecordingStateContext (single source of truth)
  const { isRecording, isPaused } = useRecordingState();
  const shortcuts = useActiveGlobalShortcuts();
  const recordingAction = isRecording ? "Stop recording" : "Start recording";
  const recordingActionHint = recordingActionReason ?? shortcutHint(recordingAction, shortcuts.startStop);
  const { openImportDialog } = useImportDialog();
  const [sidebarQuery, handleSearchChange] = useState("");
  const showSidebarSearch = pathname !== "/meetings";
  const searchQuery = showSidebarSearch ? sidebarQuery : "";
  const { results: searchResults, searching: isSearching, error: searchError } = useLibrarySearch(searchQuery, !isCollapsed && pathname !== "/settings", undefined, true, meetings);
  const passageMatches = useMemo(() => new Map(searchResults?.passages.map(hit => [hit.meeting_id, hit])), [searchResults]);
  const titleMatches = useMemo(() => new Map(searchResults?.titles.map(hit => [hit.id, hit])), [searchResults]);
  const savedMatches = useMemo(() => {
    const grouped = new Map<string, SavedHit[]>();
    for (const hit of searchResults?.saved ?? []) {
      const id = hit.reference.meeting_id;
      grouped.set(id, [...(grouped.get(id) ?? []), hit]);
    }
    return grouped;
  }, [searchResults]);
  const noteSelection = useResultNotes(searchQuery, supportsPersonalExcerpts(modelConfig?.provider));
  const [showAllMeetings, setShowAllMeetings] = useState(false);
  const appVersion = process.env.NEXT_PUBLIC_APP_VERSION ?? "";

  const [deleteFiles, setDeleteFiles] = useState(true);
  const [movingToTrash, setMovingToTrash] = useState(false);
  const deletingMeeting = useRef(false);
  useEffect(() => { setDeleteFiles(true); }, [deleteTarget]);

  const [editModalState, setEditModalState] = useState<{
    isOpen: boolean;
    meetingId: string | null;
    currentTitle: string;
  }>({
    isOpen: false,
    meetingId: null,
    currentTitle: "",
  });
  const [editingTitle, setEditingTitle] = useState<string>("");
  const filteredSidebarItems = useMemo(() => {
    if (!searchQuery.trim()) return sidebarItems;
    const ids = new Set([
      ...(searchResults?.titles.map(hit => hit.id) ?? []),
      ...(searchResults?.passages.map(hit => hit.meeting_id) ?? []),
      ...savedMatches.keys(),
    ]);
    return sidebarItems.flatMap(item => item.type === "folder"
      ? [{ ...item, children: item.children?.filter(child => ids.has(child.id)) }]
      : ids.has(item.id) ? [item] : []);
  }, [sidebarItems, searchQuery, searchResults, savedMatches]);

  const handleDelete = async (itemId: string) => {
    if (deletingMeeting.current) return;
    deletingMeeting.current = true;
    setMovingToTrash(true);

    try {
      await meetingDraft(itemId).drain();
      await invoke("api_delete_meeting", {
        meetingId: itemId,
        deleteFiles,
      });
      setMeetings(previous => previous.filter(meeting => meeting.id !== itemId));

      // Track meeting deletion
      Analytics.trackMeetingDeleted(itemId);

      // Show success toast
      toast.success("Meeting moved to Trash", {
        description: "Restore it from Meetings → Trash within 30 days.",
      });
      setDeleteTarget(null);

      // If deleting the active meeting, navigate to home
      if (currentMeeting?.id === itemId) {
        setCurrentMeeting(previous => previous?.id === itemId ? null : previous);
        if (pathname === "/meeting-details") router.push("/");
      }
    } catch (error) {
      toast.error("Could not move meeting to Trash", {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      deletingMeeting.current = false;
      setMovingToTrash(false);
    }
  };

  const handleDeleteConfirm = () => {
    if (deleteTarget) void handleDelete(deleteTarget.id);
  };

  // Handle modal editing of meeting names
  const handleEditStart = (meetingId: string, currentTitle: string) => {
    setEditModalState({
      isOpen: true,
      meetingId: meetingId,
      currentTitle: currentTitle,
    });
    setEditingTitle(currentTitle);
  };

  const handleEditConfirm = async () => {
    const newTitle = editingTitle.trim();
    const meetingId = editModalState.meetingId;

    if (!meetingId) return;

    // Prevent empty titles
    if (!newTitle) {
      toast.error("Meeting title cannot be empty");
      return;
    }

    try {
      await invoke("api_save_meeting_title", {
        meetingId: meetingId,
        title: newTitle,
      });

      // Update local state
      setMeetings(previous => previous.map(m =>
        m.id === meetingId ? { ...m, title: newTitle } : m,
      ));

      // Update current meeting if it's the one being edited
      setCurrentMeeting(previous => previous?.id === meetingId ? { ...previous, title: newTitle } : previous);

      // Track the edit
      Analytics.trackButtonClick("edit_meeting_title", "sidebar");

      toast.success("Meeting title updated successfully");

      // Close modal and reset state
      setEditModalState({ isOpen: false, meetingId: null, currentTitle: "" });
      setEditingTitle("");
    } catch (error) {
      console.error("Failed to update meeting title:", error);
      toast.error("Failed to update meeting title", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const handleEditCancel = () => {
    setEditModalState({ isOpen: false, meetingId: null, currentTitle: "" });
    setEditingTitle("");
  };

  // Expose settings navigation to the Tauri tray.
  useEffect(() => {
    window.openSettings = () => {
      router.push("/settings");
    };

    return () => {
      delete window.openSettings;
    };
  }, [router]);

  const renderCollapsedIcons = () => {
    if (!isEffectivelyCollapsed) return null;

    const isHomePage = pathname === "/";
    const isMeetingPage = pathname?.includes("/meeting-details");
    const isMeetingsPage = pathname === "/meetings";
    const isSettingsPage = pathname === "/settings";
    const collapsedNavItems = [
      {
        label: "Home",
        icon: Home,
        active: isHomePage,
        onClick: () => router.push("/"),
      },
      {
        label: "Meetings",
        icon: NotebookPen,
        active: isMeetingsPage || isMeetingPage,
        onClick: goToMeetings,
      },
    ];
    const statusDot = isRecording
      ? isPaused
        ? "bg-warning-foreground"
        : "bg-error-foreground animate-pulse"
      : "bg-success-foreground";
    const statusLabel = isRecording
      ? isPaused
        ? "Paused"
        : "Recording"
      : "Ready";

    return (
      <TooltipProvider>
        <div className="flex h-full flex-col items-center px-2 py-4">
          <div className="shrink-0">
            <Logo isCollapsed={true} />
          </div>

          <nav className="mt-6 flex flex-col items-center gap-2" aria-label="Primary">
            {collapsedNavItems.map((item) => {
              const Icon = item.icon;
              return (
                <Tooltip key={item.label}>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      onClick={item.onClick}
                      className={`whitespace-normal gap-0 p-0 [&_svg]:size-5 flex h-10 w-10 items-center justify-center rounded-md transition ${
                        item.active
                          ? "bg-primary/10 text-primary ring-1 ring-primary/20 hover:bg-primary/10 hover:text-primary"
                          : "text-muted-foreground hover:bg-sidebar-hover hover:text-sidebar-foreground"
                      }`}
                      aria-label={item.label}
                    >
                      <Icon className="h-5 w-5" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="right">
                    <p>{item.label}</p>
                  </TooltipContent>
                </Tooltip>
              );
            })}
          </nav>

          <div className="flex-1" />

          <div className="flex flex-col items-center gap-2">
            <Tooltip>
              <TooltipTrigger asChild>
                <span className="inline-flex rounded-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  tabIndex={recordingActionReason ? 0 : undefined} aria-label={recordingActionReason ?? undefined}>
                  <Button
                    variant="ghost"
                    onClick={handleRecordingToggle}
                    disabled={Boolean(recordingActionReason)}
                    className={`whitespace-normal gap-0 p-0 hover:bg-transparent hover:text-primary-foreground [&_svg]:size-5 flex h-11 w-11 items-center justify-center rounded-lg transition ${
                      isRecording
                        ? "bg-destructive text-destructive-foreground hover:bg-destructive/90 hover:text-destructive-foreground"
                        : idleRecordingButtonClass
                    }`}
                    aria-label={recordingAction}
                  >
                    {isRecording ? <Square aria-hidden="true" /> : <Mic aria-hidden="true" />}
                  </Button>
                </span>
              </TooltipTrigger>
              <TooltipContent side="right">
                <p>{recordingActionHint}</p>
              </TooltipContent>
            </Tooltip>

            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  onClick={() => openImportDialog()}
                  className="whitespace-normal gap-0 p-0 [&_svg]:size-5 flex h-10 w-10 items-center justify-center rounded-md text-muted-foreground transition hover:bg-sidebar-hover hover:text-sidebar-foreground"
                  aria-label="Import audio or video"
                >
                  <Upload className="h-5 w-5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="right">
                <p>Import audio or video</p>
              </TooltipContent>
            </Tooltip>

            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  onClick={() => openSettingsTab("general")}
                  className={`whitespace-normal gap-0 p-0 [&_svg]:size-5 flex h-10 w-10 items-center justify-center rounded-md transition ${
                    isSettingsPage
                      ? "bg-primary/10 text-primary ring-1 ring-primary/20 hover:bg-primary/10 hover:text-primary"
                      : "text-muted-foreground hover:bg-sidebar-hover hover:text-sidebar-foreground"
                  }`}
                  aria-label="Settings"
                >
                  <Settings className="h-5 w-5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="right">
                <p>Settings</p>
              </TooltipContent>
            </Tooltip>
          </div>

          <Tooltip>
            <TooltipTrigger asChild>
              <div className="mt-4 flex w-full flex-col items-center gap-2 border-t border-sidebar-border pt-3">
                <span className={`h-2.5 w-2.5 rounded-full ${statusDot}`} />
                {appVersion ? (
                  <span className="max-w-11 truncate text-[10px] leading-none text-muted-foreground">
                    v{appVersion}
                  </span>
                ) : null}
              </div>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>{statusLabel}</p>
            </TooltipContent>
          </Tooltip>
        </div>
      </TooltipProvider>
    );
  };

  const renderItem = (item: SidebarItem) => {
    const isActive = item.type === "file" && currentMeeting?.id === item.id;
    const isMeetingItem = item.id.includes("-");
    const matchingResult = passageMatches.get(item.id);
    const hasContentMatch = !!matchingResult || savedMatches.has(item.id);

    if (isCollapsed || item.type === "folder") return null;

    const open = () => {
      setCurrentMeeting({ id: item.id, title: item.title });
      router.push(`/meeting-details?id=${encodeURIComponent(item.id)}`);
    };

    return (
      <div key={item.id}>
        <div
          className={`group relative flex h-9 items-center rounded-md border transition ${
            isActive
              ? "border-primary/25 bg-primary/10 text-sidebar-foreground"
              : hasContentMatch
                ? "border-warning-border/30 bg-warning text-sidebar-foreground"
                : "border-transparent text-sidebar-foreground hover:bg-sidebar-hover"
          }`}
        >
          <button
            type="button"
            onClick={open}
            aria-current={isActive ? "page" : undefined}
            className="flex h-full min-w-0 flex-1 items-center gap-2.5 rounded-md px-2.5 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <File className={`h-4 w-4 shrink-0 ${isActive ? "text-primary" : "text-muted-foreground"}`} aria-hidden="true" />
            <span className="min-w-0 flex-1 truncate text-sm text-sidebar-foreground" title={item.title}><SearchSnippet text={item.title} highlights={titleMatches.get(item.id)?.title === item.title ? titleMatches.get(item.id)?.highlights : undefined} /></span>
            <Meta className={`shrink-0 ${isMeetingItem ? "group-focus-within:invisible group-hover:invisible" : ""}`}>
              {shortMeetingDate(item.created_at)}
            </Meta>
          </button>
          {isMeetingItem && (
            <div className="absolute right-1 top-1/2 hidden -translate-y-1/2 items-center gap-0.5 rounded-md bg-sidebar-hover group-focus-within:flex group-hover:flex">
              <Button
                variant="ghost"
                onClick={() => handleEditStart(item.id, item.title)}
                className="whitespace-normal h-auto gap-0 [&_svg]:size-3.5 rounded-md p-1.5 text-muted-foreground hover:bg-sidebar-hover hover:text-sidebar-foreground"
                aria-label={`Edit title of ${item.title}`}
              >
                <Pencil className="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                onClick={() => {
                  setDeleteFiles(true);
                  setDeleteTarget({ id: item.id, title: item.title });
                }}
                className="whitespace-normal h-auto gap-0 [&_svg]:size-3.5 rounded-md p-1.5 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                aria-label={`Move ${item.title} to Trash`}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </div>
          )}
        </div>

        {matchingResult && <div className="mt-1 flex items-start gap-1">
          <Button variant="ghost" className="block h-auto min-w-0 flex-1 whitespace-normal p-2 text-left text-xs leading-5"
            aria-label={`Open transcript match in ${item.title}`} onClick={() => {
              const url = transcriptPassageUrl(matchingResult.evidence);
              if (!url) return;
              setCurrentMeeting({ id: item.id, title: item.title });
              router.push(url);
            }}>
            <span className="line-clamp-3 break-words">Transcript · <SearchSnippet {...matchingResult} /></span>
          </Button>
          {matchingResult.start_seconds != null && Number.isFinite(matchingResult.start_seconds) && matchingResult.start_seconds >= 0 &&
            <Tooltip><TooltipTrigger asChild><Button variant="ghost" size="icon" className="h-8 w-8 shrink-0" aria-label={`Play transcript match in ${item.title}`} onClick={() => {
              const url = transcriptPassageUrl(matchingResult.evidence, false, true);
              if (!url) return;
              setCurrentMeeting({ id: item.id, title: item.title });
              router.push(url);
            }}><Play /></Button></TooltipTrigger><TooltipContent>Play passage</TooltipContent></Tooltip>}
        </div>}
        {savedMatches.get(item.id)?.map(hit => <div key={hit.reference.fingerprint}><Button variant="ghost" className="mt-1 block h-auto w-full whitespace-normal p-2 text-left text-xs leading-5"
          aria-label={`Open ${sourceLabel[hit.reference.source].toLowerCase()} match in ${item.title}`} onClick={() => {
            setCurrentMeeting({ id: item.id, title: item.title });
            router.push(savedPassageUrl(hit.reference));
          }}><span className="line-clamp-3 break-words">{sourceLabel[hit.reference.source]} · <SearchSnippet {...hit} /></span></Button>{noteSelection.control(hit)}</div>)}
      </div>
    );
  };

  const onSettings = pathname === "/settings";
  const isEffectivelyCollapsed = isCollapsed || onSettings;

  const goToMeetings = () => {
    router.push("/meetings");
  };
  const openSettingsTab = (tab: string) => {
    router.push(`/settings?tab=${tab}`);
    // If the settings page is already mounted, query-only nav won't remount it,
    // so signal the tab switch directly too.
    window.dispatchEvent(new CustomEvent("open-settings-tab", { detail: tab }));
  };

  const navItems = [
    {
      label: "Home",
      icon: Home,
      active: pathname === "/",
      onClick: () => router.push("/"),
    },
    {
      label: "Meetings",
      icon: NotebookPen,
      active: pathname === "/meetings" || pathname?.includes("/meeting-details"),
      onClick: goToMeetings,
    },
  ];
  // Meetings shown in the list: the recent ones, all of them, or the search matches.
  const visibleMeetings = filteredSidebarItems
    .filter((item) => item.type === "folder" && item.children)
    .flatMap((item) => item.children ?? [])
    .slice(0, searchQuery || showAllMeetings ? undefined : 8);
  const meetingGroups = groupMeetings(visibleMeetings);
  const idleRecordingButtonClass =
    "border border-primary/40 bg-gradient-to-br from-primary to-primary/70 text-primary-foreground shadow-[0_0_28px_hsl(var(--primary)/0.28)] hover:border-primary/50 hover:shadow-[0_0_36px_hsl(var(--primary)/0.42)]";

  return (
    <div className="fixed left-0 top-[var(--titlebar-height)] z-40 h-[calc(100vh-var(--titlebar-height))]">
      {!onSettings && isCollapsed && (
        <Button
          variant="ghost"
          onClick={toggleCollapse}
          className="whitespace-normal gap-0 p-0 [&_svg]:size-5 absolute -right-3 top-24 z-50 flex h-7 w-7 items-center justify-center rounded-full border border-sidebar-border bg-sidebar text-muted-foreground shadow-sm transition hover:bg-sidebar-hover hover:text-sidebar-foreground"
          aria-label="Expand sidebar"
        >
          <ChevronRightCircle className="h-5 w-5" />
        </Button>
      )}

      <aside
        style={isEffectivelyCollapsed ? undefined : { width: sidebarWidth }}
        className={`relative flex h-full flex-col border-r border-sidebar-border bg-sidebar text-muted-foreground shadow-sm ${isSidebarResizing ? "" : "transition-all duration-300"} ${
          isEffectivelyCollapsed ? "w-16" : ""
        }`}
      >
        {!isEffectivelyCollapsed && (
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize sidebar"
            aria-valuemin={Math.min(MIN, sidebarMaxWidth)}
            aria-valuemax={sidebarMaxWidth}
            aria-valuenow={sidebarWidth}
            tabIndex={0}
            className="absolute inset-y-0 right-0 z-40 w-[6px] cursor-col-resize touch-none hover:bg-primary/20 focus-visible:bg-primary/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
            onPointerDown={event => {
              if (event.button !== 0 || resizeDrag.current) return;
              event.preventDefault();
              event.currentTarget.setPointerCapture(event.pointerId);
              resizeDrag.current = {
                pointerId: event.pointerId, startX: event.clientX, startWidth: sidebarWidth,
                width: sidebarWidth, handle: event.currentTarget, userSelect: document.body.style.userSelect,
              };
              document.body.style.userSelect = 'none';
              setIsSidebarResizing(true);
            }}
            onPointerMove={event => {
              const drag = resizeDrag.current;
              if (!drag || drag.pointerId !== event.pointerId) return;
              drag.width = clampSidebarWidth(drag.startWidth + event.clientX - drag.startX, window.innerWidth);
              setSidebarWidth(drag.width);
            }}
            onPointerUp={event => { if (resizeDrag.current?.pointerId === event.pointerId) stopResize(true); }}
            onPointerCancel={event => { if (resizeDrag.current?.pointerId === event.pointerId) stopResize(false); }}
            onLostPointerCapture={event => { if (resizeDrag.current?.pointerId === event.pointerId) stopResize(false); }}
            onDoubleClick={() => saveWidth(DEFAULT_SIDEBAR_WIDTH)}
            onKeyDown={event => {
              const width = { ArrowLeft: sidebarWidth - 16, ArrowRight: sidebarWidth + 16, Home: MIN, End: MAX }[event.key];
              if (width === undefined) return;
              event.preventDefault();
              saveWidth(width);
            }}
          />
        )}
        {isEffectivelyCollapsed ? (
          renderCollapsedIcons()
        ) : (
          <>
            <div className="flex-shrink-0 space-y-4 border-b border-sidebar-border px-4 pb-4 pt-5">
              <div className="flex items-center justify-between gap-2">
                <Logo isCollapsed={isCollapsed} />
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={toggleCollapse}
                  className="h-8 w-8 shrink-0 text-muted-foreground hover:bg-sidebar-hover hover:text-sidebar-foreground"
                  aria-label="Collapse sidebar"
                  title="Collapse sidebar"
                >
                  <PanelLeftClose aria-hidden="true" />
                </Button>
              </div>

              {showSidebarSearch && <div className="relative">
                <InputGroup className="rounded-md border-sidebar-border bg-background/60 text-sidebar-foreground shadow-none">
                  <InputGroupInput
                    id="meeting-search"
                    aria-label="Search titles, transcripts, summaries, notes and bookmarks"
                    placeholder="Search saved meetings"
                    value={searchQuery}
                    onChange={(e) => handleSearchChange(e.target.value)}
                    className="placeholder:text-muted-foreground"
                  />
                  <InputGroupAddon>
                    <SearchIcon className="h-4 w-4 text-muted-foreground" />
                  </InputGroupAddon>
                  {searchQuery && (
                    <InputGroupAddon align="inline-end">
                      <InputGroupButton aria-label="Clear search" onClick={() => handleSearchChange("")}>
                        <X className="h-4 w-4" />
                      </InputGroupButton>
                    </InputGroupAddon>
                  )}
                </InputGroup>
              </div>}
            </div>

            <nav className="flex-shrink-0 space-y-1 px-3 pt-3">
              {navItems.map((item) => {
                const Icon = item.icon;
                return (
                  <Button
                    variant="ghost"
                    key={item.label}
                    onClick={item.onClick}
                    className={`h-auto justify-start whitespace-normal relative flex w-full items-center gap-3 rounded-md px-3 py-2.5 text-sm font-medium transition ${
                      item.active
                        ? "bg-primary/10 text-primary ring-1 ring-primary/15 hover:bg-primary/10 hover:text-primary"
                        : "text-muted-foreground hover:bg-sidebar-hover hover:text-sidebar-foreground"
                    }`}
                  >
                    {item.active && (
                      <span className="absolute bottom-2 left-0 top-2 w-0.5 rounded-full bg-primary" />
                    )}
                    <Icon className="h-4 w-4 shrink-0" />
                    <span>{item.label}</span>
                  </Button>
                );
              })}
            </nav>

            <NextUpCard />

            <div className="mt-5 flex min-h-0 flex-1 flex-col px-3">
              {isSearching && (
                <div className="mb-2 px-1">
                  <span role="status" className="text-xs text-primary">Searching…</span>
                </div>
              )}
              {searchError && <p role="alert" className="mb-2 px-1 text-xs text-destructive">{searchError}</p>}
              {searchResults?.limited && <p role="status" className="mb-2 px-1 text-xs text-muted-foreground">Limited results. Open Archive to search further.</p>}
              {searchResults && visibleMeetings.length > 0 && <Button variant="outline" size="sm" className="mb-2" onClick={() => {
                openMatchScope({ notes: noteSelection.references(visibleMeetings.flatMap(meeting => savedMatches.get(meeting.id) ?? [])), question: searchQuery.trim(), ids: visibleMeetings.map(meeting => meeting.id), project: { tags: [], mode: "any", untagged: false }, from: "", to: "" });
                if (pathname !== "/meetings") router.push("/meetings");
              }}>Ask about these results</Button>}

              <div className="flex-1 space-y-3 overflow-y-auto pr-1 custom-scrollbar">
                {meetingGroups.map((group) => (
                  <section key={group.label} aria-label={group.label}>
                    <div className="mb-1 px-1 text-xs text-muted-foreground">{group.label}</div>
                    <div className="space-y-0.5">{group.items.map((child) => renderItem(child))}</div>
                  </section>
                ))}

                {meetingsError && (
                  <div role="alert" className="rounded-md border border-error-border/30 bg-error px-3 py-3 text-sm text-error-foreground">
                    <p>{meetings.length === 0 ? meetingsError : "Could not refresh your meetings."}</p>
                    <Button variant="outline" size="sm" className="mt-2" disabled={meetingsLoading} onClick={() => void refetchMeetings()}>{meetingsLoading ? "Retrying…" : "Retry"}</Button>
                  </div>
                )}

                {meetingGroups.length === 0 && !isSearching && !searchError && !searchResults?.limited && !(meetingsError && meetings.length === 0) && (
                  <div className="rounded-md border border-sidebar-border bg-background/60 px-3 py-5 text-center text-sm text-muted-foreground">
                    No meetings found.
                  </div>
                )}
              </div>

              {!searchQuery && meetings.length > 8 && (
                <Button
                  variant="ghost"
                  onClick={() => setShowAllMeetings((value) => !value)}
                  className="whitespace-normal h-auto justify-start rounded-none py-0 hover:bg-transparent mt-3 flex items-center gap-2 px-1 text-sm font-medium text-primary hover:text-primary/80"
                >
                  {showAllMeetings ? "Show recent" : "All meetings"}
                  <ArrowRight className="h-4 w-4" />
                </Button>
              )}
            </div>

            <div className="flex-shrink-0 space-y-2.5 border-t border-sidebar-border bg-sidebar p-3">
              <Tooltip>
                <TooltipTrigger asChild>
                  <span className="block rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    tabIndex={recordingActionReason ? 0 : undefined} aria-label={recordingActionReason ?? undefined}>
                    <Button
                      variant={isRecording ? "destructive" : "default"}
                      onClick={handleRecordingToggle}
                      disabled={Boolean(recordingActionReason)}
                      className="h-10 w-full whitespace-normal text-sm font-semibold"
                    >
                      {isRecording ? <Square aria-hidden="true" /> : <Mic aria-hidden="true" />}
                      {recordingAction}
                    </Button>
                  </span>
                </TooltipTrigger>
                <TooltipContent side="right">{recordingActionHint}</TooltipContent>
              </Tooltip>

              <div className="grid grid-cols-2 gap-2">
                <Button variant="outline" onClick={() => openImportDialog()} className="min-w-0" aria-label="Import audio or video" title="Import audio or video">
                  <Upload className="h-4 w-4 shrink-0" />
                  <span className="truncate">Import</span>
                </Button>
                <Button variant="outline" onClick={() => openSettingsTab("general")} className="min-w-0">
                  <Settings className="h-4 w-4 shrink-0" />
                  <span className="truncate">Settings</span>
                </Button>
              </div>

              <div role="status" className="flex items-center justify-between gap-3 px-1 text-xs">
                <StatusText
                  tone={recordingActionReason ? "warning" : isRecording ? (isPaused ? "warning" : "error") : "ok"}
                  className={`text-xs ${isRecording && !isPaused ? "font-medium" : ""}`}
                >
                  {recordingActionReason ?? (isRecording ? (isPaused ? "Paused" : "Recording") : "Ready to record")}
                </StatusText>
                {appVersion ? <Meta>v{appVersion}</Meta> : null}
              </div>
            </div>
          </>
        )}
        <SidebarImportStatus collapsed={isEffectivelyCollapsed} />
      </aside>

      {/* Confirmation Modal for Delete */}
      <ConfirmationModal
        isOpen={!!deleteTarget}
        title={`Move “${deleteTarget?.title ?? 'meeting'}” to Trash?`}
        text="Keep this meeting and its files in Trash for 30 days. You can restore it from Meetings → Trash."
        confirmLabel={movingToTrash ? "Moving…" : "Move to Trash"}
        confirmDisabled={movingToTrash}
        onConfirm={handleDeleteConfirm}
        onCancel={() => { if (!deletingMeeting.current) setDeleteTarget(null); }}
      >
        <label className="mb-6 flex items-start gap-2 text-sm"><Checkbox checked={deleteFiles} disabled={movingToTrash} onCheckedChange={checked => setDeleteFiles(checked === true)} className="mt-1 h-[13px] w-[13px]" />Also remove verified recording files when the meeting is permanently deleted.</label>
      </ConfirmationModal>

      {/* Edit Meeting Title Modal */}
      <Dialog
        open={editModalState.isOpen}
        onOpenChange={(open) => {
          if (!open) handleEditCancel();
        }}
      >
        <DialogContent className="sm:max-w-[425px]">
          <VisuallyHidden>
            <DialogTitle>Edit meeting title</DialogTitle>
          </VisuallyHidden>
          <div className="py-4">
            <h3 className="text-lg font-semibold mb-4">Edit meeting title</h3>
            <div className="space-y-4">
              <div>
                <label
                  htmlFor="meeting-title"
                  className="block text-sm font-medium text-foreground mb-2"
                >
                  Meeting title
                </label>
                <Input
                  id="meeting-title"
                  type="text"
                  value={editingTitle}
                  onChange={(e) => setEditingTitle(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      handleEditConfirm();
                    } else if (e.key === "Escape") {
                      handleEditCancel();
                    }
                  }}
                  className="h-auto w-full px-3 py-2 border border-input bg-background rounded-md text-base md:text-base shadow-none focus:outline-none focus:ring-2 focus-visible:ring-2 focus:ring-ring focus:border-transparent"
                  placeholder="Enter meeting title"
                  autoFocus
                />
              </div>
            </div>
          </div>
          <DialogFooter>
            <Button
              variant="ghost"
              onClick={handleEditCancel}
              className="whitespace-normal h-auto gap-0 hover:text-secondary-foreground px-4 py-2 text-sm font-medium text-secondary-foreground bg-secondary hover:bg-muted rounded-md transition-colors"
            >
              Cancel
            </Button>
            <Button
              variant="ghost"
              onClick={handleEditConfirm}
              className="whitespace-normal h-auto gap-0 hover:text-primary-foreground px-4 py-2 text-sm font-medium text-primary-foreground bg-primary hover:bg-primary/90 rounded-md transition-colors"
            >
              Save
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};

export default Sidebar;
