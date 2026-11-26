"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";
import { MessageSquare } from "lucide-react";
import { Button } from "@/components/ui/button";
import { LiveAssistancePanel } from "@/components/LiveAssistancePanel";

export function LiveAssistanceDock({ sessionId, verificationError, onRetryStatus, children }: {
  sessionId: string | null; verificationError?: string | null; onRetryStatus?: () => void; children: ReactNode;
}) {
  const [panel, setPanel] = useState<{ sessionId: string; open: boolean } | null>(null);
  const opened = sessionId !== null && panel?.sessionId === sessionId;
  const open = opened && panel.open;
  const launcher = useRef<HTMLButtonElement>(null);
  const dock = useRef<HTMLElement>(null);
  useEffect(() => { if (open) dock.current?.focus({ preventScroll: true }); }, [open]);
  return <div className="flex min-h-0 min-w-0 flex-1 flex-col">
    {sessionId && <div className="flex shrink-0 justify-end border-b border-border bg-card px-4 py-2">
      <Button ref={launcher} variant="outline" size="sm" aria-expanded={open} aria-controls={opened ? "live-assistance-panel" : undefined}
        onClick={() => setPanel({ sessionId, open: !open })}>
        <MessageSquare />Live assistance
      </Button>
    </div>}
    {!open && verificationError && <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border bg-card px-4 py-2">
      <p role="alert" className="text-sm text-destructive">{verificationError}</p>
      <Button variant="outline" size="sm" onClick={onRetryStatus}>Retry status</Button>
    </div>}
    <div className="flex min-h-0 flex-1 overflow-hidden">
      {children}
      {opened && <aside ref={dock} id="live-assistance-panel" aria-label="Live assistance" hidden={!open} tabIndex={-1}
        className={`w-[min(23rem,42%)] shrink-0 flex-col border-l border-border bg-card focus-visible:outline-ring ${open ? "flex" : "hidden"}`}
        onKeyDown={event => {
          if (event.key !== "Escape" || event.defaultPrevented || event.nativeEvent.isComposing ||
            !event.currentTarget.contains(event.target as Node)) return;
          event.preventDefault();
          setPanel({ sessionId, open: false });
          launcher.current?.focus({ preventScroll: true });
        }}>
        <LiveAssistancePanel />
      </aside>}
    </div>
  </div>;
}
