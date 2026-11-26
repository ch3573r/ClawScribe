"use client";

import { Button } from "@/components/ui/button";
import { SettingsCard, SettingsFooter, StatusText, type StatusTone } from "@/components/ui/settings-layout";
import { LevelTicks, SegmentedLevelMeter } from "@/components/AudioLevelMeter";
import { formatDb, type AudioLevelFeed } from "@/components/settings/useAudioLevelFeed";
import type { useSignalCheck } from "@/components/settings/useSignalCheck";

type Feed = ReturnType<typeof useSignalCheck>;

const statusText: Record<NonNullable<Feed["result"]>["microphone"], { tone: StatusTone; text: string }> = {
  detected: { tone: "ok", text: "Signal detected" },
  noSignal: { tone: "warning", text: "No signal detected" },
  unavailable: { tone: "error", text: "Could not open this device" },
};

function SignalMeter({ label, reading }: { label: string; reading?: { db: number; peakDb: number } | null }) {
  return <>
    <div className="mb-2 flex items-baseline justify-between gap-3">
      <span className="text-sm">{label}</span>
      <span className="font-mono text-sm tabular-nums text-foreground">{reading ? formatDb(reading.db) : "—"}</span>
    </div>
    <SegmentedLevelMeter label={label + " level"} db={reading?.db ?? null} peakDb={reading?.peakDb ?? null} />
    <LevelTicks />
  </>;
}

export function SignalCheck({ feed, unavailableReason }: { feed: Feed; unavailableReason: string | null }) {
  const sources = [
    { source: "microphone", label: "Microphone" },
    { source: "systemAudio", label: "System audio" },
  ] as const;

  return (
    <SettingsCard title="Signal check" description="Speak and play sound during the five-second check. ClawScribe only measures levels.">
      {sources.map(({ source, label }) => {
        const reading = unavailableReason ? null : feed.levels[source];
        const outcome = unavailableReason ? null : feed.result?.[source];
        return (
          <div key={source} className="px-5 pb-4 pt-3">
            <SignalMeter label={label} reading={reading} />
            <div aria-live="polite">
              {outcome && <StatusText tone={statusText[outcome].tone}>{statusText[outcome].text}</StatusText>}
            </div>
          </div>
        );
      })}
      <SettingsFooter actions={
        <Button variant="outline" size="sm" disabled={feed.running || !!unavailableReason} onClick={() => void feed.start()}>
          {feed.running ? "Checking audio…" : "Check audio"}
        </Button>
      }>
        {unavailableReason && <StatusText tone="off">{unavailableReason}</StatusText>}
        {!unavailableReason && feed.error && <span role="alert"><StatusText tone="error" className="whitespace-normal">{feed.error}</StatusText></span>}
      </SettingsFooter>
    </SettingsCard>
  );
}

export function MicrophoneCheck({ feed, unavailableReason }: { feed: AudioLevelFeed; unavailableReason: string | null }) {
  const reading = unavailableReason ? null : feed.microphone;
  const error = !unavailableReason && feed.status === "error";
  return (
    <SettingsCard title="Signal check" description="Speak and watch the microphone level.">
      <div className="px-5 pb-4 pt-3">
        <SignalMeter label="Microphone" reading={reading} />
      </div>
      <SettingsFooter actions={error ? <Button variant="outline" size="sm" onClick={feed.retry}>Try again</Button> : undefined}>
        <StatusText tone={error ? "error" : reading?.active ? "ok" : "off"} className="whitespace-normal">
          {unavailableReason ?? (error ? feed.error ?? "Could not open the microphone"
            : reading ? reading.active ? "Signal detected" : "No signal detected" : "Waiting for the microphone")}
        </StatusText>
      </SettingsFooter>
    </SettingsCard>
  );
}
