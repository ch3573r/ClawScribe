"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Switch } from "./ui/switch";
import { AlertCircle, Cloud, Cpu, Layers, Users } from "lucide-react";
import { Button } from "./ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import {
  getParakeetDirectml,
  getParakeetBackendSettings,
  type ParakeetBackendSettings,
  setParakeetDirectml,
  getParakeetWorker,
  setParakeetWorker,
} from "@/lib/parakeetAccel";
import {
  getRecordingAudioSavingEnabled,
  refreshSourceAttributionAvailability,
  setSourceAttribution,
} from "@/lib/sourceAttribution";
import { setCloudTranscription } from "@/lib/cloudTranscription";
import { useCloudTranscription } from "@/hooks/useCloudTranscription";
import { useSourceAttribution } from "@/hooks/useSourceAttribution";

export function BetaSettings() {
  // Parakeet DirectML (GPU) — experimental, opt-in. Applied on the next model
  // load (the backend unloads the model when toggled).
  const [parakeetSettings, setParakeetSettings] = useState<ParakeetBackendSettings | null>(null);
  const [parakeetReadError, setParakeetReadError] = useState<string | null>(null);
  const [dmlPending, setDmlPending] = useState(false);
  const [workerPending, setWorkerPending] = useState(false);
  const [workerError, setWorkerError] = useState<string | null>(null);
  const mounted = useRef(false);
  const refreshSequence = useRef(0);
  const refreshParakeetSettings = useCallback(async () => {
    const sequence = ++refreshSequence.current;
    try {
      const settings = await getParakeetBackendSettings();
      if (!mounted.current || sequence !== refreshSequence.current) return;
      setParakeetSettings(settings);
      setParakeetReadError(null);
    } catch {
      if (!mounted.current || sequence !== refreshSequence.current) return;
      setParakeetSettings(null);
      setParakeetReadError("Could not read speech settings. Retrying shortly.");
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refreshParakeetSettings();
    // Observe startup retries and session GPU downgrades while this card is
    // open. These are backend snapshots, never locally inferred toggle state.
    const timer = setInterval(() => void refreshParakeetSettings(), 2_000);
    return () => {
      mounted.current = false;
      refreshSequence.current += 1;
      clearInterval(timer);
    };
  }, [refreshParakeetSettings]);

  const onToggleParakeetDml = async (checked: boolean) => {
    setDmlPending(true);
    try {
      await setParakeetDirectml(checked);
    } finally {
      await refreshParakeetSettings();
      if (mounted.current) setDmlPending(false);
    }
  };

  const onToggleParakeetWorker = async (checked: boolean) => {
    setWorkerPending(true);
    setWorkerError(null);
    try {
      await setParakeetWorker(checked);
    } catch {
      if (mounted.current) {
        setWorkerError(
          "Could not change this setting. Stop any recording or transcription and try again."
        );
      }
    } finally {
      await refreshParakeetSettings();
      if (mounted.current) setWorkerPending(false);
    }
  };

  const unappliedMessage = "Not active yet. It will apply when the current task finishes.";
  const workerStatus = parakeetReadError ?? (
    parakeetSettings && parakeetSettings.worker !== getParakeetWorker()
      ? unappliedMessage
      : workerError
  );
  const dmlStatus = parakeetReadError ?? (
    parakeetSettings && parakeetSettings.directml !== getParakeetDirectml()
      ? unappliedMessage
      : null
  );

  // Source attribution (Me/Participants) — experimental, opt-in (default off).
  const sourceAttribution = useSourceAttribution();
  const [audioSavingEnabled, setAudioSavingEnabled] = useState(true);

  useEffect(() => {
    let cancelled = false;

    const syncAvailability = async () => {
      const enabled = await getRecordingAudioSavingEnabled();
      if (cancelled) return;
      setAudioSavingEnabled(enabled);
      if (!enabled) {
        await refreshSourceAttributionAvailability();
      }
    };

    void syncAvailability();

    return () => {
      cancelled = true;
    };
  }, []);

  const onToggleSourceAttribution = async (checked: boolean) => {
    const enabled = await setSourceAttribution(checked);
    setAudioSavingEnabled(await getRecordingAudioSavingEnabled());
    if (checked && !enabled) {
      setAudioSavingEnabled(false);
    }
  };

  const { enabled: cloudTranscription, loaded: cloudTranscriptionLoaded } = useCloudTranscription();
  const [cloudConsentOpen, setCloudConsentOpen] = useState(false);
  const onToggleCloudTranscription = (checked: boolean) => {
    if (checked) {
      setCloudConsentOpen(true);
      return;
    }
    void setCloudTranscription(false);
  };
  const confirmCloudTranscription = () => {
    setCloudConsentOpen(false);
    void setCloudTranscription(true);
  };

  return (
    <div className="space-y-6">
      <div className="flex items-start gap-3 p-4 bg-warning border border-warning-border/30 rounded-md">
        <AlertCircle className="h-5 w-5 text-warning-foreground flex-shrink-0 mt-0.5" />
        <div className="text-sm text-warning-foreground">
          <p className="font-medium">Experimental settings</p>
          <p className="mt-1">
            These settings are still being tested. You may encounter issues, and
            changes may take effect on the next recording.
          </p>
        </div>
      </div>

      {/* Parakeet GPU acceleration (DirectML) — experimental, opt-in */}
      <div className="rounded-lg border border-border bg-card p-6 shadow-sm">
        <div className="flex items-center justify-between">
          <div className="flex-1">
            <div className="mb-2 flex items-center gap-2">
              <Cpu className="h-5 w-5 text-muted-foreground" />
              <h3 id="parakeet-directml-title" className="text-lg font-semibold text-foreground">
                GPU transcription (Parakeet · DirectML)
              </h3>
              <span className="rounded-full bg-warning px-2 py-0.5 text-xs font-medium text-warning-foreground">
                Beta
              </span>
            </div>
            <p id="parakeet-directml-help" className="text-sm text-muted-foreground">
              Run Parakeet through DirectML on your GPU instead of the CPU. Uses
              the current int8 model, so unsupported ops fall back to CPU —
              useful to benchmark whether your GPU helps. Takes effect on the
              next recording. Windows only.
            </p>
            {dmlStatus && (
              <p role="status" className="mt-2 text-sm text-muted-foreground">
                {dmlStatus}
              </p>
            )}
            {parakeetSettings?.directmlSessionDisabled && (
              <p role="status" className="mt-2 text-sm text-muted-foreground">
                GPU acceleration was turned off for this session after the speech model stopped responding.
              </p>
            )}
          </div>
          <div className="ml-6">
            <Switch
              aria-labelledby="parakeet-directml-title"
              aria-describedby="parakeet-directml-help"
              checked={parakeetSettings?.directml ?? false}
              disabled={!parakeetSettings || dmlPending || workerPending}
              onCheckedChange={onToggleParakeetDml}
            />
          </div>
        </div>
      </div>

      <div className="rounded-lg border border-border bg-card p-6 shadow-sm">
        <div className="flex items-center justify-between">
          <div className="flex-1">
            <div className="mb-2 flex items-center gap-2">
              <Layers className="h-5 w-5 text-muted-foreground" />
              <h3 id="parakeet-worker-title" className="text-lg font-semibold text-foreground">
                Run transcription in a separate process
              </h3>
              <span className="rounded-full bg-warning px-2 py-0.5 text-xs font-medium text-warning-foreground">
                Beta
              </span>
            </div>
            <p id="parakeet-worker-help" className="text-sm text-muted-foreground">
              Keeps recording running if the speech model crashes or stops responding. Applies from the next recording.
            </p>
            {workerStatus && (
              <p role="status" className="mt-2 text-sm text-muted-foreground">
                {workerStatus}
              </p>
            )}
          </div>
          <div className="ml-6">
            <Switch
              id="parakeet-worker"
              aria-labelledby="parakeet-worker-title"
              aria-describedby="parakeet-worker-help"
              checked={parakeetSettings?.worker ?? false}
              disabled={!parakeetSettings || workerPending || dmlPending}
              onCheckedChange={onToggleParakeetWorker}
            />
          </div>
        </div>
      </div>

      {/* Source attribution (Me / Participants) — experimental, opt-in */}
      <div className="rounded-lg border border-border bg-card p-6 shadow-sm">
        <div className="flex items-center justify-between">
          <div className="flex-1">
            <div className="mb-2 flex items-center gap-2">
              <Users className="h-5 w-5 text-muted-foreground" />
              <h3 className="text-lg font-semibold text-foreground">
                Source attribution (Me / Participants)
              </h3>
              <span className="rounded-full bg-warning px-2 py-0.5 text-xs font-medium text-warning-foreground">
                Beta
              </span>
            </div>
            <p className="text-sm text-muted-foreground">
              Label each transcript line as &quot;Me&quot; (your microphone) or
              &quot;Participants&quot; (system audio) based on which source was
              louder. The heuristic still mislabels often, so it&apos;s off by
              default — when disabled, lines carry no speaker label. Takes effect
              on the next transcribed segment.
            </p>
            {!audioSavingEnabled && (
              <p className="mt-3 rounded-md border border-border bg-muted px-3 py-2 text-xs text-muted-foreground">
                Enable Recording &gt; Save audio recordings first. Source
                attribution depends on the saved audio workflow and stays off
                while audio saving is disabled.
              </p>
            )}
          </div>
          <div className="ml-6">
            <Switch
              checked={audioSavingEnabled && sourceAttribution}
              onCheckedChange={onToggleSourceAttribution}
              disabled={!audioSavingEnabled}
            />
          </div>
        </div>
      </div>

      {/* Cloud transcription - experimental, opt-in with consent */}
      <div className="rounded-lg border border-border bg-card p-6 shadow-sm">
        <div className="flex items-center justify-between">
          <div className="flex-1">
            <div className="mb-2 flex items-center gap-2">
              <Cloud className="h-5 w-5 text-muted-foreground" />
              <h3 className="text-lg font-semibold text-foreground">
                Cloud transcription
              </h3>
              <span className="rounded-full bg-warning px-2 py-0.5 text-xs font-medium text-warning-foreground">
                Beta
              </span>
            </div>
            <p className="text-sm text-muted-foreground">
              Cloud transcription uploads your meeting audio to the selected
              third-party service for processing. Off by default; local
              transcription keeps audio on-device.
            </p>
          </div>
          <div className="ml-6">
            <Switch
              checked={cloudTranscription}
              disabled={!cloudTranscriptionLoaded}
              onCheckedChange={onToggleCloudTranscription}
            />
          </div>
        </div>
      </div>

      <Dialog open={cloudConsentOpen} onOpenChange={setCloudConsentOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Enable cloud transcription?</DialogTitle>
            <DialogDescription>
              Cloud transcription uploads your meeting audio to the selected
              third-party service for processing. Off by default; local
              transcription keeps audio on-device.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => setCloudConsentOpen(false)}
            >
              Cancel
            </Button>
            <Button type="button" onClick={confirmCloudTranscription}>
              Enable
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
