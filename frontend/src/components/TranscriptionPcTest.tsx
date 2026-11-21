"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Gauge } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Meta, SettingsCard, SettingsRow } from "@/components/ui/settings-layout";
import { DEFAULT_PARAKEET_MODEL } from "@/constants/modelDefaults";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import { getModelDisplayName } from "@/lib/parakeet";
import { formatMeasuredAt, formatSpeed, savePcTestResult, speedOf, usePcTestResults } from "@/components/settings/pcTestResults";

interface Sample {
  language: "de" | "en";
  audioSeconds: number;
  elapsedSeconds: number;
  recognized: boolean;
}

interface PcTestResult {
  engine: string;
  model: string;
  backend: string;
  fallbackReason: string | null;
  audioSeconds: number;
  elapsedSeconds: number;
  realtimeFactor: number;
  samples: Sample[];
}

function modelLabel(provider: string, model: string) {
  switch (provider.toLowerCase()) {
    case "parakeet": return "Parakeet · " + getModelDisplayName(model);
    case "nemotron": return "Nemotron · " + (model.toLowerCase().includes("int8") ? "INT8" : "FP16");
    default: return "Whisper · " + model;
  }
}

/**
 * State of the "Test this PC" run for the model that live recording will use.
 * Hosted selections test the local model that live recording falls back to.
 */
export function useTranscriptionPcTest({ provider, model }: { provider: string; model: string }) {
  const recording = useRecordingState();
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<PcTestResult | null>(null);
  const [error, setError] = useState("");
  const pending = useRef(false);
  const revision = useRef(0);
  const cloud = provider === "cloud-whisper" || provider === "mai-transcribe";
  const testProvider = cloud ? "parakeet" : provider;
  const testModel = cloud ? DEFAULT_PARAKEET_MODEL : model;
  const supported = Boolean(testModel) && ["parakeet", "localWhisper", "nemotron"].includes(testProvider);
  const recordingBusy = Boolean(recording.isRecording || recording.isStarting || recording.isStopping);
  const stored = usePcTestResults()[testModel];

  // A result describes one model. Clear it when another model becomes the target.
  useEffect(() => {
    setResult(null);
    setError("");
    return () => { revision.current += 1; };
  }, [provider, model]);

  async function run() {
    if (pending.current || !supported || recordingBusy) return;
    const request = revision.current;
    pending.current = true;
    setRunning(true);
    setResult(null);
    setError("");
    try {
      const measured = await invoke<PcTestResult>("run_transcription_pc_test", { provider: testProvider, model: testModel });
      savePcTestResult(measured.model, measured);
      if (request === revision.current) setResult(measured);
    } catch (problem) {
      if (request === revision.current) setError(typeof problem === "string" ? problem : "Could not complete the test. Check that the selected model is installed and try again.");
    } finally {
      pending.current = false;
      setRunning(false);
    }
  }

  return { recording, running, result, error, cloud, testProvider, testModel, supported, recordingBusy, stored, run };
}

export type PcTest = ReturnType<typeof useTranscriptionPcTest>;

/** Time of the last stored test and the outline button that starts a new one. */
export function PcTestAction({ test }: { test: PcTest }) {
  return (
    <>
      {test.stored && <Meta>Last test {formatMeasuredAt(test.stored.measuredAt)}</Meta>}
      <Button variant="outline" size="sm" disabled={test.running || test.recordingBusy || !test.supported} onClick={() => void test.run()}>
        <Gauge aria-hidden="true" />
        {test.running ? "Testing this PC…" : "Test this PC"}
      </Button>
    </>
  );
}

/** Progress, errors and the outcome of the most recent run. Renders nothing when there is nothing to say. */
export function PcTestReport({ test }: { test: PcTest }) {
  const { recording, running, result, error, supported, recordingBusy } = test;
  const missedSpeech = result?.samples.some(sample => !sample.recognized);
  const complete = result?.samples.length === 2;
  const behind = result && result.realtimeFactor > 1;
  const notes = !supported || recordingBusy || running || error || result;
  if (!notes) return null;
  return (
    <div className="space-y-2 border-t border-border/60 px-5 py-3 text-sm">
      {!supported && <p className="text-muted-foreground">Select an installed local transcription model to run this test.</p>}
      {recordingBusy && <p className="text-muted-foreground">Finish your recording before starting a test.</p>}
      {running && <p role="status" className="text-muted-foreground">Loading the model and transcribing two built-in samples…</p>}
      {error && <p role="alert" className="text-destructive">{error}</p>}
      {result && <div className="space-y-2" role="status">
        <p className="font-medium">
          {behind ? "The model fell behind on these samples." : complete && !missedSpeech && "The model kept up with these samples."}
          {!complete && !missedSpeech && " Speech recognition could not be confirmed for both samples."}
          {missedSpeech && (behind ? " It missed speech on at least one sample." : "The model missed speech on at least one sample.")}
        </p>
        <p>
          {modelLabel(result.engine, result.model)} · {result.backend} · {result.elapsedSeconds.toFixed(1)} s for {result.audioSeconds.toFixed(1)} s of audio
        </p>
        <ul className="list-inside list-disc text-muted-foreground">
          {result.samples.map(sample => <li key={sample.language}>
            {sample.language === "de" ? "German" : "English"} · {sample.elapsedSeconds.toFixed(1)} s for {sample.audioSeconds.toFixed(1)} s of audio
            {!sample.recognized && " · No speech recognized"}
          </li>)}
        </ul>
        {result.fallbackReason && <p className="text-warning-foreground">{result.fallbackReason}</p>}
        {(behind || missedSpeech) && <div className="space-y-2">
          <p className="text-muted-foreground">Try another installed model, or save audio and transcribe after the meeting.</p>
          {recording.recordingMode === "audio_only" ? <p>Record now, transcribe later is selected.</p> :
            <Button variant="outline" size="sm" disabled={recordingBusy || recording.isSavingMode}
              onClick={() => void recording.setRecordingMode("audio_only")}>Use record now, transcribe later</Button>}
          {recording.modeError && <p role="alert" className="text-destructive">{recording.modeError}</p>}
        </div>}
      </div>}
    </div>
  );
}

/** Stand-alone speed card, shown for engines that have no model table of their own. */
export function TranscriptionPcTest({ provider, model }: { provider: string; model: string }) {
  const test = useTranscriptionPcTest({ provider, model });
  return (
    <SettingsCard title="Transcription speed" description="Time your local model on about 30 seconds of German and English speech" actions={<div className="flex items-center gap-3"><PcTestAction test={test} /></div>}>
      {test.supported && <SettingsRow
        title={test.cloud ? "Local Parakeet test model" : "Selected model"}
        description={modelLabel(test.testProvider, test.testModel)}>
        {test.stored
          ? <Meta>{formatSpeed(speedOf(test.stored))}</Meta>
          : <Meta>Not tested</Meta>}
      </SettingsRow>}
      <PcTestReport test={test} />
    </SettingsCard>
  );
}
