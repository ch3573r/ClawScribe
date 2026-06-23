"use client";

import React, { useState, useMemo, useEffect, useCallback } from "react";
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
import Analytics from "@/lib/analytics";
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
    toggleCollapse,
    handleRecordingToggle,
    searchTranscripts,
    searchResults,
    isSearching,
    meetings,
    setMeetings,
  } = useSidebar();

  // Get recording state from RecordingStateContext (single source of truth)
  const { isRecording, isPaused } = useRecordingState();
  const { openImportDialog } = useImportDialog();
  const [searchQuery, setSearchQuery] = useState<string>("");
  const [showAllMeetings, setShowAllMeetings] = useState(false);
  const appVersion = process.env.NEXT_PUBLIC_APP_VERSION ?? "";

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
    console.log("Deleting item:", itemId);
    const payload = {
      meetingId: itemId,
    };

    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("api_delete_meeting", {
        meetingId: itemId,
      });
      console.log("Meeting deleted successfully");
      const updatedMeetings = meetings.filter(
        (m: CurrentMeeting) => m.id !== itemId,
      );
      setMeetings(updatedMeetings);

      // Track meeting deletion
      Analytics.trackMeetingDeleted(itemId);

      // Show success toast
      toast.success("Meeting deleted successfully", {
        description: "All associated data has been removed",
      });

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
      {
        label: "Settings",
        icon: Settings,
        active: isSettingsPage,
        onClick: () => openSettingsTab("general"),
      },
    ];
    const statusDot = isRecording
      ? isPaused
        ? "bg-amber-500"
        : "bg-red-500 animate-pulse"
      : "bg-emerald-500";
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
                    <button
                      onClick={item.onClick}
                      className={`flex h-10 w-10 items-center justify-center rounded-md transition ${
                        item.active
                          ? "bg-primary/10 text-primary ring-1 ring-primary/20"
                          : "text-muted-foreground hover:bg-sidebar-hover hover:text-sidebar-foreground"
                      }`}
                      aria-label={item.label}
                    >
                      <Icon className="h-5 w-5" />
                    </button>
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
                <button
                  onClick={handleRecordingToggle}
                  className={`flex h-11 w-11 items-center justify-center rounded-lg shadow-sm transition ${
                    isRecording
                      ? "bg-red-500 text-white hover:bg-red-600"
                      : "bg-primary text-primary-foreground shadow-primary/20 hover:bg-primary/90"
                  }`}
                  aria-label={isRecording ? "Stop recording" : "Start recording"}
                >
                  {isRecording ? (
                    <Square className="h-5 w-5" />
                  ) : (
                    <Mic className="h-5 w-5" />
                  )}
                </button>
              </TooltipTrigger>
              <TooltipContent side="right">
                <p>
                  {isRecording
                    ? isPaused
                      ? "Paused — click to stop"
                      : "Recording — click to stop"
                    : "Start Recording"}
                </p>
              </TooltipContent>
            </Tooltip>

            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => openImportDialog()}
                  className="flex h-10 w-10 items-center justify-center rounded-md text-muted-foreground transition hover:bg-sidebar-hover hover:text-sidebar-foreground"
                  aria-label="Import audio"
                >
                  <Upload className="h-5 w-5" />
                </button>
              </TooltipTrigger>
              <TooltipContent side="right">
                <p>Import Audio</p>
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
            : item.id.includes("-")
              ? `/meeting-details?id=${item.id}`
              : `/notes/${item.id}`;
          router.push(basePath);
        }}
        className={`group cursor-pointer rounded-md border px-2.5 py-2.5 transition ${
          isActive
            ? "border-primary/25 bg-primary/10 text-sidebar-foreground shadow-sm"
            : hasTranscriptMatch
              ? "border-amber-400/30 bg-amber-400/10 text-sidebar-foreground"
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
            <div className="mt-1 text-xs text-muted-foreground">Recent meeting</div>
          </div>
          {isMeetingItem && (
            <div className="flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  handleEditStart(item.id, item.title);
                }}
                className="rounded-md p-1.5 text-muted-foreground hover:bg-sidebar-hover hover:text-sidebar-foreground"
                aria-label="Edit meeting title"
              >
                <Pencil className="h-3.5 w-3.5" />
              </button>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  setDeleteModalState({ isOpen: true, itemId: item.id });
                }}
                className="rounded-md p-1.5 text-muted-foreground hover:bg-red-500/10 hover:text-red-500"
                aria-label="Delete meeting"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          )}
        </div>

        {hasTranscriptMatch && (
          <div className="mt-2 rounded-md border border-amber-400/30 bg-amber-400/10 p-2 text-xs leading-5 text-amber-800 dark:text-amber-100">
            <span className="font-medium text-amber-800 dark:text-amber-200">Match:</span>{" "}
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
    {
      label: "Settings",
      icon: Settings,
      active: onSettings,
      onClick: () => openSettingsTab("general"),
    },
  ];

  return (
    <div className="fixed left-0 top-[var(--titlebar-height)] z-40 h-[calc(100vh-var(--titlebar-height))]">
      {!onSettings && (
        <button
          onClick={toggleCollapse}
          className="absolute -right-3 top-24 z-50 flex h-7 w-7 items-center justify-center rounded-full border border-sidebar-border bg-sidebar text-muted-foreground shadow-sm transition hover:bg-sidebar-hover hover:text-sidebar-foreground"
          aria-label={isCollapsed ? "Expand sidebar" : "Collapse sidebar"}
        >
          {isCollapsed ? (
            <ChevronRightCircle className="h-5 w-5" />
          ) : (
            <ChevronLeftCircle className="h-5 w-5" />
          )}
        </button>
      )}

      <aside
        className={`flex h-full flex-col border-r border-sidebar-border bg-sidebar text-muted-foreground shadow-sm transition-all duration-300 ${
          isEffectivelyCollapsed ? "w-16" : "w-[17.5rem]"
        }`}
      >
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
                  <button
                    key={item.label}
                    onClick={item.onClick}
                    className={`relative flex w-full items-center gap-3 rounded-md px-3 py-2.5 text-sm font-medium transition ${
                      item.active
                        ? "bg-primary/10 text-primary ring-1 ring-primary/15"
                        : "text-muted-foreground hover:bg-sidebar-hover hover:text-sidebar-foreground"
                    }`}
                  >
                    {item.active && (
                      <span className="absolute bottom-2 left-0 top-2 w-0.5 rounded-full bg-primary" />
                    )}
                    <Icon className="h-4 w-4 shrink-0" />
                    <span>{item.label}</span>
                  </button>
                );
              })}
            </nav>

            <div className="mt-6 flex min-h-0 flex-1 flex-col px-3">
              <div className="mb-2 flex items-center justify-between px-1">
                <div className="text-[11px] font-semibold uppercase tracking-[0.16em] text-muted-foreground">
                  Recent Meetings
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
                <button
                  onClick={() => setShowAllMeetings((value) => !value)}
                  className="mt-3 flex items-center gap-2 px-1 text-sm font-medium text-primary hover:text-primary/80"
                >
                  {showAllMeetings ? "Show recent" : "View all meetings"}
                  <ArrowRight className="h-4 w-4" />
                </button>
              )}
            </div>

            <div className="flex-shrink-0 space-y-2.5 border-t border-sidebar-border bg-sidebar p-3">
              <button
                onClick={handleRecordingToggle}
                title={isRecording ? "Click to stop recording" : undefined}
                className={`flex w-full items-center justify-center gap-2 rounded-lg px-3 py-3 text-sm font-semibold shadow-sm transition ${
                  isRecording
                    ? "bg-red-500 text-white hover:bg-red-600"
                    : "bg-primary text-primary-foreground shadow-primary/20 hover:bg-primary/90"
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
                    : "Start Recording"}
                </span>
              </button>

              <div className="grid grid-cols-2 gap-2">
                <button
                  onClick={() => openImportDialog()}
                  className="flex min-w-0 items-center justify-center gap-2 rounded-lg border border-sidebar-border bg-transparent px-2.5 py-2.5 text-sm font-medium text-sidebar-foreground transition hover:bg-sidebar-hover"
                >
                  <Upload className="h-4 w-4 shrink-0" />
                  <span className="truncate">Import</span>
                </button>

                <button
                  onClick={() => router.push("/settings")}
                  className="flex min-w-0 items-center justify-center gap-2 rounded-lg border border-sidebar-border bg-transparent px-2.5 py-2.5 text-sm font-medium text-sidebar-foreground transition hover:bg-sidebar-hover"
                >
                  <Settings className="h-4 w-4 shrink-0" />
                  <span className="truncate">Settings</span>
                </button>
              </div>

              {isRecording ? (
                isPaused ? (
                  <div className="flex items-center gap-2 rounded-lg border border-amber-500/20 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-300">
                    <span className="h-2 w-2 rounded-full bg-amber-500" />
                    Paused
                  </div>
                ) : (
                  <div className="flex items-center gap-2 rounded-lg border border-red-500/20 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
                    <span className="h-2 w-2 animate-pulse rounded-full bg-red-500" />
                    Recording
                  </div>
                )
              ) : (
                <div className="flex items-center gap-2 rounded-lg border border-emerald-500/20 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-700 dark:text-emerald-300">
                  <span className="h-2 w-2 rounded-full bg-emerald-500" />
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
        text="Are you sure you want to delete this meeting? This action cannot be undone."
        onConfirm={handleDeleteConfirm}
        onCancel={() => setDeleteModalState({ isOpen: false, itemId: null })}
      />

      {/* Edit Meeting Title Modal */}
      <Dialog
        open={editModalState.isOpen}
        onOpenChange={(open) => {
          if (!open) handleEditCancel();
        }}
      >
        <DialogContent className="sm:max-w-[425px]">
          <VisuallyHidden>
            <DialogTitle>Edit Meeting Title</DialogTitle>
          </VisuallyHidden>
          <div className="py-4">
            <h3 className="text-lg font-semibold mb-4">Edit Meeting Title</h3>
            <div className="space-y-4">
              <div>
                <label
                  htmlFor="meeting-title"
                  className="block text-sm font-medium text-foreground mb-2"
                >
                  Meeting Title
                </label>
                <input
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
                  className="w-full px-3 py-2 border border-input bg-background rounded-md focus:outline-none focus:ring-2 focus:ring-ring focus:border-transparent"
                  placeholder="Enter meeting title"
                  autoFocus
                />
              </div>
            </div>
          </div>
          <DialogFooter>
            <button
              onClick={handleEditCancel}
              className="px-4 py-2 text-sm font-medium text-secondary-foreground bg-secondary hover:bg-muted rounded-md transition-colors"
            >
              Cancel
            </button>
            <button
              onClick={handleEditConfirm}
              className="px-4 py-2 text-sm font-medium text-primary-foreground bg-primary hover:bg-primary/90 rounded-md transition-colors"
            >
              Save
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};

export default Sidebar;
