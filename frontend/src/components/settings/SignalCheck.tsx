"use client";

import { Button } from "@/components/ui/button";
import { SettingsCard, SettingsFooter, StatusText, type StatusTone } from "@/components/ui/settings-layout";
import { LevelTicks, SegmentedLevelMeter } from "@/components/AudioLevelMeter";
import { formatDb, type AudioLevelFeed } from "@/components/settings/useAudioLevelFeed";

/** One-line summary of the feed under the meters. */
export function signalStatus(feed: AudioLevelFeed, unavailableReason: string | null): { tone: StatusTone; text: string } {
  if (unavailableReason) return { tone: "off", text: unavailableReason };
  if (feed.status === "error") return { tone: "error", text: feed.error ?? "The microphone check could not start" };
  if (feed.status !== "live") return { tone: "off", text: "Starting the microphone check" };
  if (feed.monitored?.usedDefault) return { tone: "warning", text: "Your chosen microphone is unavailable. Showing the Windows default" };
  if (!feed.microphone) return { tone: "off", text: "Waiting for the first reading" };
  return feed.microphone.active
    ? { tone: "ok", text: "The microphone is sending audio" }
    : { tone: "neutral", text: "No sound from the microphone yet" };
}

/**
 * Live level of the microphone. The native feed measures input devices only,
 * so system audio is labelled as not measured instead of drawing a made-up bar.
 */
export function SignalCheck({ feed, unavailableReason }: { feed: AudioLevelFeed; unavailableReason: string | null }) {
  const reading = unavailableReason ? null : feed.microphone;
  const status = signalStatus(feed, unavailableReason);
  return (
    <SettingsCard title="Signal check" description="Speak and watch the meter. Nothing is recorded.">
      <div className="px-5 pb-5 pt-4">
        <div className="mb-3 flex items-baseline justify-between gap-3">
          <span className="text-sm">Microphone</span>
          <span aria-live="off" className="font-mono text-xl tabular-nums text-foreground">{reading ? formatDb(reading.db) : "- dB"}</span>
        </div>
        <SegmentedLevelMeter label="Microphone level" db={reading ? reading.db : null} peakDb={reading ? reading.peakDb : null} />
        <LevelTicks />
      </div>
      <div className="px-5 pb-5 pt-4">
        <div className="flex items-baseline justify-between gap-3">
          <span className="text-sm">System audio</span>
          <span className="text-sm text-muted-foreground">Not measured here</span>
        </div>
      </div>
      <SettingsFooter actions={feed.status === "error" && !unavailableReason
        ? <Button variant="ghost" size="sm" onClick={feed.retry}>Try again</Button> : undefined}>
        <StatusText tone={status.tone} className="whitespace-normal text-[13px]">{status.text}</StatusText>
      </SettingsFooter>
    </SettingsCard>
  );
}
