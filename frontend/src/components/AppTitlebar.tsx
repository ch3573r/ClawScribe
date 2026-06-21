"use client";

import { Minus, Square, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";

const getAppWindow = () => {
  if (typeof window === "undefined") return null;
  return getCurrentWindow();
};

export function AppTitlebar() {
  const handleDrag = async (event: React.MouseEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;

    try {
      const appWindow = getAppWindow();
      if (!appWindow) return;

      if (event.detail === 2) {
        await appWindow.toggleMaximize();
      } else {
        await appWindow.startDragging();
      }
    } catch {
      // Browser previews do not have Tauri's native window API.
    }
  };

  const handleMinimize = async () => {
    try {
      const appWindow = getAppWindow();
      if (!appWindow) return;
      await appWindow.minimize();
    } catch {
      // Browser previews do not have Tauri's native window API.
    }
  };

  const handleMaximize = async () => {
    try {
      const appWindow = getAppWindow();
      if (!appWindow) return;
      await appWindow.toggleMaximize();
    } catch {
      // Browser previews do not have Tauri's native window API.
    }
  };

  const handleClose = async () => {
    try {
      const appWindow = getAppWindow();
      if (!appWindow) return;
      await appWindow.close();
    } catch {
      // Browser previews do not have Tauri's native window API.
    }
  };

  return (
    <header
      className="fixed inset-x-0 top-0 z-[100] flex h-[var(--titlebar-height)] select-none items-center border-b border-sidebar-border/80 bg-sidebar text-foreground"
      onMouseDown={handleDrag}
      data-tauri-drag-region
    >
      <div className="min-w-0 flex-1 self-stretch" data-tauri-drag-region />

      <div
        className="no-drag flex h-full items-stretch border-l border-sidebar-border/80"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button
          type="button"
          className="flex h-full w-10 items-center justify-center text-muted-foreground transition-colors hover:bg-sidebar-hover hover:text-sidebar-foreground"
          onClick={handleMinimize}
          aria-label="Minimize"
          title="Minimize"
        >
          <Minus className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          className="flex h-full w-10 items-center justify-center text-muted-foreground transition-colors hover:bg-sidebar-hover hover:text-sidebar-foreground"
          onClick={handleMaximize}
          aria-label="Maximize or restore"
          title="Maximize or restore"
        >
          <Square className="h-3 w-3" />
        </button>
        <button
          type="button"
          className="flex h-full w-10 items-center justify-center text-muted-foreground transition-colors hover:bg-destructive hover:text-destructive-foreground"
          onClick={handleClose}
          aria-label="Close"
          title="Close"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
    </header>
  );
}
