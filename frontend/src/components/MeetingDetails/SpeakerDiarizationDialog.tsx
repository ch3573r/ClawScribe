"use client";

import {
  AlertCircle,
  CheckCircle2,
  Clock,
  Cpu,
  Gauge,
  Hash,
  Loader2,
  Route,
  Users,
  Zap,
} from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';

export interface SpeakerDiarizationProgress {
  meeting_id: string;
  stage: string;
  progress_percentage: number;
  message: string;
}

export interface SpeakerDiarizationComplete {
  meeting_id: string;
  speaker_count: number;
  updated_segments: number;
  duration_seconds: number;
  processing_seconds: number;
  provider: string;
  embedding_model: string;
  turn_count: number;
}

interface SpeakerDiarizationDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  isProcessing: boolean;
  progress: SpeakerDiarizationProgress | null;
  result: SpeakerDiarizationComplete | null;
  error: string | null;
  speakerMode: string | null;
  onClearError: () => void;
}

function formatDuration(seconds: number): string {
  const safeSeconds = Number.isFinite(seconds) && seconds > 0 ? seconds : 0;
  const hours = Math.floor(safeSeconds / 3600);
  const minutes = Math.floor((safeSeconds % 3600) / 60);
  const secs = Math.floor(safeSeconds % 60);

  if (hours > 0) {
    return `${hours}:${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  }
  return `${minutes}:${secs.toString().padStart(2, '0')}`;
}

function formatProvider(provider: string): string {
  const normalized = provider.trim().toLowerCase();
  if (normalized === 'directml') return 'DirectML';
  if (normalized === 'cpu') return 'CPU';
  if (!normalized) return 'Unknown provider';
  return provider;
}

export function SpeakerDiarizationDialog({
  open,
  onOpenChange,
  isProcessing,
  progress,
  result,
  error,
  speakerMode,
  onClearError,
}: SpeakerDiarizationDialogProps) {
  const speed = result && result.processing_seconds > 0
    ? result.duration_seconds / result.processing_seconds
    : null;

  const handleOpenChange = (newOpen: boolean) => {
    if (!newOpen && isProcessing) {
      return;
    }
    onOpenChange(newOpen);
  };

  const handleEscapeKeyDown = (event: KeyboardEvent) => {
    if (isProcessing) {
      event.preventDefault();
    }
  };

  const handleInteractOutside = (event: Event) => {
    if (isProcessing) {
      event.preventDefault();
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="sm:max-w-[500px]"
        onEscapeKeyDown={handleEscapeKeyDown}
        onInteractOutside={handleInteractOutside}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {isProcessing ? (
              <>
                <Loader2 className="h-5 w-5 animate-spin text-primary" />
                Detecting Speakers...
              </>
            ) : error ? (
              <>
                <AlertCircle className="h-5 w-5 text-destructive" />
                Speaker Diarization Failed
              </>
            ) : result ? (
              <>
                <CheckCircle2 className="h-5 w-5 text-emerald-500" />
                Speaker Diarization Complete
              </>
            ) : (
              <>
                <Users className="h-5 w-5 text-primary" />
                Detect Speakers
              </>
            )}
          </DialogTitle>
          <DialogDescription>
            {isProcessing
              ? progress?.message || `Running ${speakerMode?.toLowerCase() || 'speaker detection'}...`
              : error
              ? 'An error occurred during speaker diarization'
              : result
              ? 'Speaker labels were applied. Diarization benchmark below.'
              : 'Detect speaker turns from the saved meeting audio'}
          </DialogDescription>
        </DialogHeader>

        <div className="min-w-0 space-y-4 py-4">
          {isProcessing && (
            <div className="space-y-2">
              <div className="relative">
                <div className="h-3 w-full rounded-full bg-muted">
                  <div
                    className="h-3 rounded-full bg-primary transition-all duration-300 ease-out"
                    style={{ width: `${Math.min(progress?.progress_percentage ?? 0, 100)}%` }}
                  />
                </div>
                <div className="mt-1 flex justify-between text-xs text-muted-foreground">
                  <span>{progress?.stage || 'starting'}</span>
                  <span>{Math.round(progress?.progress_percentage ?? 0)}%</span>
                </div>
              </div>
              <p className="text-center text-sm text-muted-foreground">
                {progress?.message || `Starting ${speakerMode?.toLowerCase() || 'speaker detection'}...`}
              </p>
            </div>
          )}

          {error && !isProcessing && (
            <div className="rounded-lg border border-destructive/30 bg-destructive/10 p-3">
              <p className="text-sm text-destructive">{error}</p>
            </div>
          )}

          {result && !isProcessing && !error && (
            <div className="space-y-3">
              <div className="grid grid-cols-2 gap-3">
                <div className="rounded-lg border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Clock className="h-3.5 w-3.5" />
                    Audio length
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {formatDuration(result.duration_seconds)}
                  </p>
                </div>
                <div className="rounded-lg border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Gauge className="h-3.5 w-3.5" />
                    Processing time
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {result.processing_seconds.toFixed(1)}s
                  </p>
                </div>
                <div className="rounded-lg border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Zap className="h-3.5 w-3.5" />
                    Speed
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {speed ? `${speed.toFixed(1)}x realtime` : '-'}
                  </p>
                </div>
                <div className="rounded-lg border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Users className="h-3.5 w-3.5" />
                    Speakers
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">{result.speaker_count}</p>
                </div>
                <div className="rounded-lg border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Route className="h-3.5 w-3.5" />
                    Turns
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">{result.turn_count}</p>
                </div>
                <div className="rounded-lg border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Hash className="h-3.5 w-3.5" />
                    Rows
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">{result.updated_segments}</p>
                </div>
              </div>
              <div className="space-y-1 text-xs text-muted-foreground">
                <div className="flex items-start gap-1.5">
                  <Cpu className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                  <span>Provider: {formatProvider(result.provider)}</span>
                </div>
                <div className="flex items-start gap-1.5">
                  <Users className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                  <span className="break-words">Embedding: {result.embedding_model}</span>
                </div>
              </div>
            </div>
          )}
        </div>

        <DialogFooter>
          {isProcessing && (
            <Button variant="outline" disabled>
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              Processing...
            </Button>
          )}
          {error && !isProcessing && (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                Close
              </Button>
              <Button onClick={onClearError} variant="outline">
                Choose Again
              </Button>
            </>
          )}
          {result && !isProcessing && !error && (
            <Button onClick={() => onOpenChange(false)} className="bg-primary hover:bg-primary/90">
              Close
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
