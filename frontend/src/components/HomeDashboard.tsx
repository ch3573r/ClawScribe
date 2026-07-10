"use client";

import { useMemo } from "react";
import {
  ArrowRight,
  AudioLines,
  ChevronRight,
  Clock3,
  FileText,
  Languages,
  Mic,
  Radio,
  Settings2,
  ShieldCheck,
  Sparkles,
  Upload,
  Volume2,
} from "lucide-react";
import { RecordingControls } from "@/components/RecordingControls";
import { UpcomingMeetings } from "@/components/UpcomingMeetings";
import { useRouter } from "next/navigation";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { useConfig } from "@/contexts/ConfigContext";
import { useImportDialog } from "@/contexts/ImportDialogContext";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import {
  getAudioDeviceDisplayName,
  isExplicitAudioDevice,
} from "@/lib/audioDevicePreferences";

interface HomeDashboardProps {
  canRecord: boolean;
  isRecording: boolean;
  isProcessingStop: boolean;
  isRecordingDisabled: boolean;
  barHeights: string[];
  meetingName?: string;
  onRecordingStart: () => void;
  onRecordingStop: (callApi?: boolean) => void;
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
  canRecord,
  isRecording,
  isProcessingStop,
  isRecordingDisabled,
  barHeights,
  meetingName,
  onRecordingStart,
  onRecordingStop,
  onStopInitiated,
  onTranscriptionError,
}: HomeDashboardProps) {
  const { meetings } = useSidebar();
  const {
    selectedDevices,
    selectedLanguage,
    transcriptModelConfig,
  } = useConfig();
  const { openImportDialog } = useImportDialog();
  const recordingState = useRecordingState();
  const router = useRouter();

  const recentMeetings = useMemo(() => meetings.slice(0, 5), [meetings]);
  const micLabel = getAudioDeviceDisplayName(
    selectedDevices.micDevice,
    "System default microphone",
  );
  const systemLabel = getAudioDeviceDisplayName(
    selectedDevices.systemDevice,
    "System default output",
  );
  const providerLabel =
    PROVIDER_LABELS[transcriptModelConfig.provider] ?? transcriptModelConfig.provider;
  const languageLabel =
    LANGUAGE_LABELS[selectedLanguage] ?? selectedLanguage.toUpperCase();
  const isLocalTranscription = ["localWhisper", "parakeet", "nemotron"].includes(
    transcriptModelConfig.provider,
  );

  const appStatus = isRecording
    ? recordingState.isPaused
      ? { label: "Paused", dot: "bg-amber-400", ring: "ring-amber-400/20" }
      : { label: "Recording", dot: "bg-red-400", ring: "ring-red-400/20" }
    : canRecord
      ? { label: "Ready to record", dot: "bg-emerald-400", ring: "ring-emerald-400/20" }
      : { label: "Microphone required", dot: "bg-amber-400", ring: "ring-amber-400/20" };

  const signalSteps = [
    {
      number: "01",
      icon: Mic,
      label: "Microphone",
      value: micLabel,
      meta: isExplicitAudioDevice(selectedDevices.micDevice) ? "Preferred device" : "Follows Windows default",
      onClick: () => router.push("/settings?tab=recording"),
    },
    {
      number: "02",
      icon: Volume2,
      label: "Meeting audio",
      value: systemLabel,
      meta: isExplicitAudioDevice(selectedDevices.systemDevice) ? "Preferred device" : "Follows Windows default",
      onClick: () => router.push("/settings?tab=recording"),
    },
    {
      number: "03",
      icon: AudioLines,
      label: "Transcription",
      value: providerLabel,
      meta: `${languageLabel} · ${isLocalTranscription ? "On-device" : "Hosted"}`,
      onClick: () => router.push("/settings?tab=transcription"),
    },
  ];

  return (
    <div className="min-h-0 flex-1 overflow-y-auto bg-background text-foreground">
      <div className="mx-auto flex min-h-full w-full max-w-[1680px] flex-col gap-5 px-5 py-6 md:px-8 md:py-8">
        <header className="grid gap-5 lg:grid-cols-[1fr_auto] lg:items-end">
          <div className="max-w-3xl">
            <div className="mb-3 flex items-center gap-3 font-mono text-[11px] font-medium uppercase tracking-[0.2em] text-muted-foreground">
              <span className="h-px w-8 bg-primary" aria-hidden="true" />
              Capture workspace
            </div>
            <h1 className="text-3xl font-semibold tracking-[-0.03em] text-foreground md:text-4xl">
              Everything ready for the next conversation.
            </h1>
            <p className="mt-3 max-w-2xl text-sm leading-6 text-muted-foreground md:text-base">
              Check the signal path, start the recording, and keep the meeting in one place.
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-2 lg:justify-end">
            <div className="inline-flex h-10 items-center gap-2.5 rounded-md border border-border bg-card px-3.5 text-sm text-muted-foreground shadow-sm">
              <span className={`h-2 w-2 rounded-full ring-4 ${appStatus.dot} ${appStatus.ring}`} />
              {appStatus.label}
            </div>
            <button
              type="button"
              onClick={() => openImportDialog()}
              className="inline-flex h-10 items-center gap-2 rounded-md border border-border bg-card px-3.5 text-sm font-medium text-foreground shadow-sm transition-colors hover:border-primary/40 hover:bg-primary/10"
            >
              <Upload className="h-4 w-4 text-primary" />
              Import media
            </button>
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
                maskImage: "linear-gradient(to bottom, black, transparent 86%)",
              }}
              aria-hidden="true"
            />
            <div className="pointer-events-none absolute -right-28 -top-36 h-80 w-80 rounded-full bg-primary/10 blur-3xl" />

            <div className="relative flex h-full min-h-[390px] flex-col p-6 md:p-8">
              <div className="flex items-start justify-between gap-4">
                <div>
                  <div className="font-mono text-[11px] uppercase tracking-[0.18em] text-primary">
                    New capture
                  </div>
                  <h2 className="mt-2 text-2xl font-semibold tracking-tight text-foreground">
                    Record this meeting
                  </h2>
                </div>
                <div className="hidden items-center gap-2 rounded-md border border-border bg-background/75 px-3 py-2 font-mono text-[11px] uppercase tracking-wider text-muted-foreground backdrop-blur-sm sm:flex">
                  <Radio className="h-3.5 w-3.5 text-primary" />
                  Two-channel capture
                </div>
              </div>

              <div className="flex flex-1 flex-col items-center justify-center py-10 text-center">
                <div className="mb-6 flex h-14 w-14 items-center justify-center rounded-full border border-primary/30 bg-primary/10 text-primary shadow-[0_0_50px_hsl(var(--primary)/0.15)]">
                  <AudioLines className="h-6 w-6" />
                </div>
                <p className="max-w-md text-sm leading-6 text-muted-foreground">
                  {canRecord
                    ? "Your microphone and meeting audio will be captured on separate channels for a cleaner transcript."
                    : "Grant microphone access before starting a capture."}
                </p>

                <div className="mt-7 rounded-lg border border-border bg-background/85 p-5 shadow-lg backdrop-blur-sm">
                  {canRecord ? (
                    <RecordingControls
                      variant="dashboard"
                      isRecording={recordingState.isRecording}
                      onRecordingStop={onRecordingStop}
                      onRecordingStart={onRecordingStart}
                      onTranscriptReceived={() => {}}
                      onStopInitiated={onStopInitiated}
                      barHeights={barHeights}
                      onTranscriptionError={onTranscriptionError}
                      isRecordingDisabled={isRecordingDisabled}
                      isParentProcessing={isProcessingStop}
                      selectedDevices={selectedDevices}
                      meetingName={meetingName}
                    />
                  ) : (
                    <div className="max-w-sm rounded-md border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-left text-sm text-amber-800 dark:text-amber-100">
                      Microphone access is required before recording.
                    </div>
                  )}
                </div>
              </div>

              <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border pt-4 text-xs text-muted-foreground">
                <span className="inline-flex items-center gap-2">
                  {isLocalTranscription ? (
                    <ShieldCheck className="h-4 w-4 text-emerald-400" />
                  ) : (
                    <Radio className="h-4 w-4 text-primary" />
                  )}
                  {isLocalTranscription ? "Transcription stays on this device" : "Hosted transcription is selected"}
                </span>
                <button
                  type="button"
                  onClick={() => router.push("/settings?tab=recording")}
                  className="inline-flex items-center gap-1.5 font-medium text-foreground transition-colors hover:text-primary"
                >
                  Recording settings <ArrowRight className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
          </section>

          <aside className="rounded-lg border border-border bg-card p-6 shadow-sm">
            <div className="flex items-start justify-between gap-4">
              <div>
                <div className="font-mono text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
                  Active configuration
                </div>
                <h2 className="mt-2 text-xl font-semibold tracking-tight text-foreground">
                  Signal path
                </h2>
                <p className="mt-1 text-sm leading-5 text-muted-foreground">
                  Settings changes appear here immediately.
                </p>
              </div>
              <Settings2 className="mt-1 h-5 w-5 text-primary" />
            </div>

            <div className="relative mt-6 space-y-3 before:absolute before:bottom-7 before:left-[1.15rem] before:top-7 before:w-px before:bg-border">
              {signalSteps.map((step) => (
                <button
                  key={step.number}
                  type="button"
                  onClick={step.onClick}
                  className="group relative grid w-full grid-cols-[2.3rem_minmax(0,1fr)_auto] items-center gap-3 rounded-md border border-border bg-background px-3 py-3.5 text-left transition-colors hover:border-primary/40 hover:bg-primary/5"
                >
                  <span className="relative z-10 flex h-9 w-9 items-center justify-center rounded-full border border-border bg-card font-mono text-[10px] text-muted-foreground group-hover:border-primary/40 group-hover:text-primary">
                    {step.number}
                  </span>
                  <span className="min-w-0">
                    <span className="flex items-center gap-2 text-xs text-muted-foreground">
                      <step.icon className="h-3.5 w-3.5" />
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
                </button>
              ))}
            </div>

            <button
              type="button"
              onClick={() => router.push("/settings?tab=recording")}
              className="mt-5 inline-flex w-full items-center justify-center gap-2 rounded-md border border-border bg-muted px-4 py-2.5 text-sm font-medium text-foreground transition-colors hover:border-primary/40 hover:bg-primary/10"
            >
              <Settings2 className="h-4 w-4 text-primary" />
              Configure audio devices
            </button>
          </aside>
        </div>

        <UpcomingMeetings />

        <div className="grid gap-5 xl:grid-cols-[minmax(0,1.5fr)_minmax(320px,0.72fr)]">
          <section className="overflow-hidden rounded-lg border border-border bg-card shadow-sm">
            <div className="flex items-end justify-between gap-4 border-b border-border px-6 py-5">
              <div>
                <div className="font-mono text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
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
                  <button
                    key={meeting.id}
                    type="button"
                    onClick={() => router.push(`/meeting-details?id=${meeting.id}`)}
                    className="group grid w-full grid-cols-[2rem_minmax(0,1fr)_auto] items-center gap-4 px-6 py-4 text-left transition-colors hover:bg-muted"
                  >
                    <span className="font-mono text-[11px] text-muted-foreground">
                      {(index + 1).toString().padStart(2, "0")}
                    </span>
                    <span className="min-w-0">
                      <span className="block truncate text-sm font-medium text-foreground">
                        {meeting.title}
                      </span>
                      <span className="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground">
                        <Clock3 className="h-3.5 w-3.5" />
                        {formatMeetingDate(meeting.created_at ?? meeting.updated_at)}
                      </span>
                    </span>
                    <span className="flex h-8 w-8 items-center justify-center rounded-md border border-transparent text-muted-foreground transition-colors group-hover:border-border group-hover:bg-card group-hover:text-primary">
                      <ArrowRight className="h-4 w-4" />
                    </span>
                  </button>
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
            <div className="font-mono text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
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
                  detail: `${providerLabel} · ${languageLabel}`,
                  onClick: () => router.push("/settings?tab=transcription"),
                },
                {
                  icon: Sparkles,
                  label: "Summary setup",
                  detail: "Provider, model, and output",
                  onClick: () => router.push("/settings?tab=summary"),
                },
              ].map((item) => (
                <button
                  key={item.label}
                  type="button"
                  onClick={item.onClick}
                  className="group flex w-full items-center gap-3 rounded-md border border-border bg-background px-3.5 py-3 text-left transition-colors hover:border-primary/40 hover:bg-primary/5"
                >
                  <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
                    <item.icon className="h-4 w-4" />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block text-sm font-medium text-foreground">{item.label}</span>
                    <span className="mt-0.5 block truncate text-xs text-muted-foreground">{item.detail}</span>
                  </span>
                  <ChevronRight className="h-4 w-4 text-muted-foreground group-hover:text-primary" />
                </button>
              ))}
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
