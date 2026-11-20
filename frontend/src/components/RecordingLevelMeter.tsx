"use client";

import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { SegmentedLevelMeter } from "@/components/AudioLevelMeter";
import {
  applyLevels,
  applySilence,
  initialMeterState,
  resetMeter,
  type MeterState,
  type RecordingLevelsPayload,
  type SourceState,
} from "@/lib/recording-levels";

const REDUCED_MOTION = "(prefers-reduced-motion: reduce)";

function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(false);
  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const query = window.matchMedia(REDUCED_MOTION);
    const update = () => setReduced(query.matches);
    update();
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);
  return reduced;
}

/**
 * The levels of a running recording: the microphone ("Me") and system audio
 * ("Meeting audio"), from the core's `recording-levels` event. The subscription,
 * the timers and the state all live here, so only this small meter redraws about
 * ten times a second and the page around it does not.
 *
 * - A source the core does not capture reads "Not captured", never silence.
 * - After 500 ms without an event, captured sources read as silence.
 * - While paused the core sends nothing; the meters rest and say so.
 * - With reduced motion the meters refresh twice a second.
 * - A microphone that has made no sound for 10 s since the start (or resume) is
 *   noted quietly in its row. Once it has been heard, never.
 */
export function RecordingLevelMeter({ paused }: { paused: boolean }) {
  const [state, setState] = useState<MeterState>(initialMeterState);
  const reduced = useReducedMotion();
  const live = useRef({ paused, reduced });
  live.current = { paused, reduced };
  const pending = useRef<RecordingLevelsPayload | null>(null);

  // Pausing or resuming starts the readings again.
  useEffect(() => {
    pending.current = null;
    setState(resetMeter());
  }, [paused]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<RecordingLevelsPayload>("recording-levels", (event) => {
      if (live.current.paused) return;
      if (live.current.reduced) {
        pending.current = event.payload;
        return;
      }
      setState((previous) => applyLevels(previous, event.payload, Date.now()));
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch((error) => console.error("Failed to listen for recording levels:", error));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // Turns missing events into silence, and applies the latest reading at the
  // slower pace when motion is reduced.
  useEffect(() => {
    if (paused) return;
    const timer = setInterval(() => {
      const now = Date.now();
      const reading = pending.current;
      pending.current = null;
      setState((previous) => (reading ? applyLevels(previous, reading, now) : applySilence(previous, now)));
    }, reduced ? 500 : 200);
    return () => clearInterval(timer);
  }, [paused, reduced]);

  return (
    <div role="group" aria-label="Recording levels" className="flex flex-col gap-1">
      <MeterRow label="Me" source={state.mic} paused={paused} note={state.micNoSignal ? "No microphone signal yet" : null} />
      <MeterRow label="Meeting audio" source={state.system} paused={paused} note={null} />
      {/* Announced only when it changes, never the levels themselves. */}
      <span role="status" className="sr-only">
        {paused ? "Recording paused" : state.micNoSignal ? "No microphone signal yet" : ""}
      </span>
    </div>
  );
}

function MeterRow({ label, source, paused, note }: { label: string; source: SourceState; paused: boolean; note: string | null }) {
  const reading = !paused && source.kind === "level" ? source : null;
  return (
    <div className="flex items-center gap-2 text-[11px] leading-3">
      <span className="w-[5.5rem] shrink-0 text-muted-foreground">{label}</span>
      {source.kind === "absent" ? (
        <span className="text-muted-foreground">Not captured</span>
      ) : (
        <>
          <SegmentedLevelMeter
            size="mini"
            label={`${label} level`}
            db={reading ? reading.db : null}
            peakDb={reading ? reading.peakDb : null}
            className={paused ? "opacity-60" : ""}
          />
          {paused ? <span className="text-muted-foreground">Paused</span>
            : note ? <span className="whitespace-nowrap text-warning-foreground">{note}</span> : null}
        </>
      )}
    </div>
  );
}
