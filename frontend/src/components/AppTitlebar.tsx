"use client";

import Image from "next/image";
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
      className="fixed inset-x-0 top-0 z-[100] flex h-[var(--titlebar-height)] select-none items-center border-b border-sidebar-border bg-sidebar text-sidebar-foreground shadow-sm"
      onMouseDown={handleDrag}
      data-tauri-drag-region
    >
      <div
        className="pointer-events-none absolute inset-x-0 bottom-0 h-px bg-gradient-to-r from-transparent via-primary/70 to-transparent"
        aria-hidden="true"
      />

      <div className="flex min-w-0 flex-1 items-center gap-2.5 px-3" data-tauri-drag-region>
        <div className="flex h-7 w-7 items-center justify-center rounded-md border border-primary/20 bg-brand-gradient shadow-sm">
          <Image
            src="/brand/clawscribe-icon-64.png"
            alt=""
            width={23}
            height={23}
            className="pointer-events-none rounded-[4px]"
            priority
          />
        </div>
        <span className="flex min-w-0 items-baseline gap-2" data-tauri-drag-region>
          <span className="truncate text-[13px] font-semibold tracking-tight text-sidebar-foreground" data-tauri-drag-region>
            ClawScribe
          </span>
          <span
            className="hidden font-mono text-[10px] font-semibold uppercase tracking-[0.18em] text-primary/75 sm:inline"
            data-tauri-drag-region
          >
            Meeting AI
          </span>
        </span>
      </div>

      <div
        className="no-drag mr-2 flex items-center gap-1 rounded-md border border-sidebar-border bg-background/55 p-1 shadow-inner"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button
          type="button"
          className="flex h-7 w-8 items-center justify-center rounded-[4px] text-muted-foreground transition hover:bg-sidebar-hover hover:text-sidebar-foreground"
          onClick={handleMinimize}
          aria-label="Minimize"
          title="Minimize"
        >
          <Minus className="h-4 w-4" />
        </button>
        <button
          type="button"
          className="flex h-7 w-8 items-center justify-center rounded-[4px] text-muted-foreground transition hover:bg-sidebar-hover hover:text-sidebar-foreground"
          onClick={handleMaximize}
          aria-label="Maximize or restore"
          title="Maximize or restore"
        >
          <Square className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          className="flex h-7 w-8 items-center justify-center rounded-[4px] text-muted-foreground transition hover:bg-destructive hover:text-destructive-foreground"
          onClick={handleClose}
          aria-label="Close"
          title="Close"
        >
          <X className="h-4 w-4" />
        </button>
      </div>
    </header>
  );
}
