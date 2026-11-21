"use client";

import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import { reportStartFailure, START_BUSY, useStartRecordingSession } from "@/hooks/useRecordingStart";
import {
  confirmCallPrompt, finishStartRequest, getCoreCallSettings, getPendingCallPrompt,
  getPendingStartRequest, setCoreCallSettings,
  type CallNotice, type CallPrompt, type StartRequest, type StartStatus,
} from "@/services/callDetectionService";
import { isDiskSpaceRefusal } from "@/lib/start-error";
import { CALL_PROVIDERS, getCallSettings, setCallSettings, type CallSettings } from "@/lib/autoRecord";

const PROMPT_DURATION_MS = 20_000;

const sameSettings = (a: CallSettings, b: CallSettings) =>
  a.mode === b.mode && a.autoStop === b.autoStop && a.providers.join() === b.providers.join();

/**
 * The core owns the call monitor and answers the tray, the shortcuts and the
 * command line. It cannot choose audio devices or a calendar title, so this
 * window carries out the starts it asks for, in the background: no navigation,
 * so the page and its drafts stay as they are, and no focus change.
 *
 * It also shows the call prompt and the call notices the core sends, and keeps
 * the core's call detection settings in line with the ones saved here.
 */
export function RecordingRequests({ ready }: { ready: boolean }) {
  const state = useRecordingState();
  const start = useStartRecordingSession();
  const current = useRef({ state, start, ready });
  current.current = { state, start, ready };
  const afterSave = useRef<(() => void) | null>(null);

  // The native commit and next request can arrive in one render. Let the
  // stopped event update the shared state before trying the newly issued ID.
  useEffect(() => {
    if (!state.isRecording && !state.isStarting && !state.isStopping && !state.isProcessing && !state.isSaving) {
      const resume = afterSave.current;
      afterSave.current = null;
      resume?.();
    }
  }, [state.isRecording, state.isStarting, state.isStopping, state.isProcessing, state.isSaving]);

  useEffect(() => {
    let disposed = false;
    const handled = new Set<string>();
    let prompt: { id: string | number; key: string } | undefined;

    const dismissPrompt = () => {
      if (prompt) toast.dismiss(prompt.id);
      prompt = undefined;
    };
    const showPrompt = (call: CallPrompt) => {
      if (disposed || !current.current.ready) return;
      dismissPrompt();
      const label = CALL_PROVIDERS[call.provider];
      const answer = () => {
        if (prompt?.key !== call.key) return;
        void confirmCallPrompt(call.key).catch(() => toast.error("Could not start the recording."));
      };
      const id = toast(call.retry ? `Could not record the ${label} call` : `${label} call detected`, {
        description: call.retry ? "Try again?" : "Start recording this call?",
        duration: PROMPT_DURATION_MS,
        action: { label: call.retry ? "Try again" : "Record", onClick: answer },
      });
      prompt = { id, key: call.key };
    };
    const showNotice = (notice: CallNotice) => {
      if (disposed) return;
      if (notice.kind === "started") toast.success(`Recording started for ${CALL_PROVIDERS[notice.provider]}`);
      else if (notice.kind === "stopped") toast.success("Call ended. Recording stopped.");
      else toast.error("Could not finish the call recording. ClawScribe will retry. You can also stop it manually.");
    };

    const busy = () => {
      const { state } = current.current;
      return state.isRecording || state.isStarting || state.isStopping || state.isProcessing || state.isSaving;
    };
    const run = async (request: StartRequest) => {
      if (disposed || handled.has(request.requestId)) return;
      handled.add(request.requestId);
      if (handled.size > 32) handled.delete(handled.values().next().value!);
      const answer = (status: StartStatus) => finishStartRequest(request.requestId, status).catch(() => false);
      if (!current.current.ready) { await answer("refused"); return; }
      if (busy()) {
        if (request.source === "after_save" && !current.current.state.isStarting) {
          afterSave.current = () => {
            handled.delete(request.requestId);
            void getPendingStartRequest().then(pending => {
              if (pending?.requestId === request.requestId) void run(pending);
            }).catch(() => {});
          };
        } else {
          await answer("busy");
        }
        return;
      }
      try {
        await current.current.start(request.source === "call" ? "detected_call" : "remote_request", request.requestId);
      } catch (error) {
        if (!disposed) reportStartFailure(error);
        // An early webview refusal never entered the native start command.
        await answer(error === START_BUSY ? "busy" : isDiskSpaceRefusal(error) ? "refused" : "failed");
      }
    };

    const listeners = [
      listen<StartRequest>("recording-start-requested", event => { void run(event.payload); }),
      listen<string>("recording-start-feedback", event => { if (!disposed) toast(event.payload, { id: "recording-start-feedback" }); }),
      listen<CallPrompt>("call-detection-prompt", event => showPrompt(event.payload)),
      listen("call-detection-prompt-cleared", dismissPrompt),
      listen<CallNotice>("call-detection-notice", event => showNotice(event.payload)),
    ];
    const unlisten: Array<() => void> = [];
    const registered = Promise.all(listeners.map(subscription => subscription.then(fn => {
      if (disposed) fn(); else unlisten.push(fn);
    })));
    // Anything that arrived before this window was listening is still waiting in the core.
    void registered.then(async () => {
      const [request, call] = await Promise.all([
        getPendingStartRequest().catch(() => null), getPendingCallPrompt().catch(() => null),
      ]);
      if (request) void run(request);
      if (call) showPrompt(call);
    }).catch(() => {});

    return () => {
      disposed = true;
      afterSave.current = null;
      dismissPrompt();
      unlisten.forEach(fn => fn());
    };
  }, []);

  useEffect(() => {
    if (!ready) return;
    let disposed = false;
    void (async () => {
      const core = await getCoreCallSettings();
      if (disposed) return;
      const local = getCallSettings();
      if (!core) await setCoreCallSettings(local);
      else if (!sameSettings(core, local)) setCallSettings(core);
    })().catch(() => {});
    return () => { disposed = true; };
  }, [ready]);
  return null;
}
