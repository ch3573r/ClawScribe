import React from 'react';
import { Button } from '@/components/ui/button';

interface ConfirmationModalProps {
  onConfirm: () => void;
  onCancel: () => void;
  text: string;
  isOpen: boolean;
  children?: React.ReactNode;
}

export function ConfirmationModal({ onConfirm, onCancel, text, isOpen, children }: ConfirmationModalProps) {
  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-overlay/50 flex items-center justify-center z-50">
      <div className="bg-card rounded-lg p-6 max-w-md w-full mx-4">
        <h2 className="text-xl font-semibold mb-4">Confirm delete</h2>
        <p className="text-muted-foreground mb-6">{text}</p>
        {children}
        <div className="flex justify-end space-x-4">
          <Button variant="ghost"
            onClick={onCancel}
            className="px-4 py-2 text-muted-foreground hover:bg-muted rounded-md transition-colors h-auto text-base font-normal hover:text-muted-foreground"
          >
            Cancel
          </Button>
          <Button variant="ghost"
            onClick={onConfirm}
            className="px-4 py-2 bg-destructive text-destructive-foreground hover:bg-destructive/90 rounded-md transition-colors h-auto text-base font-normal hover:text-destructive-foreground"
          >
            Delete
          </Button>
        </div>
      </div>
    </div>
  );
}
