"use client";

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Cloud, Cpu, Layers, Users } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { SettingsCard, SettingsRow, SettingsTag } from "@/components/ui/settings-layout";
import { Switch } from "@/components/ui/switch";
import {
  getParakeetBackendSettings,
  getParakeetDirectml,
  getParakeetWorker,
  setParakeetDirectml,
  setParakeetWorker,
  type ParakeetBackendSettings,
} from "@/lib/parakeetAccel";
import {
  getRecordingAudioSavingEnabled,
  refreshSourceAttributionAvailability,
  setSourceAttribution,
} from "@/lib/sourceAttribution";
import { setCloudTranscription } from "@/lib/cloudTranscription";
import { useCloudTranscription } from "@/hooks/useCloudTranscription";
import { useSourceAttribution } from "@/hooks/useSourceAttribution";
import { TRANSCRIPTION_OPTIONS_ID } from "@/components/settings/transcriptionAnchors";


const UNAPPLIED_MESSAGE = "Not active yet. It will apply when the current task finishes.";

function Note({ children }: { children: ReactNode }) {
  return <span role="status" className="mt-1 block">{children}</span>;
}

function BetaTitle({ children }: { children: ReactNode }) {
  return <>{children}<SettingsTag>Beta</SettingsTag></>;
}

/** The four beta switches that used to live on their own settings tab. */
export function TranscriptionOptions() {
  // Parakeet DirectML (GPU) and the separate worker process are applied by the
  // backend on the next model load. The switches always show backend truth.
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
        setWorkerError("Could not change this setting. Stop any recording or transcription and try again.");
      }
    } finally {
      await refreshParakeetSettings();
      if (mounted.current) setWorkerPending(false);
    }
  };

  const workerStatus = parakeetReadError ?? (
    parakeetSettings && parakeetSettings.worker !== getParakeetWorker() ? UNAPPLIED_MESSAGE : workerError
  );
  const dmlStatus = parakeetReadError ?? (
    parakeetSettings && parakeetSettings.directml !== getParakeetDirectml() ? UNAPPLIED_MESSAGE : null
  );

  // Source attribution (Me / Participants), default off. It depends on saved audio.
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

  // Cloud transcription needs consent before it turns on. Turning it on adds
  // the cloud engines to the engine list above.
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
    <SettingsCard id={TRANSCRIPTION_OPTIONS_ID} tabIndex={-1} title="Options" className="scroll-mt-6 focus-visible:outline-none">
      <SettingsRow
        icon={<Cpu aria-hidden="true" />}
        htmlFor="parakeet-directml"
        title={<BetaTitle>Run on the GPU with DirectML</BetaTitle>}
        description={<>
          Operations the GPU cannot run fall back to the CPU. Windows only. Applies to the next recording
          {dmlStatus && <Note>{dmlStatus}</Note>}
          {parakeetSettings?.directmlSessionDisabled && (
            <Note>GPU acceleration was turned off for this session after the speech model stopped responding.</Note>
          )}
        </>}
      >
        <Switch
          id="parakeet-directml"
          checked={parakeetSettings?.directml ?? false}
          disabled={!parakeetSettings || dmlPending || workerPending}
          onCheckedChange={onToggleParakeetDml}
        />
      </SettingsRow>

      <SettingsRow
        icon={<Layers aria-hidden="true" />}
        htmlFor="parakeet-worker"
        title={<BetaTitle>Run transcription in a separate process</BetaTitle>}
        description={<>
          Keeps recording if the speech model crashes or stops responding. Applies from the next recording
          {workerStatus && <Note>{workerStatus}</Note>}
        </>}
      >
        <Switch
          id="parakeet-worker"
          checked={parakeetSettings?.worker ?? false}
          disabled={!parakeetSettings || workerPending || dmlPending}
          onCheckedChange={onToggleParakeetWorker}
        />
      </SettingsRow>

      <SettingsRow
        icon={<Users aria-hidden="true" />}
        htmlFor="source-attribution"
        title={<BetaTitle>Label lines as Me or Participants</BetaTitle>}
        description={<>
          Based on which source was louder. Still mislabels often
          {!audioSavingEnabled && <Note>Turn on Save audio recordings in Recording first. This depends on saved audio and stays off without it.</Note>}
        </>}
      >
        <Switch
          id="source-attribution"
          checked={audioSavingEnabled && sourceAttribution}
          onCheckedChange={onToggleSourceAttribution}
          disabled={!audioSavingEnabled}
        />
      </SettingsRow>

      <SettingsRow
        icon={<Cloud aria-hidden="true" />}
        htmlFor="cloud-transcription"
        title={<BetaTitle>Cloud transcription</BetaTitle>}
        description="Uploads meeting audio to a service you choose. Off keeps audio on this PC"
      >
        <Switch
          id="cloud-transcription"
          checked={cloudTranscription}
          disabled={!cloudTranscriptionLoaded}
          onCheckedChange={onToggleCloudTranscription}
        />
      </SettingsRow>

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
            <Button type="button" variant="outline" onClick={() => setCloudConsentOpen(false)}>
              Cancel
            </Button>
            <Button type="button" onClick={confirmCloudTranscription}>
              Enable
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </SettingsCard>
  );
}
