import React, { useEffect, useRef, useState } from 'react';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';

interface ConfirmationModalProps {
  onConfirm: () => void | Promise<void>;
  onCancel: () => void;
  text: string;
  isOpen: boolean;
  title?: string;
  confirmLabel?: string;
}

/** Shared destructive-action dialog: safe initial focus, Escape, focus trap,
 * and explicit focus restoration even when there is no Radix Trigger element.
 */
export function ConfirmationModal({ onConfirm, onCancel, text, isOpen, title = 'Confirm delete', confirmLabel = 'Delete' }: ConfirmationModalProps) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  const previousFocus = useRef<HTMLElement | null>(null);
  const pending = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { if (isOpen) setError(null); }, [isOpen]);

  const confirm = async () => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setError(null);
    try { await onConfirm(); }
    catch { setError('The action could not be completed. Please try again.'); }
    finally { pending.current = false; setBusy(false); }
  };

  return (
    <Dialog open={isOpen} onOpenChange={open => { if (!open && !pending.current) onCancel(); }}>
      <DialogContent
        onOpenAutoFocus={event => {
          event.preventDefault();
          previousFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
          cancelRef.current?.focus();
        }}
        onCloseAutoFocus={event => {
          event.preventDefault();
          const target = previousFocus.current;
          if (target?.isConnected) target.focus();
          else document.querySelector<HTMLElement>('main, [role="main"]')?.focus();
        }}
        onEscapeKeyDown={event => { if (pending.current) event.preventDefault(); }}
        onInteractOutside={event => event.preventDefault()}
        className="max-h-[calc(100dvh-2rem)] w-[calc(100vw-2rem)] max-w-md overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{text}</DialogDescription>
        </DialogHeader>
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        <DialogFooter className="gap-2">
          <button ref={cancelRef} type="button" disabled={busy} onClick={onCancel}
            className="min-h-10 rounded-md border border-border px-4 py-2 text-foreground hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50">Cancel</button>
          <button type="button" disabled={busy} onClick={() => void confirm()}
            className="min-h-10 rounded-md bg-destructive px-4 py-2 text-destructive-foreground hover:bg-destructive/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50">
            {busy ? 'Working…' : confirmLabel}
          </button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
