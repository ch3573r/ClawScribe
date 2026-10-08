import React from 'react';
import { CheckCircle2, Loader2, XCircle } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import type { PermissionRowProps } from '@/types/onboarding';

export function PermissionRow({ icon, title, description, status, isPending = false, onAction }: PermissionRowProps) {
  const isAuthorized = status === 'authorized';
  const isDenied = status === 'denied';
  const isChecking = isPending;

  const getButtonText = () => {
    if (isChecking) return 'Checking...';
    if (isDenied) return 'Open settings';
    return 'Enable';
  };

  return (
    <div
      className={cn(
        'flex items-center justify-between rounded-2xl border px-6 py-5',
        'transition-all duration-200',
        isAuthorized ? 'border-primary bg-muted' : isDenied ? 'border-[hsl(var(--theme-error-fg)/0.3)] bg-[hsl(var(--theme-error-bg))]' : 'bg-card border-border'
      )}
    >
      {/* Left side: Icon + Info */}
      <div className="flex items-center gap-3 flex-1 min-w-0">
        {/* Icon */}
        <div
          className={cn(
            'flex size-10 items-center justify-center rounded-full flex-shrink-0',
            isAuthorized ? 'bg-secondary' : isDenied ? 'bg-[hsl(var(--theme-error-bg))]' : 'bg-muted'
          )}
        >
          <div className={cn(isAuthorized ? 'text-foreground' : isDenied ? 'text-[hsl(var(--theme-error-fg))]' : 'text-muted-foreground')}>{icon}</div>
        </div>

        {/* Title + Description */}
        <div className="min-w-0 flex-1">
          <div className="font-medium truncate text-foreground">{title}</div>
          <div className="text-sm text-muted-foreground">
            {isAuthorized ? (
              <span className="text-[hsl(var(--theme-success-fg))] flex items-center gap-1">
                <CheckCircle2 className="w-3.5 h-3.5" />
                Access granted
              </span>
            ) : isDenied ? (
              <span className="text-[hsl(var(--theme-error-fg))] flex items-center gap-1">
                <XCircle className="w-3.5 h-3.5" />
                Access denied - please grant in System Settings
              </span>
            ) : (
              <span>{description}</span>
            )}
          </div>
        </div>
      </div>

      {/* Right side: Action button or checkmark */}
      <div className="flex items-center gap-2 flex-shrink-0 ml-3">
        {!isAuthorized && (
          <Button
            variant={isDenied ? "destructive" : "outline"}
            size="sm"
            onClick={onAction}
            disabled={isChecking}
            className="min-w-[100px]"
          >
            {isChecking && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            {getButtonText()}
          </Button>
        )}
        {isAuthorized && (
          <div className="flex size-8 items-center justify-center rounded-full bg-[hsl(var(--theme-success-bg))]">
            <CheckCircle2 className="w-4 h-4 text-[hsl(var(--theme-success-fg))]" />
          </div>
        )}
      </div>
    </div>
  );
}
