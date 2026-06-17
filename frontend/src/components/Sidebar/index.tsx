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
  Plug,
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
import { useConfig } from "@/contexts/ConfigContext";

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
  const { betaFeatures } = useConfig();
  const [searchQuery, setSearchQuery] = useState<string>("");
  const [showAllMeetings, setShowAllMeetings] = useState(false);

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
    if (!isCollapsed) return null;

    const isHomePage = pathname === "/";
    const isMeetingPage = pathname?.includes("/meeting-details");
    const isSettingsPage = pathname === "/settings";

    return (
      <TooltipProvider>
        <div className="flex flex-col items-center gap-4 pt-4">
          <Logo isCollapsed={isCollapsed} />

          {[
            {
              label: "Home",
              icon: Home,
              active: isHomePage,
              onClick: () => router.push("/"),
            },
            {
              label: "Meeting Notes",
              icon: NotebookPen,
              active: isMeetingPage,
              onClick: goToMeetingNotes,
            },
            {
              label: "Settings",
              icon: Settings,
              active: isSettingsPage,
              onClick: () => router.push("/settings?tab=general"),
            },
          ].map((item) => {
            const Icon = item.icon;
            return (
              <Tooltip key={item.label}>
                <TooltipTrigger asChild>
                  <button
                    onClick={item.onClick}
                    className={`flex h-10 w-10 items-center justify-center rounded-md transition ${
                      item.active
                        ? "bg-primary/10 text-primary ring-1 ring-primary/20"
                        : "text-muted-foreground hover:bg-muted hover:text-foreground"
                    }`}
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

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={handleRecordingToggle}
                className={`flex h-10 w-10 items-center justify-center rounded-md shadow-sm transition ${
                  isRecording
                    ? "bg-red-500 text-white hover:bg-red-600"
                    : "bg-gradient-to-br from-primary to-primary/70 text-primary-foreground shadow-primary/20 hover:scale-105"
                }`}
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

          {betaFeatures.importAndRetranscribe && (
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => openImportDialog()}
                  className="flex h-10 w-10 items-center justify-center rounded-md text-muted-foreground transition hover:bg-muted hover:text-foreground"
                >
                  <Upload className="h-5 w-5" />
                </button>
              </TooltipTrigger>
              <TooltipContent side="right">
                <p>Import Audio</p>
              </TooltipContent>
            </Tooltip>
          )}
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
        className={`group cursor-pointer rounded-md border px-3 py-3 transition ${
          isActive
            ? "border-primary/30 bg-primary/10 text-primary"
            : hasTranscriptMatch
              ? "border-amber-300/20 bg-amber-300/10 text-foreground"
              : "border-transparent bg-muted text-muted-foreground hover:border-border hover:bg-muted"
        }`}
      >
        <div className="flex items-start gap-3">
          <div
            className={`mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg ${
              isActive
                ? "bg-primary/10 text-primary"
                : "bg-muted text-muted-foreground"
            }`}
          >
            <File className="h-4 w-4" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="line-clamp-2 text-sm font-medium leading-5">
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
                className="rounded-md p-1.5 text-muted-foreground hover:bg-muted hover:text-foreground"
                aria-label="Edit meeting title"
              >
                <Pencil className="h-3.5 w-3.5" />
              </button>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  setDeleteModalState({ isOpen: true, itemId: item.id });
                }}
                className="rounded-md p-1.5 text-muted-foreground hover:bg-red-500/10 hover:text-red-300"
                aria-label="Delete meeting"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          )}
        </div>

        {hasTranscriptMatch && (
          <div className="mt-3 rounded-lg border border-amber-300/20 bg-amber-300/10 p-2 text-xs leading-5 text-amber-100/90">
            <span className="font-medium text-amber-200">Match:</span>{" "}
            {matchingResult.matchContext}
          </div>
        )}
      </div>
    );
  };

  const onSettings = pathname === "/settings";
  const onAddons = onSettings && settingsTab === "integrations";

  const goToMeetingNotes = () => {
    // Only open a real, loadable meeting; otherwise go home (the meetings list),
    // never the placeholder "intro-call" (which 404s the details page).
    const id = currentMeeting?.id;
    const isReal = !!id && id !== "intro-call" && meetings?.some((m) => m.id === id);
    router.push(isReal ? `/meeting-details?id=${id}` : "/");
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
      label: "Meeting Notes",
      icon: NotebookPen,
      active: pathname?.includes("/meeting-details"),
      onClick: goToMeetingNotes,
    },
    {
      label: "Settings",
      icon: Settings,
      active: onSettings && !onAddons,
      onClick: () => openSettingsTab("general"),
    },
    {
      label: "Add-ons",
      icon: Plug,
      active: onAddons,
      onClick: () => openSettingsTab("integrations"),
    },
  ];

  return (
    <div className="fixed left-0 top-0 z-40 h-screen">
      <button
        onClick={toggleCollapse}
        className="absolute -right-3 top-24 z-50 flex h-7 w-7 items-center justify-center rounded-full border border-border bg-card text-muted-foreground shadow-sm transition hover:text-foreground"
        aria-label={isCollapsed ? "Expand sidebar" : "Collapse sidebar"}
      >
        {isCollapsed ? (
          <ChevronRightCircle className="h-5 w-5" />
        ) : (
          <ChevronLeftCircle className="h-5 w-5" />
        )}
      </button>

      <aside
        className={`flex h-screen flex-col border-r border-border bg-background text-muted-foreground shadow-sm transition-all duration-300 ${
          isCollapsed ? "w-16" : "w-[17.5rem]"
        }`}
      >
        {isCollapsed ? (
          renderCollapsedIcons()
        ) : (
          <>
            <div className="flex-shrink-0 space-y-5 px-4 pb-4 pt-5">
              <Logo isCollapsed={isCollapsed} />

              <div className="relative">
                <InputGroup className="rounded-md border-border bg-muted text-foreground">
                  <InputGroupInput
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

            <nav className="flex-shrink-0 space-y-1 px-3">
              {navItems.map((item) => {
                const Icon = item.icon;
                return (
                  <button
                    key={item.label}
                    onClick={item.onClick}
                    className={`flex w-full items-center gap-3 rounded-md px-3 py-2.5 text-sm font-medium transition ${
                      item.active
                        ? "bg-primary/10 text-primary shadow-[inset_3px_0_0_hsl(var(--primary))]"
                        : "text-muted-foreground hover:bg-muted hover:text-foreground"
                    }`}
                  >
                    <Icon className="h-4 w-4" />
                    <span>{item.label}</span>
                  </button>
                );
              })}
            </nav>

            <div className="mt-6 flex min-h-0 flex-1 flex-col px-3">
              <div className="mb-3 flex items-center justify-between px-1">
                <div className="text-[11px] font-semibold uppercase tracking-[0.22em] text-muted-foreground">
                  Recent Meetings
                </div>
                {isSearching && (
                  <span className="text-[11px] text-primary">Searching…</span>
                )}
              </div>

              <div className="flex-1 space-y-2 overflow-y-auto pr-1 custom-scrollbar">
                {filteredSidebarItems
                  .filter((item) => item.type === "folder" && item.children)
                  .flatMap((item) => item.children ?? [])
                  .slice(0, searchQuery || showAllMeetings ? undefined : 8)
                  .map((child) => renderItem(child, 1))}

                {filteredSidebarItems.every(
                  (item) => !item.children?.length,
                ) && (
                  <div className="rounded-md border border-border bg-muted px-3 py-5 text-center text-sm text-muted-foreground">
                    No meetings found.
                  </div>
                )}
              </div>

              {!searchQuery && meetings.length > 8 && (
                <button
                  onClick={() => setShowAllMeetings((value) => !value)}
                  className="mt-3 flex items-center gap-2 px-1 text-sm font-medium text-primary hover:text-primary"
                >
                  {showAllMeetings ? "Show recent" : "View all meetings"}
                  <ArrowRight className="h-4 w-4" />
                </button>
              )}
            </div>

            <div className="flex-shrink-0 space-y-2 border-t border-border p-3">
              <button
                onClick={handleRecordingToggle}
                title={isRecording ? "Click to stop recording" : undefined}
                className={`flex w-full items-center justify-center gap-2 rounded-md px-3 py-3 text-sm font-semibold shadow-sm transition ${
                  isRecording
                    ? "bg-red-500 text-white hover:bg-red-600"
                    : "bg-gradient-to-r from-primary to-primary/70 text-primary-foreground shadow-primary/20 hover:from-primary hover:to-primary/60"
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

              {betaFeatures.importAndRetranscribe && (
                <button
                  onClick={() => openImportDialog()}
                  className="flex w-full items-center justify-center gap-2 rounded-md border border-border bg-muted px-3 py-2.5 text-sm font-medium text-foreground transition hover:bg-muted"
                >
                  <Upload className="h-4 w-4" />
                  Import Audio
                </button>
              )}

              <button
                onClick={() => router.push("/settings")}
                className="flex w-full items-center justify-center gap-2 rounded-md border border-border bg-muted px-3 py-2.5 text-sm font-medium text-foreground transition hover:bg-muted"
              >
                <Settings className="h-4 w-4" />
                Settings
              </button>

              {isRecording ? (
                isPaused ? (
                  <div className="flex items-center gap-2 rounded-md border border-amber-500/20 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-300">
                    <span className="h-2 w-2 rounded-full bg-amber-500" />
                    Paused
                  </div>
                ) : (
                  <div className="flex items-center gap-2 rounded-md border border-red-500/20 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
                    <span className="h-2 w-2 animate-pulse rounded-full bg-red-500" />
                    Recording
                  </div>
                )
              ) : (
                <div className="flex items-center gap-2 rounded-md border border-emerald-500/20 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-700 dark:text-emerald-300">
                  <span className="h-2 w-2 rounded-full bg-emerald-500" />
                  Ready for recording
                </div>
              )}

              <div className="px-1 text-center text-xs text-muted-foreground">
                v0.5.0-alpha.2
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
