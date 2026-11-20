"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { LANGUAGES } from "@/constants/languages";

interface SourceLanguages {
  session_id: string;
  revision: number;
  evidence: "whisper_audio" | "transcript_text";
  microphone: string | null;
  system: string | null;
}

const languageName = (code: string) => LANGUAGES.find(item => item.code === code)?.name ?? code;

export function LiveSourceLanguages({ sessionId }: { sessionId: string }) {
  const [current, setCurrent] = useState<SourceLanguages | null>(null);
  useEffect(() => {
    let active = true;
    let lastRevision = -1;
    function accept(snapshot: SourceLanguages | null) {
      if (!active || snapshot?.session_id !== sessionId || snapshot.revision <= lastRevision) return;
      lastRevision = snapshot.revision;
      setCurrent(snapshot);
    }
    const subscription = listen<SourceLanguages>("live-source-languages", event => accept(event.payload));
    // Subscribe first. A delayed initial read cannot undo a newer language switch.
    void subscription.then(async () => {
      if (active) accept(await invoke<SourceLanguages | null>("get_live_source_languages"));
    }).catch(() => {});
    return () => { active = false; void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, [sessionId]);
  if (current?.session_id !== sessionId || (!current.microphone && !current.system)) return null;
  return <p className="flex flex-wrap justify-center gap-x-4 text-sm text-muted-foreground" aria-live="polite">
    {current.evidence === "transcript_text" && <span>Estimated from transcript</span>}
    {current.microphone && <span>Microphone · {languageName(current.microphone)}</span>}
    {current.system && <span>System audio · {languageName(current.system)}</span>}
  </p>;
}
