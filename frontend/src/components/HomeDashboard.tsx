"use client";

import { useEffect, useMemo } from "react";
import { Button } from "@/components/ui/button";
import {
  ArrowRight,
  AudioLines,
  ChevronRight,
  Clock3,
  FileText,
  Languages,
  Mic,
  Settings2,
  RefreshCw,
  Sparkles,
  Upload,
  Volume2,
} from "lucide-react";
import { RecordingControls } from "@/components/RecordingControls";
import { RecordingModeSelect } from '@/components/RecordingModeSelect';
import { UpcomingMeetings } from "@/components/UpcomingMeetings";
import { useRouter } from "next/navigation";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { useConfig } from "@/contexts/ConfigContext";
import { useImportDialog } from "@/contexts/ImportDialogContext";
import { RecordingStatus, useRecordingState } from "@/contexts/RecordingStateContext";
import {
  getAudioDeviceDisplayName,
  audioSourceReadiness,
  type AudioDevice,
} from "@/lib/audioDevicePreferences";
import type { useLiveTranscriptionReadiness } from "@/hooks/useLiveTranscriptionReadiness";
import { LIVE_MODEL_FAILURE_MESSAGE, transcriptionSettingsPath } from "@/lib/start-error";
import { useCloudTranscription } from "@/hooks/useCloudTranscription";

interface HomeDashboardProps {
  liveReadiness: ReturnType<typeof useLiveTranscriptionReadiness>;
  isRecording: boolean;
  isProcessingStop: boolean;
  isRecordingDisabled: boolean;
  meetingName?: string;
  onRecordingStart: () => void;
  onStopInitiated: () => void;
  onTranscriptionError: (message: string) => void;
}

const PROVIDER_LABELS: Record<string, string> = {
  localWhisper: "Whisper",
  parakeet: "Parakeet",
  nemotron: "Nemotron",
  deepgram: "Deepgram",
  elevenLabs: "ElevenLabs",
  groq: "Groq",
  openai: "OpenAI",
  "cloud-whisper": "Hosted Whisper",
  "mai-transcribe": "Microsoft AI",
};

const LANGUAGE_LABELS: Record<string, string> = {
  auto: "Auto detect",
  de: "German",
  en: "English",
  es: "Spanish",
  fr: "French",
  it: "Italian",
  nl: "Dutch",
  pl: "Polish",
  pt: "Portuguese",
};

function formatMeetingDate(value?: string): string {
  if (!value) return "Saved meeting";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "Saved meeting";

  return date.toLocaleDateString([], {
    month: "short",
    day: "numeric",
    year: date.getFullYear() === new Date().getFullYear() ? undefined : "numeric",
  });
}

