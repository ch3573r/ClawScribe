"use client";

import { useEffect, useRef } from "react";
import { toast } from "sonner";
import { RecordingStatus, useRecordingState } from "@/contexts/RecordingStateContext";
import { useStartRecordingSession } from "@/hooks/useRecordingStart";
import { getCallDetectionStatus } from "@/services/teamsDetectionService";
import { recordingService } from "@/services/recordingService";
import { isDiskSpaceRefusal, startErrorText } from "@/lib/start-error";
import { CALL_PROVIDERS, CALL_SETTINGS_CHANGED, getCallSettings, ObservedCalls, type CallWindow } from "@/lib/autoRecord";

const POLL_INTERVAL_MS = 12_000;
const ignorePageState = () => {};

export function TeamsAutoRecord() {
  const state = useRecordingState();
  const start = useStartRecordingSession(ignorePageState);
  const current = useRef({ state, start });
  current.current = { state, start };

  useEffect(() => {
    const seen = new ObservedCalls();
    let owner: { call: CallWindow; sessionId: string } | null = null;
    let disposed = false;
    let revision = 0;
    let acting = false;
    let prompt: { id: string | number; key: string } | undefined;
    let timer: number;

    const dismiss = () => {
      if (prompt) toast.dismiss(prompt.id);
      prompt = undefined;
    };
    const settingsChanged = () => {
      revision++;
      dismiss();
      seen.interrupted();
      const prefs = getCallSettings();
      if (owner && (!prefs.autoStop || prefs.mode === "off" || !prefs.providers.includes(owner.call.provider))) owner = null;
    };
    const idle = () => {
      const { state } = current.current;
      return !state.isRecording && !state.isStarting && !state.isStopping && !state.isProcessing && !state.isSaving;
    };
    const eligible = (call: CallWindow, version: number, mode: "prompt" | "auto") => {
      const prefs = getCallSettings();
      return !disposed && version === revision && seen.ownsObservation(call) &&
        prefs.mode === mode && prefs.providers.includes(call.provider) && idle();
    };
    const begin = async (call: CallWindow, version: number, mode: "prompt" | "auto") => {
      if (acting || !eligible(call, version, mode)) return;
      acting = true;
      let attempted = false;
      try {
        // Revalidate even when a user clicks a still-visible prompt or retries an earlier failure.
        const [scan, native] = await Promise.all([
          getCallDetectionStatus(getCallSettings().providers), recordingService.getRecordingState(),
        ]);
        if (native.is_recording || !scan.supported || !scan.complete || scan.active.length !== 1 ||
          scan.active[0].key !== call.key || !eligible(call, version, mode)) return;
        attempted = true;
        const { sessionId } = await current.current.start("detected_call");
        if (disposed) return;
        if (sessionId && version === revision && getCallSettings().autoStop) owner = { call, sessionId };
        toast.success(`Recording started for ${CALL_PROVIDERS[call.provider]}`);
      } catch (error) {
        if (disposed) return;
        if (isDiskSpaceRefusal(error)) {
          // A retry cannot help until space is freed, so no Retry action.
          toast.error("Not enough disk space", { description: startErrorText(error), duration: 15000 });
          return;
        }
        if (attempted) {
          const snapshot = await recordingService.getRecordingState().catch(() => null);
          const { state } = current.current;
          if (!disposed && !snapshot?.is_recording && !state.isRecording && !state.isStopping && !state.isProcessing && !state.isSaving) {
            current.current.state.setStatus(RecordingStatus.ERROR, "Could not start the detected call recording.");
          }
        }
        if (!disposed && version === revision) toast.error("Could not start the detected call recording.", {
          action: { label: "Retry", onClick: () => void begin(call, version, mode) },
        });
      } finally { acting = false; }
    };
    const tick = async () => {
      const prefs = getCallSettings();
      const version = revision;
      try {
        if (acting || prefs.mode === "off" || !prefs.providers.length) return;
        const scan = await getCallDetectionStatus(prefs.providers, seen.windows());
        if (disposed || revision !== version) return;
        if (!scan.supported || !scan.complete) { seen.interrupted(); dismiss(); return; }
        if (prompt && !scan.active.some(call => call.key === prompt?.key)) dismiss();
        const ended = seen.update(scan);
        if (owner && ended.includes(owner.call.key)) {
          const target = owner;
          owner = null;
          if (getCallSettings().autoStop) {
            try {
              if (await recordingService.stopDetectedCall(target.sessionId) && !disposed) toast.success("Call ended. Recording stopped.");
            } catch {
              if (!disposed && version === revision) {
                owner = target;
                seen.remember(target.call);
                toast.error("Could not finish the call recording. ClawScribe will retry. You can also stop it manually.");
              }
            }
          }
          return;
        }
        if (scan.active.length !== 1) return;
        const call = scan.active[0];
        if (seen.has(call)) return;
        // A manual recording observed during this call stays manual, including after the user stops it.
        if (!idle()) { if (current.current.state.isRecording) seen.remember(call); return; }
        seen.remember(call);
        if (prefs.mode === "auto") {
          await begin(call, version, "auto");
        } else {
          const id = toast(`${CALL_PROVIDERS[call.provider]} call detected`, {
            description: "Start recording this call?", duration: 20_000,
            action: { label: "Record", onClick: () => {
              if (prompt?.id === id) void begin(call, version, "prompt");
            } },
          });
          prompt = { id, key: call.key };
        }
      } catch { if (!disposed) { seen.interrupted(); dismiss(); } }
      finally { if (!disposed) timer = window.setTimeout(tick, POLL_INTERVAL_MS); }
    };
    window.addEventListener(CALL_SETTINGS_CHANGED, settingsChanged);
    window.addEventListener("storage", settingsChanged);
    void tick();
    return () => {
      disposed = true;
      window.clearTimeout(timer);
      window.removeEventListener(CALL_SETTINGS_CHANGED, settingsChanged);
      window.removeEventListener("storage", settingsChanged);
      dismiss();
    };
  }, []);
  return null;
}
