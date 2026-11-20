import React, { useRef } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';

interface ConfirmationModalProps {
  onConfirm: () => void;
  onCancel: () => void;
  text: string;
  title?: string;
  confirmLabel?: string;
  isOpen: boolean;
  /** Disables the confirm action while the surface cannot act on it yet. */
  confirmDisabled?: boolean;
  /** Control that opened this dialog. Needed when it was opened from a menu item that closes with the menu. */
  returnFocus?: React.RefObject<HTMLElement | null>;
  children?: React.ReactNode;
}

export function ConfirmationModal({ onConfirm, onCancel, text, title = 'Confirm delete', confirmLabel = 'Delete', isOpen, confirmDisabled = false, returnFocus, children }: ConfirmationModalProps) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);

  return (
    <Dialog open={isOpen} onOpenChange={open => { if (!open) onCancel(); }}>
      <DialogContent className="max-w-md"
        onOpenAutoFocus={event => {
          event.preventDefault();
          if (typeof document !== 'undefined') returnFocusRef.current = document.activeElement as HTMLElement | null;
          cancelRef.current?.focus();
        }}
        onCloseAutoFocus={event => {
          event.preventDefault();
          const target = returnFocus?.current?.isConnected ? returnFocus.current : returnFocusRef.current;
          if (target?.isConnected) target.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{text}</DialogDescription>
        </DialogHeader>
        {children}
        <DialogFooter>
          <Button ref={cancelRef} variant="outline"
            onClick={onCancel}
          >
            Cancel
          </Button>
          <Button variant="destructive" disabled={confirmDisabled}
            onClick={onConfirm}
          >
            {confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