export function HomeDashboard({
  liveReadiness,
  isRecording,
  isProcessingStop,
  isRecordingDisabled,
  meetingName,
  onRecordingStart,
  onStopInitiated,
  onTranscriptionError,
}: HomeDashboardProps) {
  const { meetings } = useSidebar();
  const {
    selectedDevices,
    selectedLanguage,
    audioDeviceInventory,
    devicePreferencesStatus,
    refreshDevicePreferences,
  } = useConfig();
  const { openImportDialog } = useImportDialog();
  const recordingState = useRecordingState();
  const router = useRouter();

  const recentMeetings = useMemo(() => meetings.slice(0, 5), [meetings]);
  const { enabled: hostedEnabled, loaded: hostedSettingLoaded } = useCloudTranscription();
  const { data: inventory, status: inventoryStatus, refresh } = audioDeviceInventory;
  useEffect(() => { void refresh(); }, [refresh]);
  const checkingDevices = inventoryStatus === "checking" || devicePreferencesStatus === "loading";
  const deviceCheckFailed = inventoryStatus === "error" || devicePreferencesStatus === "error";
  const micReady = inventory && audioSourceReadiness(inventory, "Input", selectedDevices.micDevice);
  const systemReady = inventory && audioSourceReadiness(inventory, "Output", selectedDevices.systemDevice);
  const audioOnly = recordingState.recordingMode === "audio_only";
  const modelIssue = audioOnly ? null : liveReadiness.data?.modelIssue;
  const checkingModel = !audioOnly && !liveReadiness.data && !liveReadiness.error;
  const modelCheckFailed = !audioOnly && liveReadiness.error;
  const provider = liveReadiness.data?.provider;
  const providerLabel = provider ? PROVIDER_LABELS[provider] : null;
  const batch = liveReadiness.data?.hostedBatchProvider;
  const languageLabel = LANGUAGE_LABELS[selectedLanguage] ?? selectedLanguage.toUpperCase();
  const firstFailure = deviceCheckFailed ? "Could not check audio devices"
    : checkingDevices ? "Checking audio devices"
    : micReady === "preferred_unavailable" ? "Preferred microphone unavailable"
    : micReady !== "available" ? "Default microphone unavailable"
    : systemReady === "preferred_unavailable" ? "Preferred output unavailable"
    : !recordingState.recordingMode ? "Choose a recording mode"
    : checkingModel ? "Checking speech model"
    : modelCheckFailed ? "Could not check speech model"
    : modelIssue ? "Live transcription needs attention"
    : null;
  const micOnly = !deviceCheckFailed && !checkingDevices && micReady === "available" && systemReady === "default_unavailable";
  const canStart = !firstFailure && !recordingState.isSavingMode;
  const appStatus = isRecording
    ? recordingState.isPaused
      ? { label: "Paused", dot: "bg-warning-foreground", ring: "ring-warning-foreground/20" }
      : { label: "Recording", dot: "bg-error-foreground", ring: "ring-error-foreground/20" }
    : firstFailure || micOnly || recordingState.status === RecordingStatus.ERROR
      ? { label: firstFailure ?? (micOnly ? "Microphone only" : "Previous start failed"), dot: "bg-warning-foreground", ring: "ring-warning-foreground/20" }
      : { label: "Ready to record", dot: "bg-success-foreground", ring: "ring-success-foreground/20" };

  const deviceMeta = (type: AudioDevice["device_type"], preferred: string | null) => {
    if (deviceCheckFailed) return "Could not check device";
    if (checkingDevices || !inventory) return "Checking devices";
    const result = audioSourceReadiness(inventory, type, preferred);
    if (result === "preferred_unavailable") return "Preferred device unavailable";
    if (result === "default_unavailable") return "No system default";
    return preferred ? "Preferred device connected" : "Follows system default";
  };

  const openRecordingSettings = () => router.push("/settings?tab=recording");
  const openTranscriptionSettings = () => router.push(transcriptionSettingsPath(provider));
  const signalSteps = [
    {
      icon: Mic,
      label: "Microphone",
      value: getAudioDeviceDisplayName(selectedDevices.micDevice, inventory?.defaultMicrophone ?? "System default microphone"),
      meta: deviceMeta("Input", selectedDevices.micDevice),
      onClick: openRecordingSettings,
    },
    {
      icon: Volume2,
      label: "System audio",
      value: getAudioDeviceDisplayName(selectedDevices.systemDevice, inventory?.defaultOutput ?? "System default output"),
      meta: deviceMeta("Output", selectedDevices.systemDevice),
      onClick: openRecordingSettings,
    },
    {
      icon: AudioLines,
      label: "Live transcription",
      value: audioOnly ? "Off in audio-only mode" : providerLabel ?? (checkingModel ? "Checking engine" : "Engine unavailable"),
      meta: audioOnly ? "Transcribe from the saved meeting" : providerLabel ? `${languageLabel} · On this device` : "On-device engine required",
      onClick: openTranscriptionSettings,
    },
  ];

  const recheckDevices = () => {
    void refresh();
    if (devicePreferencesStatus === "error") void refreshDevicePreferences();
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto bg-background text-foreground">
      <div className="mx-auto flex min-h-full w-full max-w-[1680px] flex-col gap-5 px-5 py-6 md:px-8 md:py-8">
        <header className="grid gap-5 lg:grid-cols-[1fr_auto] lg:items-end">
          <div className="max-w-3xl">
            <div className="mb-3 flex items-center gap-3 font-mono text-[11px] font-medium tracking-[0.2em] text-muted-foreground">
              <span className="h-px w-8 bg-primary" aria-hidden="true" />
              Capture workspace
            </div>
            <h1 className="text-3xl font-semibold tracking-[-0.03em] text-foreground md:text-4xl">
              Record a meeting
            </h1>
            <p className="mt-3 max-w-2xl text-sm leading-6 text-muted-foreground md:text-base">
              Check your audio devices and choose how to record.
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-2 lg:justify-end">
            <div role="status" className="inline-flex h-10 items-center gap-2.5 rounded-md border border-border bg-card px-3.5 text-sm text-muted-foreground shadow-sm">
              <span className={`h-2 w-2 rounded-full ring-4 ${appStatus.dot} ${appStatus.ring}`} />
              {appStatus.label}
            </div>
            <Button
              variant="ghost"
              type="button"
              onClick={() => openImportDialog()}
              className="h-auto whitespace-normal inline-flex h-10 items-center gap-2 rounded-md border border-border bg-card px-3.5 text-sm font-medium text-foreground shadow-sm transition-colors hover:border-primary/40 hover:bg-primary/10 hover:text-foreground"
            >
              <Upload className="h-4 w-4 text-primary" />
              Import media
            </Button>
          </div>
        </header>

        <div className="grid gap-5 xl:grid-cols-[minmax(0,1.5fr)_minmax(320px,0.72fr)]">
          <section className="relative min-h-[390px] overflow-hidden rounded-lg border border-border bg-card shadow-sm">
            <div
              className="pointer-events-none absolute inset-0 opacity-50"
              style={{
                backgroundImage:
                  "linear-gradient(hsl(var(--border) / 0.22) 1px, transparent 1px), linear-gradient(90deg, hsl(var(--border) / 0.22) 1px, transparent 1px)",
                backgroundSize: "34px 34px",
                maskImage: "linear-gradient(to bottom, hsl(var(--foreground)), transparent 86%)",
              }}
              aria-hidden="true"
            />
            <div className="pointer-events-none absolute -right-28 -top-36 h-80 w-80 rounded-full bg-primary/10 blur-3xl" />

            <div className="relative flex h-full min-h-[390px] flex-col p-6 md:p-8">
              <div className="flex items-start justify-between gap-4">
                <div>
                  <div className="font-mono text-[11px] tracking-[0.18em] text-primary">
                    New capture
                  </div>
                  <h2 className="mt-2 text-2xl font-semibold tracking-tight text-foreground">
                    Record this meeting
                  </h2>
                </div>
              </div>

              <div className="flex flex-1 flex-col items-center justify-center py-10 text-center">
                <div className="mb-6 flex h-14 w-14 items-center justify-center rounded-full border border-primary/30 bg-primary/10 text-primary shadow-[0_0_50px_hsl(var(--primary)/0.15)]">
                  <AudioLines className="h-6 w-6" />
                </div>
                <p className="max-w-md text-sm leading-6 text-muted-foreground">
                  {audioOnly ? "Audio is saved on this device for later transcription." : "Live speech recognition runs on this device."}
                </p>

                <div className="mt-7 space-y-4 rounded-lg border border-border bg-background/85 p-5 shadow-lg backdrop-blur-sm">
                  <RecordingModeSelect />
                  {micOnly && <p role="status" className="max-w-sm text-sm text-warning-foreground">No default output is available. Only your microphone will be recorded.</p>}
                  {deviceCheckFailed && <p role="alert" className="max-w-sm text-sm text-destructive">Could not verify the audio devices or load their preferences. Recheck devices to try again.</p>}
                  {(micReady === "preferred_unavailable" || micReady === "default_unavailable" || systemReady === "preferred_unavailable") &&
                    <p role="status" className="max-w-sm text-sm text-warning-foreground">Reconnect the device or choose an available device in Recording settings.</p>}
                  {modelIssue && <p role="status" className="max-w-sm text-sm text-warning-foreground">{LIVE_MODEL_FAILURE_MESSAGE[modelIssue]}</p>}
                  {modelCheckFailed && <Button variant="outline" onClick={() => void liveReadiness.refresh()}>Recheck speech model</Button>}
                  {(modelIssue || modelCheckFailed) && (
                    <div className="flex flex-wrap justify-center gap-2">
                      <Button variant="outline" onClick={openTranscriptionSettings}>Transcription settings</Button>
                      <Button variant="outline" disabled={recordingState.isSavingMode}
                        onClick={() => void recordingState.setRecordingMode("audio_only")}>Use audio-only mode</Button>
                    </div>
                  )}
                  <RecordingControls
                    variant="dashboard"
                    isRecording={recordingState.isRecording}
                    onRecordingStart={onRecordingStart}
                    onStopInitiated={onStopInitiated}
                    onTranscriptionError={onTranscriptionError}
                    isRecordingDisabled={isRecordingDisabled || !canStart}
                    isParentProcessing={isProcessingStop}
                    selectedDevices={selectedDevices}
                    meetingName={meetingName}
                  />
                </div>
              </div>

              <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border pt-4 text-xs text-muted-foreground">
                <span>
                  {batch
                    ? `${PROVIDER_LABELS[batch]} · Hosted file transcription${hostedSettingLoaded && !hostedEnabled ? " (off)" : ""}`
                    : "Audio capture stays on this device"}
                </span>
                <Button
                  variant="ghost"
                  type="button"
                  onClick={openRecordingSettings}
                  className="h-auto whitespace-normal p-0 text-xs hover:bg-transparent inline-flex items-center gap-1.5 font-medium text-foreground transition-colors hover:text-primary"
                >
                  Recording settings <ArrowRight className="!h-3.5 !w-3.5" />
                </Button>
              </div>
            </div>
          </section>

          <aside className="rounded-lg border border-border bg-card p-6 shadow-sm">
            <div className="flex items-start justify-between gap-4">
              <div>
                <div className="font-mono text-[11px] tracking-[0.18em] text-muted-foreground">
                  Active configuration
                </div>
                <h2 className="mt-2 text-xl font-semibold tracking-tight text-foreground">
                  Signal path
                </h2>
                <p className="mt-1 text-sm leading-5 text-muted-foreground">
                  Devices and engine for the next recording.
                </p>
              </div>
              <Settings2 className="mt-1 h-5 w-5 text-primary" />
            </div>

            <div className="mt-6 space-y-3">
              {signalSteps.map((step) => (
                <Button
                  variant="ghost"
                  key={step.label}
                  type="button"
                  onClick={step.onClick}
                  className="h-auto whitespace-normal font-normal group relative grid w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 rounded-md border border-border bg-background px-3 py-3.5 text-left transition-colors hover:border-primary/40 hover:bg-primary/5 hover:text-foreground"
                >
                  <step.icon className="h-4 w-4 text-muted-foreground" aria-hidden="true" />
                  <span className="min-w-0">
                    <span className="flex items-center gap-2 text-xs text-muted-foreground">
                      {step.label}
                    </span>
                    <span className="mt-1 block truncate text-sm font-medium text-foreground" title={step.value}>
                      {step.value}
                    </span>
                    <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">
                      {step.meta}
                    </span>
                  </span>
                  <ChevronRight className="h-4 w-4 text-muted-foreground transition-transform group-hover:translate-x-0.5 group-hover:text-primary" />
                </Button>
              ))}
            </div>

            <Button variant="outline" className="mt-5 w-full" onClick={recheckDevices} disabled={checkingDevices}>
              <RefreshCw aria-hidden="true" />
              Recheck devices
            </Button>
          </aside>
        </div>

        <UpcomingMeetings />

        <div className="grid gap-5 xl:grid-cols-[minmax(0,1.5fr)_minmax(320px,0.72fr)]">
          <section className="overflow-hidden rounded-lg border border-border bg-card shadow-sm">
            <div className="flex items-end justify-between gap-4 border-b border-border px-6 py-5">
              <div>
                <div className="font-mono text-[11px] tracking-[0.18em] text-muted-foreground">
                  Library
                </div>
                <h2 className="mt-1.5 text-xl font-semibold tracking-tight text-foreground">
                  Recent meetings
                </h2>
              </div>
              <span className="font-mono text-xs text-muted-foreground">
                {meetings.length.toString().padStart(2, "0")} total
              </span>
            </div>

            <div className="divide-y divide-border">
              {recentMeetings.length > 0 ? (
                recentMeetings.map((meeting, index) => (
                  <Button
                    variant="ghost"
                    key={meeting.id}
                    type="button"
                    onClick={() => router.push(`/meeting-details?id=${meeting.id}`)}
                    className="h-auto whitespace-normal font-normal rounded-none group grid w-full grid-cols-[2rem_minmax(0,1fr)_auto] items-center gap-4 px-6 py-4 text-left transition-colors hover:bg-muted hover:text-foreground"
                  >
                    <span className="font-mono text-[11px] text-muted-foreground">
                      {(index + 1).toString().padStart(2, "0")}
                    </span>
                    <span className="min-w-0">
                      <span className="block truncate text-sm font-medium text-foreground">
                        {meeting.title}
                      </span>
                      <span className="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground">
                        <Clock3 className="!h-3.5 !w-3.5" />
                        {formatMeetingDate(meeting.created_at ?? meeting.updated_at)}
                      </span>
                    </span>
                    <span className="flex h-8 w-8 items-center justify-center rounded-md border border-transparent text-muted-foreground transition-colors group-hover:border-border group-hover:bg-card group-hover:text-primary">
                      <ArrowRight className="h-4 w-4" />
                    </span>
                  </Button>
                ))
              ) : (
                <div className="flex flex-col items-center px-6 py-12 text-center">
                  <span className="flex h-11 w-11 items-center justify-center rounded-md border border-border bg-muted text-primary">
                    <FileText className="h-5 w-5" />
                  </span>
                  <p className="mt-4 text-sm font-medium text-foreground">No meetings yet</p>
                  <p className="mt-1 max-w-sm text-sm text-muted-foreground">
                    Your first completed recording will appear here.
                  </p>
                </div>
              )}
            </div>
          </section>

          <section className="rounded-lg border border-border bg-card p-6 shadow-sm">
            <div className="font-mono text-[11px] tracking-[0.18em] text-muted-foreground">
              Workbench
            </div>
            <h2 className="mt-1.5 text-xl font-semibold tracking-tight text-foreground">
              Quick actions
            </h2>
            <p className="mt-1 text-sm text-muted-foreground">
              Common routes without leaving the capture flow.
            </p>

            <div className="mt-5 space-y-2.5">
              {[
                {
                  icon: Upload,
                  label: "Import audio or video",
                  detail: "Transcribe an existing file",
                  onClick: () => openImportDialog(),
                },
                {
                  icon: Languages,
                  label: "Transcription setup",
                  detail: "Speech engines and models",
                  onClick: () => router.push("/settings?tab=transcription"),
                },
                {
                  icon: Sparkles,
                  label: "Summary setup",
                  detail: "Provider, model, and output",
                  onClick: () => router.push("/settings?tab=summary"),
                },
              ].map((item) => (
                <Button
                  variant="ghost"
                  key={item.label}
                  type="button"
                  onClick={item.onClick}
                  className="h-auto whitespace-normal font-normal group flex w-full items-center gap-3 rounded-md border border-border bg-background px-3.5 py-3 text-left transition-colors hover:border-primary/40 hover:bg-primary/5 hover:text-foreground"
                >
                  <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
                    <item.icon className="h-4 w-4" />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block text-sm font-medium text-foreground">{item.label}</span>
                    <span className="mt-0.5 block truncate text-xs text-muted-foreground">{item.detail}</span>
                  </span>
                  <ChevronRight className="h-4 w-4 text-muted-foreground group-hover:text-primary" />
                </Button>
              ))}
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
