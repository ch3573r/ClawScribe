"use client";

import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { PageSection } from "@/components/ui/page-section";
import { DEFAULT_PARAKEET_MODEL } from "@/constants/modelDefaults";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import { getModelDisplayName } from "@/lib/parakeet";

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

export function TranscriptionPcTest({ provider, model }: { provider: string; model: string }) {
  const recording = useRecordingState();
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<PcTestResult | null>(null);
  const [error, setError] = useState("");
  const pending = useRef(false);
  const cloud = provider === "cloud-whisper" || provider === "mai-transcribe";
  const testProvider = cloud ? "parakeet" : provider;
  const testModel = cloud ? DEFAULT_PARAKEET_MODEL : model;
  const supported = Boolean(testModel) && ["parakeet", "localWhisper", "nemotron"].includes(testProvider);
  const recordingBusy = recording.isRecording || recording.isStarting || recording.isStopping;
  const missedSpeech = result?.samples.some(sample => !sample.recognized);
  const behind = result && result.realtimeFactor > 1;

  async function run() {
    if (pending.current || !supported || recordingBusy) return;
    pending.current = true;
    setRunning(true);
    setResult(null);
    setError("");
    try {
      const measured = await invoke<PcTestResult>("run_transcription_pc_test", { provider: testProvider, model: testModel });
      setResult(measured);
    } catch (problem) {
      setError(typeof problem === "string" ? problem : "Could not complete the test. Check that the selected model is installed and try again.");
    } finally {
      pending.current = false;
      setRunning(false);
    }
  }

  return (
    <PageSection title="Transcription speed" description="Time your local model on about 30 seconds of German and English speech.">
      <div className="space-y-3 p-5">
        {supported && <p className="text-sm text-muted-foreground">
          {cloud ? "Local model used for live recording" : "Selected model"} · {modelLabel(testProvider, testModel)}
        </p>}
        <Button disabled={running || recordingBusy || !supported} onClick={() => void run()}>
          {running ? "Testing this PC…" : "Test this PC"}
        </Button>
        {!supported && <p className="text-sm text-muted-foreground">Select an installed local transcription model to run this test.</p>}
        {recordingBusy && <p className="text-sm text-muted-foreground">Finish your recording before starting a test.</p>}
        {running && <p role="status" className="text-sm text-muted-foreground">Loading the model and transcribing two built-in samples…</p>}
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        {result && <div className="space-y-2" role="status">
          <p className="text-sm font-medium">
            {behind ? "The model fell behind on these samples." : !missedSpeech && "The model kept up with these samples."}
            {missedSpeech && (behind ? " It missed speech on at least one sample." : "The model missed speech on at least one sample.")}
          </p>
          <p className="text-sm">
            {modelLabel(result.engine, result.model)} · {result.backend} · {result.elapsedSeconds.toFixed(1)} s for {result.audioSeconds.toFixed(1)} s of audio
          </p>
          <ul className="list-inside list-disc text-sm text-muted-foreground">
            {result.samples.map(sample => <li key={sample.language}>
              {sample.language === "de" ? "German" : "English"} · {sample.elapsedSeconds.toFixed(1)} s for {sample.audioSeconds.toFixed(1)} s of audio
              {!sample.recognized && " · No speech recognized"}
            </li>)}
          </ul>
          {result.fallbackReason && <p className="text-sm text-warning">{result.fallbackReason}</p>}
          {(behind || missedSpeech) && <div className="space-y-2">
            <p className="text-sm text-muted-foreground">Try another installed model above, or save audio and transcribe after the meeting.</p>
            {recording.recordingMode === "audio_only" ? <p className="text-sm">Record now, transcribe later is selected.</p> :
              <Button variant="outline" disabled={recordingBusy || recording.isSavingMode}
                onClick={() => void recording.setRecordingMode("audio_only")}>Use record now, transcribe later</Button>}
            {recording.modeError && <p role="alert" className="text-sm text-destructive">{recording.modeError}</p>}
          </div>}
        </div>}
      </div>
    </PageSection>
  );
}
