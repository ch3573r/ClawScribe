"use client";

import React, { useState, useMemo, useEffect, useCallback, useRef } from "react";
import {
  ArrowRight,
  File,
  Settings,
  ChevronLeftCircle,
  ChevronRightCircle,
  Home,
  Trash2,
  Mic,
  Square,
  Pencil,
  NotebookPen,
  SearchIcon,
  X,
  Upload,
} from "lucide-react";
import { useRouter, usePathname } from "next/navigation";
import { useSidebar } from "./SidebarProvider";
import type { CurrentMeeting } from "@/components/Sidebar/SidebarProvider";
import { ConfirmationModal } from "../ConfirmationModel/confirmation-modal";
import { indexedDBService } from "@/services/indexedDBService";
import Analytics from "@/lib/analytics";
import { formatSidebarMeetingDate } from "@/lib/meetingDates";
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

interface SidebarItem {
  id: string;
  title: string;
  created_at?: string;
  type: "folder" | "file";
  children?: SidebarItem[];
}

const Sidebar: React.FC = () => {
  const router = useRouter();
  const pathname = usePathname();
  // Track the active Settings tab without useSearchParams (which breaks static
  // prerender). Read it on path change and set it optimistically on click.
  const [settingsTab, setSettingsTab] = useState<string | null>(null);
  useEffect(() => {
    if (pathname === "/settings" && typeof window !== "undefined") {
      setSettingsTab(new URLSearchParams(window.location.search).get("tab"));
    } else {
      setSettingsTab(null);
    }
  }, [pathname]);
  const {
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
    searchTranscripts,
    searchResults,
    isSearching,
    meetings,
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
  const { openImportDialog } = useImportDialog();
  const [searchQuery, setSearchQuery] = useState<string>("");
  const [showAllMeetings, setShowAllMeetings] = useState(false);
  const appVersion = process.env.NEXT_PUBLIC_APP_VERSION ?? "";

  const [deleteFiles, setDeleteFiles] = useState(true);
  const deletingMeeting = useRef(false);
  const [deleteModalState, setDeleteModalState] = useState<{
    isOpen: boolean;
    itemId: string | null;
  }>({ isOpen: false, itemId: null });

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
  // Handle search input changes
  const handleSearchChange = useCallback(
    async (value: string) => {
      setSearchQuery(value);

      // If search query is empty, just return to normal view
      if (!value.trim()) return;

      // Search through transcripts
      await searchTranscripts(value);
    },
    [searchTranscripts],
  );

  // Combine search results with sidebar items
  const filteredSidebarItems = useMemo(() => {
    if (!searchQuery.trim()) return sidebarItems;

    // If we have search results, highlight matching meetings
    if (searchResults.length > 0) {
      // Get the IDs of meetings that matched in transcripts
      const matchedMeetingIds = new Set(
        searchResults.map((result) => result.id),
      );

      return sidebarItems
        .map((folder) => {
          // Always include folders in the results
          if (folder.type === "folder") {
            if (!folder.children) return folder;

            // Filter children based on search results or title match
            const filteredChildren = folder.children.filter((item) => {
              // Include if the meeting ID is in our search results
              if (matchedMeetingIds.has(item.id)) return true;

              // Or if the title matches the search query
              return item.title
                .toLowerCase()
                .includes(searchQuery.toLowerCase());
            });

            return {
              ...folder,
              children: filteredChildren,
            };
          }

          // For non-folder items, check if they match the search
          return matchedMeetingIds.has(folder.id) ||
            folder.title.toLowerCase().includes(searchQuery.toLowerCase())
            ? folder
            : undefined;
        })
        .filter((item): item is SidebarItem => item !== undefined); // Type-safe filter
    } else {
      // Fall back to title-only filtering if no transcript results
      return sidebarItems
        .map((folder) => {
          // Always include folders in the results
          if (folder.type === "folder") {
            if (!folder.children) return folder;

            // Filter children based on search query
            const filteredChildren = folder.children.filter((item) =>
              item.title.toLowerCase().includes(searchQuery.toLowerCase()),
            );

            return {
              ...folder,
              children: filteredChildren,
            };
          }

          // For non-folder items, check if they match the search
          return folder.title.toLowerCase().includes(searchQuery.toLowerCase())
            ? folder
            : undefined;
        })
        .filter((item): item is SidebarItem => item !== undefined); // Type-safe filter
    }
  }, [sidebarItems, searchQuery, searchResults]);

  const handleDelete = async (itemId: string) => {
    if (deletingMeeting.current) return;
    deletingMeeting.current = true;
    console.log("Deleting item:", itemId);
    const payload = {
      meetingId: itemId,
    };

    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const metadata = await invoke<{ folder_path?: string | null }>('api_get_meeting_metadata', { meetingId: itemId });
      const result = await invoke<{ warnings: string[] }>("api_delete_meeting", {
        meetingId: itemId,
        deleteFiles,
      });
      try {
        await indexedDBService.deleteMeeting(itemId, metadata.folder_path);
      } catch {
        toast.warning('Meeting deleted, but its cached recovery copy could not be removed.');
      }
      console.log("Meeting deleted successfully");
      const updatedMeetings = meetings.filter(
        (m: CurrentMeeting) => m.id !== itemId,
      );
      setMeetings(updatedMeetings);

      // Track meeting deletion
      Analytics.trackMeetingDeleted(itemId);

      // Show success toast
      toast.success("Meeting deleted successfully", {
        description: deleteFiles ? "The meeting was removed from your library." : "Recording files were kept.",
      });

      for (const warning of result.warnings ?? []) toast.warning(warning, { duration: 15000 });

      // If deleting the active meeting, navigate to home
      if (currentMeeting?.id === itemId) {
        setCurrentMeeting({ id: "intro-call", title: "+ New Call" });
        router.push("/");
      }
    } catch (error) {
      console.error("Failed to delete meeting:", error);
      toast.error("Failed to delete meeting", {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      deletingMeeting.current = false;
    }
  };

  const handleDeleteConfirm = () => {
    if (deleteModalState.itemId) {
      handleDelete(deleteModalState.itemId);
    }
    setDeleteModalState({ isOpen: false, itemId: null });
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
      const updatedMeetings = meetings.map((m: CurrentMeeting) =>
        m.id === meetingId ? { ...m, title: newTitle } : m,
      );
      setMeetings(updatedMeetings);

      // Update current meeting if it's the one being edited
      if (currentMeeting?.id === meetingId) {
        setCurrentMeeting({ id: meetingId, title: newTitle });
      }

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
    (window as any).openSettings = () => {
      router.push("/settings");
    };

    return () => {
      delete (window as any).openSettings;
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
                <Button
                  variant="ghost"
                  onClick={handleRecordingToggle}
                  className={`whitespace-normal gap-0 p-0 hover:bg-transparent hover:text-primary-foreground [&_svg]:size-5 flex h-11 w-11 items-center justify-center rounded-lg transition ${
                    isRecording
                      ? "bg-destructive text-destructive-foreground hover:bg-destructive/90 hover:text-destructive-foreground"
                      : idleRecordingButtonClass
                  }`}
                  aria-label={isRecording ? "Stop recording" : "Start recording"}
                >
                  {isRecording ? (
                    <Square className="h-5 w-5" />
                  ) : (
                    <Mic className="h-5 w-5" />
                  )}
                </Button>
              </TooltipTrigger>
              <TooltipContent side="right">
                <p>
                  {isRecording
                    ? isPaused
                      ? "Paused — click to stop"
                      : "Recording — click to stop"
                    : "Start recording"}
                </p>
              </TooltipContent>
            </Tooltip>

            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  onClick={() => openImportDialog()}
                  className="whitespace-normal gap-0 p-0 [&_svg]:size-5 flex h-10 w-10 items-center justify-center rounded-md text-muted-foreground transition hover:bg-sidebar-hover hover:text-sidebar-foreground"
                  aria-label="Import audio"
                >
                  <Upload className="h-5 w-5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="right">
                <p>Import audio</p>
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

  // Find matching transcript snippet for a meeting item
  const findMatchingSnippet = (itemId: string) => {
    if (!searchQuery.trim() || !searchResults.length) return null;
    return searchResults.find((result) => result.id === itemId);
  };

  const renderItem = (item: SidebarItem, depth = 0) => {
    const isActive = item.type === "file" && currentMeeting?.id === item.id;
    const isMeetingItem =
      item.id.includes("-") && !item.id.startsWith("intro-call");
    const matchingResult = isMeetingItem ? findMatchingSnippet(item.id) : null;
    const hasTranscriptMatch = !!matchingResult;

    if (isCollapsed || item.type === "folder") return null;

    return (
      <div
        key={item.id}
        onClick={() => {
          setCurrentMeeting({ id: item.id, title: item.title });
          const basePath = item.id.startsWith("intro-call")
            ? "/"
            : `/meeting-details?id=${item.id}`;
          router.push(basePath);
        }}
        className={`group cursor-pointer rounded-md border px-2.5 py-2.5 transition ${
          isActive
            ? "border-primary/25 bg-primary/10 text-sidebar-foreground shadow-sm"
            : hasTranscriptMatch
              ? "border-warning-border/30 bg-warning text-sidebar-foreground"
              : "border-transparent text-sidebar-foreground hover:border-sidebar-border hover:bg-sidebar-hover"
        }`}
      >
        <div className="flex items-start gap-2.5">
          <div
            className={`mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-md ${
              isActive
                ? "bg-primary/10 text-primary"
                : "bg-background/60 text-muted-foreground"
            }`}
          >
            <File className="h-4 w-4" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="line-clamp-2 text-sm font-medium leading-5 text-sidebar-foreground">
              {item.title}
            </div>
            <div className="mt-1 text-xs text-muted-foreground">{formatSidebarMeetingDate(item.created_at)}</div>
          </div>
          {isMeetingItem && (
            <div className="flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
              <Button
                variant="ghost"
                onClick={(e) => {
                  e.stopPropagation();
                  handleEditStart(item.id, item.title);
                }}
                className="whitespace-normal h-auto gap-0 [&_svg]:size-3.5 rounded-md p-1.5 text-muted-foreground hover:bg-sidebar-hover hover:text-sidebar-foreground"
                aria-label="Edit meeting title"
              >
                <Pencil className="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                onClick={(e) => {
                  e.stopPropagation();
                  setDeleteFiles(true);
                  setDeleteModalState({ isOpen: true, itemId: item.id });
                }}
                className="whitespace-normal h-auto gap-0 [&_svg]:size-3.5 rounded-md p-1.5 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                aria-label="Delete meeting"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </div>
          )}
        </div>

        {hasTranscriptMatch && (
          <div className="mt-2 rounded-md border border-warning-border/30 bg-warning p-2 text-xs leading-5 text-warning-foreground">
            <span className="font-medium text-warning-foreground">Match:</span>{" "}
            {matchingResult.matchContext}
          </div>
        )}
      </div>
    );
  };

  const onSettings = pathname === "/settings";
  const isEffectivelyCollapsed = isCollapsed || onSettings;

  const goToMeetings = () => {
    router.push("/meetings");
  };
  const openSettingsTab = (tab: string) => {
    setSettingsTab(tab); // optimistic so the highlight swaps on query-only nav
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
  const idleRecordingButtonClass =
    "border border-primary/40 bg-gradient-to-br from-primary to-primary/70 text-primary-foreground shadow-[0_0_28px_hsl(var(--primary)/0.28)] hover:border-primary/50 hover:shadow-[0_0_36px_hsl(var(--primary)/0.42)]";

  return (
    <div className="fixed left-0 top-[var(--titlebar-height)] z-40 h-[calc(100vh-var(--titlebar-height))]">
      {!onSettings && (
        <Button
          variant="ghost"
          onClick={toggleCollapse}
          className="whitespace-normal gap-0 p-0 [&_svg]:size-5 absolute -right-3 top-24 z-50 flex h-7 w-7 items-center justify-center rounded-full border border-sidebar-border bg-sidebar text-muted-foreground shadow-sm transition hover:bg-sidebar-hover hover:text-sidebar-foreground"
          aria-label={isCollapsed ? "Expand sidebar" : "Collapse sidebar"}
        >
          {isCollapsed ? (
            <ChevronRightCircle className="h-5 w-5" />
          ) : (
            <ChevronLeftCircle className="h-5 w-5" />
          )}
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
              <Logo isCollapsed={isCollapsed} />

              <div className="relative">
                <InputGroup className="rounded-md border-sidebar-border bg-background/60 text-sidebar-foreground shadow-none">
                  <InputGroupInput
                    id="meeting-search"
                    placeholder="Search meetings..."
                    value={searchQuery}
                    onChange={(e) => handleSearchChange(e.target.value)}
                    className="placeholder:text-muted-foreground"
                  />
                  <InputGroupAddon>
                    <SearchIcon className="h-4 w-4 text-muted-foreground" />
                  </InputGroupAddon>
                  {searchQuery && (
                    <InputGroupAddon align="inline-end">
                      <InputGroupButton onClick={() => handleSearchChange("")}>
                        <X className="h-4 w-4" />
                      </InputGroupButton>
                    </InputGroupAddon>
                  )}
                </InputGroup>
              </div>
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

            <div className="mt-6 flex min-h-0 flex-1 flex-col px-3">
              <div className="mb-2 flex items-center justify-between px-1">
                <div className="text-[11px] font-semibold tracking-[0.16em] text-muted-foreground">
                  {showAllMeetings ? "All meetings" : "Recent meetings"}
                </div>
                {isSearching && (
                  <span className="text-[11px] text-primary">Searching…</span>
                )}
              </div>

              <div className="flex-1 space-y-1.5 overflow-y-auto pr-1 custom-scrollbar">
                {filteredSidebarItems
                  .filter((item) => item.type === "folder" && item.children)
                  .flatMap((item) => item.children ?? [])
                  .slice(0, searchQuery || showAllMeetings ? undefined : 8)
                  .map((child) => renderItem(child, 1))}

                {filteredSidebarItems.every(
                  (item) => !item.children?.length,
                ) && (
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
                  {showAllMeetings ? "Show recent" : "View all meetings"}
                  <ArrowRight className="h-4 w-4" />
                </Button>
              )}
            </div>

            <div className="flex-shrink-0 space-y-2.5 border-t border-sidebar-border bg-sidebar p-3">
              <Button
                variant="ghost"
                onClick={handleRecordingToggle}
                title={isRecording ? "Click to stop recording" : undefined}
                className={`whitespace-normal h-auto hover:bg-transparent hover:text-primary-foreground flex w-full items-center justify-center gap-2 rounded-lg px-3 py-3 text-sm font-semibold transition ${
                  isRecording
                    ? "bg-destructive text-destructive-foreground hover:bg-destructive/90 hover:text-destructive-foreground"
                    : idleRecordingButtonClass
                }`}
              >
                {isRecording ? (
                  <Square className="h-4 w-4" />
                ) : (
                  <Mic className="h-4 w-4" />
                )}
                <span>
                  {isRecording
                    ? isPaused
                      ? "Paused — click to stop"
                      : "Recording — click to stop"
                    : "Start recording"}
                </span>
              </Button>

              <div className="grid grid-cols-2 gap-2">
                <Button
                  variant="ghost"
                  onClick={() => openImportDialog()}
                  className="whitespace-normal h-auto hover:text-sidebar-foreground flex min-w-0 items-center justify-center gap-2 rounded-lg border border-sidebar-border bg-transparent px-2.5 py-2.5 text-sm font-medium text-sidebar-foreground transition hover:bg-sidebar-hover"
                >
                  <Upload className="h-4 w-4 shrink-0" />
                  <span className="truncate">Import</span>
                </Button>

                <Button
                  variant="ghost"
                  onClick={() => openSettingsTab("general")}
                  className={`whitespace-normal h-auto flex min-w-0 items-center justify-center gap-2 rounded-lg border px-2.5 py-2.5 text-sm font-medium transition ${
                    onSettings
                      ? "border-primary/20 bg-primary/10 text-primary hover:bg-primary/10 hover:text-primary"
                      : "border-sidebar-border bg-transparent text-sidebar-foreground hover:bg-sidebar-hover hover:text-sidebar-foreground"
                  }`}
                >
                  <Settings className="h-4 w-4 shrink-0" />
                  <span className="truncate">Settings</span>
                </Button>
              </div>

              {isRecording ? (
                isPaused ? (
                  <div className="flex items-center gap-2 rounded-lg border border-warning-border/20 bg-warning px-3 py-2 text-xs text-warning-foreground">
                    <span className="h-2 w-2 rounded-full bg-warning-foreground" />
                    Paused
                  </div>
                ) : (
                  <div className="flex items-center gap-2 rounded-lg border border-error-border/20 bg-error px-3 py-2 text-xs text-error-foreground">
                    <span className="h-2 w-2 animate-pulse rounded-full bg-error-foreground" />
                    Recording
                  </div>
                )
              ) : (
                <div className="flex items-center gap-2 rounded-lg border border-success-border/20 bg-success px-3 py-2 text-xs text-success-foreground">
                  <span className="h-2 w-2 rounded-full bg-success-foreground" />
                  Ready for recording
                </div>
              )}

              <div className="px-1 text-center text-xs text-muted-foreground">
                {appVersion ? `v${appVersion}` : null}
              </div>
            </div>
          </>
        )}
      </aside>

      {/* Confirmation Modal for Delete */}
      <ConfirmationModal
        isOpen={deleteModalState.isOpen}
        text="Delete this meeting and its transcript, notes, export history, and temporary Codex files? This action cannot be undone."
        onConfirm={handleDeleteConfirm}
        onCancel={() => setDeleteModalState({ isOpen: false, itemId: null })}
      >
        <label className="mb-6 flex items-start gap-2 text-sm"><Checkbox checked={deleteFiles} onCheckedChange={checked => setDeleteFiles(checked === true)} className="mt-1 h-[13px] w-[13px]" />Also delete the recording files (audio, transcript copy, metadata, and generated documents).</label>
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
