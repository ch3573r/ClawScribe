'use client';

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Cpu, Zap, AlertTriangle } from 'lucide-react';

interface AccelerationStatus {
  compiledBackend: string;
  runtimeDetectedGpu: string;
  useGpu: boolean;
  flashAttn: boolean;
  label: string;
  gpuAvailableButUnused: boolean;
  cpuCores: number;
  memoryGb: number;
  performanceTier: string;
}

/**
 * Shows the transcription backend actually compiled into this build (CPU vs
 * Vulkan/CUDA/Metal) plus the GPU detected at runtime. This matters because
 * runtime GPU detection can't help a CPU-only binary — so a machine with a
 * capable GPU running the CPU build needs to know to install the GPU build.
 */
export function WhisperAccelerationStatus() {
  const [status, setStatus] = useState<AccelerationStatus | null>(null);

  useEffect(() => {
    let cancelled = false;
    invoke<AccelerationStatus>('whisper_get_acceleration_status')
      .then((s) => {
        if (!cancelled) setStatus(s);
      })
      .catch(() => {
        /* non-fatal: just don't show the card */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!status) return null;

  const onGpu = status.useGpu;

  return (
    <div className="rounded-lg border border-border bg-muted p-3 text-sm">
      <div className="flex items-center gap-2">
        {onGpu ? (
          <Zap className="h-4 w-4 text-emerald-500" />
        ) : (
          <Cpu className="h-4 w-4 text-muted-foreground" />
        )}
        <span className="font-medium text-foreground">{status.label}</span>
        <span className="ml-auto text-xs text-muted-foreground">
          {status.cpuCores} cores · {status.memoryGb} GB · {status.performanceTier}
        </span>
      </div>

      {status.gpuAvailableButUnused && (
        <div className="mt-2 flex items-start gap-2 rounded-md border border-amber-300/40 bg-amber-50 p-2 text-xs text-amber-800 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-200">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>
            A {status.runtimeDetectedGpu} GPU was detected, but this is the
            CPU-only build, so transcription runs on the CPU. Install the GPU
            build to use it for much faster transcription.
          </span>
        </div>
      )}
    </div>
  );
}
