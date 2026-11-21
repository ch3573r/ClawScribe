"use client";

import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { deviceLabel, recordingHealthMetrics, recordingHealthWarnings, type MissingRecordingDevice, type RecordingHealth } from '@/lib/recording-health';
import { recordingService } from '@/services/recordingService';

export function RecordingHealthBanner() {
  const { sessionId, isRecording, isStopping } = useRecordingState();
  const [snapshot, setSnapshot] = useState<{ sessionId: string; health: RecordingHealth | null } | null>(null);
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<{ sessionId: string; message: string } | null>(null);

  useEffect(() => {
    if (!sessionId || (!isRecording && !isStopping)) return;
    const id = sessionId;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const health = await recordingService.getRecordingHealth(id);
        if (!disposed) setSnapshot({ sessionId: id, health });
      } catch {
        // Keep the last valid state visible during a transient IPC failure.
      }
      if (!disposed) timer = setTimeout(refresh, 1000);
    }
    void refresh();
    return () => { disposed = true; clearTimeout(timer); };
  }, [isRecording, isStopping, sessionId]);

  async function retry({ name, device_type }: MissingRecordingDevice) {
    if (!sessionId || retrying) return;
    setRetrying(true);
    setRetryError(null);
    try {
      const result = await recordingService.attemptDeviceReconnect(name, device_type, sessionId);
      if (!result.reconnected) setRetryError({ sessionId, message: 'Audio device still unavailable. Check its connection and try again.' });
    } catch (error) {
      setRetryError({ sessionId, message: typeof error === 'string' ? error : 'Could not reconnect the audio device.' });
    } finally {
      setRetrying(false);
    }
  }

  const health = (isRecording || isStopping) && snapshot && snapshot.sessionId === sessionId ? snapshot.health : null;
  if (!health) return null;
  const warnings = recordingHealthWarnings(health);
  const metrics = recordingHealthMetrics(health);
  const missing = health.disconnected_devices;
  const message = retryError?.sessionId === sessionId ? retryError?.message : null;
  if (!warnings.length && !metrics.length && !missing.length && !message && !health.reconnecting) return null;
  return <div className="shrink-0 space-y-1 border-b border-border bg-card px-4 py-2 text-sm">
    {metrics.length > 0 && <p className="text-xs text-muted-foreground">{metrics.join(' · ')}</p>}
    {warnings.length > 0 && <ul role="alert" className="space-y-1 text-warning-foreground">
      {warnings.map(warning => <li key={warning}>{warning}</li>)}
    </ul>}
    {health.reconnecting && <p role="status">Checking audio device…</p>}
    {!isStopping && missing.map(device => <div key={device.device_type} className="flex items-center gap-2">
      <span>{deviceLabel(device)} unavailable</span>
      <Button type="button" size="sm" variant="outline" disabled={health.reconnecting || retrying}
        onClick={() => void retry(device)}>Retry</Button>
    </div>)}
    {message && missing.length > 0 && <p role="alert" className="text-destructive">{message}</p>}
  </div>;
}
