"use client";

import { useEffect, useState } from "react";
import { Switch } from "./ui/switch";
import { AlertCircle, Cpu, Users } from "lucide-react";
import { getParakeetDirectml, setParakeetDirectml } from "@/lib/parakeetAccel";
import {
  getSourceAttribution,
  setSourceAttribution,
} from "@/lib/sourceAttribution";

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
  const [sourceAttribution, setSourceAttributionState] = useState(false);
  useEffect(() => {
    setSourceAttributionState(getSourceAttribution());
  }, []);
  const onToggleSourceAttribution = (checked: boolean) => {
    setSourceAttributionState(checked);
    void setSourceAttribution(checked);
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
          </div>
          <div className="ml-6">
            <Switch
              checked={sourceAttribution}
              onCheckedChange={onToggleSourceAttribution}
            />
          </div>
        </div>
      </div>

      {/* Info Box */}
      <div className="p-4 rounded-lg border border-primary/30 bg-primary/10">
        <p className="text-sm text-primary">
          <strong>Note:</strong> Import Audio and Retranscribe are now stable
          features and are available from Home, the sidebar, and meeting details.
        </p>
      </div>
    </div>
  );
}
