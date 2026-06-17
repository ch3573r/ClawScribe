"use client";

import { useEffect, useState } from "react";
import { Switch } from "./ui/switch";
import { FlaskConical, AlertCircle, Cpu, Users } from "lucide-react";
import { useConfig } from "@/contexts/ConfigContext";
import {
  BetaFeatureKey,
  BETA_FEATURE_NAMES,
  BETA_FEATURE_DESCRIPTIONS,
} from "@/types/betaFeatures";
import { getParakeetDirectml, setParakeetDirectml } from "@/lib/parakeetAccel";
import { getNemotronDirectml, setNemotronDirectml } from "@/lib/nemotronAccel";
import {
  getSourceAttribution,
  setSourceAttribution,
} from "@/lib/sourceAttribution";

export function BetaSettings() {
  const { betaFeatures, toggleBetaFeature } = useConfig();

  // Define feature order for display (allows custom ordering)
  const featureOrder: BetaFeatureKey[] = ["importAndRetranscribe"];

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

  // Nemotron DirectML (GPU) — Nemotron is heavy on CPU; DirectML is the only
  // GPU path for ONNX on Windows (the Vulkan build only accelerates Whisper).
  const [nemotronDml, setNemotronDml] = useState(false);
  useEffect(() => {
    setNemotronDml(getNemotronDirectml());
  }, []);
  const onToggleNemotronDml = (checked: boolean) => {
    setNemotronDml(checked);
    void setNemotronDirectml(checked);
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
      {/* Yellow Warning Banner */}
      <div className="flex items-start gap-3 p-4 bg-yellow-50 border border-yellow-200 rounded-md">
        <AlertCircle className="h-5 w-5 text-yellow-600 flex-shrink-0 mt-0.5" />
        <div className="text-sm text-yellow-800">
          <p className="font-medium">Beta Features</p>
          <p className="mt-1">
            These features are still being tested. You may encounter issues, and
            we appreciate your feedback.
          </p>
        </div>
      </div>

      {/* Dynamic Feature Toggles - Automatically renders all features */}
      {featureOrder.map((featureKey) => (
        <div
          key={featureKey}
          className="rounded-lg border border-border bg-card p-6 shadow-sm"
        >
          <div className="flex items-center justify-between">
            <div className="flex-1">
              <div className="flex items-center gap-2 mb-2">
                <FlaskConical className="h-5 w-5 text-muted-foreground" />
                <h3 className="text-lg font-semibold text-foreground">
                  {BETA_FEATURE_NAMES[featureKey]}
                </h3>
                <span className="px-2 py-0.5 text-xs font-medium bg-yellow-100 text-yellow-800 rounded-full">
                  BETA
                </span>
              </div>
              <p className="text-sm text-muted-foreground">
                {BETA_FEATURE_DESCRIPTIONS[featureKey]}
              </p>
            </div>

            <div className="ml-6">
              <Switch
                checked={betaFeatures[featureKey]}
                onCheckedChange={(checked) =>
                  toggleBetaFeature(featureKey, checked)
                }
              />
            </div>
          </div>
        </div>
      ))}

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

      {/* Nemotron GPU acceleration (DirectML) — experimental, opt-in */}
      <div className="rounded-lg border border-border bg-card p-6 shadow-sm">
        <div className="flex items-center justify-between">
          <div className="flex-1">
            <div className="mb-2 flex items-center gap-2">
              <Cpu className="h-5 w-5 text-muted-foreground" />
              <h3 className="text-lg font-semibold text-foreground">
                GPU transcription (Nemotron · DirectML)
              </h3>
              <span className="rounded-full bg-yellow-100 px-2 py-0.5 text-xs font-medium text-yellow-800">
                BETA
              </span>
            </div>
            <p className="text-sm text-muted-foreground">
              Run Nemotron through DirectML on your GPU instead of the CPU.
              Nemotron is heavy on the CPU, so this is the main way to make it
              fast on Windows (the Vulkan build only accelerates Whisper, not
              ONNX models). Requires a DirectML-enabled build; takes effect on
              the next recording or import. Windows only.
            </p>
          </div>
          <div className="ml-6">
            <Switch checked={nemotronDml} onCheckedChange={onToggleNemotronDml} />
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
          <strong>Note:</strong> When disabled, beta features will be hidden.
          Your existing meetings remain unaffected.
        </p>
      </div>
    </div>
  );
}
