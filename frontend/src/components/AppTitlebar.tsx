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
      className="fixed inset-x-0 top-0 z-[100] flex h-[var(--titlebar-height)] select-none items-center border-b border-border bg-card text-foreground"
      onMouseDown={handleDrag}
      data-tauri-drag-region
    >
      <div className="flex min-w-0 flex-1 items-center gap-2.5 px-3" data-tauri-drag-region>
        <div className="flex h-7 w-7 items-center justify-center rounded-md border border-primary/25 bg-background shadow-sm">
          <Image
            src="/brand/clawscribe-icon-64.png"
            alt=""
            width={23}
            height={23}
            className="pointer-events-none rounded-[4px]"
            priority
          />
        </div>
        <span className="flex min-w-0 items-center gap-2" data-tauri-drag-region>
          <span className="truncate text-[12px] font-semibold tracking-tight text-foreground" data-tauri-drag-region>
            ClawScribe
          </span>
          <span className="h-3 w-px bg-border" aria-hidden="true" data-tauri-drag-region />
          <span
            className="hidden font-mono text-[10px] font-semibold uppercase tracking-[0.18em] text-primary sm:inline"
            data-tauri-drag-region
          >
            Meeting AI
          </span>
        </span>
      </div>

      <div
        className="no-drag flex h-full items-stretch border-l border-border"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button
          type="button"
          className="flex h-full w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          onClick={handleMinimize}
          aria-label="Minimize"
          title="Minimize"
        >
          <Minus className="h-4 w-4" />
        </button>
        <button
          type="button"
          className="flex h-full w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          onClick={handleMaximize}
          aria-label="Maximize or restore"
          title="Maximize or restore"
        >
          <Square className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          className="flex h-full w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-destructive hover:text-destructive-foreground"
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
