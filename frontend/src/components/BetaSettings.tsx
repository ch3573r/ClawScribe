"use client";

import { useEffect, useState } from "react";
import { Switch } from "./ui/switch";
import { AlertCircle, Cloud, Cpu, Users } from "lucide-react";
import { Button } from "./ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import { getParakeetDirectml, setParakeetDirectml } from "@/lib/parakeetAccel";
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
  const [parakeetDml, setParakeetDml] = useState(false);
  useEffect(() => {
    setParakeetDml(getParakeetDirectml());
  }, []);
  const onToggleParakeetDml = (checked: boolean) => {
    setParakeetDml(checked);
    void setParakeetDirectml(checked);
  };

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

  const cloudTranscription = useCloudTranscription();
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
      <div className="flex items-start gap-3 p-4 bg-yellow-50 border border-yellow-200 rounded-md">
        <AlertCircle className="h-5 w-5 text-yellow-600 flex-shrink-0 mt-0.5" />
        <div className="text-sm text-yellow-800">
          <p className="font-medium">Experimental Settings</p>
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
              <h3 className="text-lg font-semibold text-foreground">
                GPU transcription (Parakeet · DirectML)
              </h3>
              <span className="rounded-full bg-yellow-100 px-2 py-0.5 text-xs font-medium text-yellow-800">
                BETA
              </span>
            </div>
            <p className="text-sm text-muted-foreground">
              Run Parakeet through DirectML on your GPU instead of the CPU. Uses
              the current int8 model, so unsupported ops fall back to CPU —
              useful to benchmark whether your GPU helps. Takes effect on the
              next recording. Windows only.
            </p>
          </div>
          <div className="ml-6">
            <Switch checked={parakeetDml} onCheckedChange={onToggleParakeetDml} />
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
              <span className="rounded-full bg-yellow-100 px-2 py-0.5 text-xs font-medium text-yellow-800">
                BETA
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
                Enable Recording &gt; Save Audio Recordings first. Source
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
              <span className="rounded-full bg-yellow-100 px-2 py-0.5 text-xs font-medium text-yellow-800">
                BETA
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
