"use client";

import { useEffect, useState, useSyncExternalStore } from "react";
import Link from "next/link";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button } from "@/components/ui/button";
import { PageSection } from "@/components/ui/page-section";
import { Progress } from "@/components/ui/progress";
import { getBehaviorSettings, subscribeBehaviorSettings } from "@/lib/behaviorSettings";

interface ConversationSnapshot {
  session_id: string;
  revision: number;
  availability: "waiting_for_speech" | "labels_off" | "unclear_source" | "available";
  totals: { speech_seconds: number; words: number; questions: number };
  sides: {
    me_seconds: number;
    others_seconds: number;
    me_share: number;
    pace_wpm: number | null;
    recent_pace_wpm: number | null;
    me_questions: number;
    others_questions: number;
    current_monologue_seconds: number;
    longest_monologue_seconds: number;
  } | null;
}

function duration(seconds: number) {
  const whole = Math.floor(seconds);
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}

function pace(wpm: number | null) {
  return wpm === null ? "Unavailable" : `${wpm} wpm`;
}

export function LiveConversationMetrics({ sessionId }: { sessionId: string }) {
  const settings = useSyncExternalStore(subscribeBehaviorSettings, getBehaviorSettings, getBehaviorSettings);
  return settings.showConversationMetrics ? <ConversationReadout key={sessionId} sessionId={sessionId} /> : null;
}

export function ConversationReadout({ sessionId }: { sessionId: string }) {
  const [current, setCurrent] = useState<ConversationSnapshot | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let active = true;
    let revision = -1;
    const accept = (snapshot: ConversationSnapshot | null) => {
      if (!active || snapshot?.session_id !== sessionId || snapshot.revision <= revision) return;
      revision = snapshot.revision;
      setCurrent(snapshot);
      setFailed(false);
    };
    const subscription = listen<ConversationSnapshot>("live-conversation-metrics", event => accept(event.payload));
    void subscription.then(async () => {
      if (!active) return;
      const snapshot = await invoke<ConversationSnapshot | null>("get_live_conversation_metrics", { sessionId });
      if (active && revision < 0 && !snapshot) setFailed(true);
      accept(snapshot);
    }).catch(() => { if (active && revision < 0) setFailed(true); });
    return () => {
      active = false;
      void subscription.then(unlisten => unlisten()).catch(() => {});
    };
  }, [sessionId, attempt]);

  const sides = current?.sides;
  const me = sides ? Math.round(sides.me_share * 100) : 0;
  return (
    <PageSection aria-label="Conversation" className="space-y-3 p-4">
      <h2 className="text-sm font-semibold">Conversation</h2>
      {failed ? (
        <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <p role="status">Could not read conversation metrics.</p>
          <Button variant="outline" size="sm" onClick={() => { setFailed(false); setAttempt(value => value + 1); }}>Retry</Button>
        </div>
      ) : !current || current.availability === "waiting_for_speech" ? (
        <p role="status" className="text-xs text-muted-foreground">
          {current ? "Waiting for finalized speech…" : "Reading conversation metrics…"}
        </p>
      ) : sides ? (
        <>
          <div className="space-y-1.5">
            <div className="flex justify-between gap-2 text-xs font-medium tabular-nums" aria-hidden="true">
              <span>You {me}%</span>
              <span>Others {100 - me}%</span>
            </div>
            <Progress
              value={me}
              className="bg-primary/30 motion-reduce:[&>div]:transition-none"
              aria-label="Talk time"
              getValueLabel={() => `You ${me}%, others ${100 - me}%`}
            />
          </div>
          <dl className="grid grid-cols-3 gap-2 text-xs tabular-nums">
            <div>
              <dt className="font-medium">Your pace</dt>
              <dd>{pace(sides.pace_wpm)}</dd>
              <dd className="text-muted-foreground">Last 2 min · {pace(sides.recent_pace_wpm)}</dd>
            </div>
            <div>
              <dt className="font-medium">Questions</dt>
              <dd>You {sides.me_questions}</dd>
              <dd className="text-muted-foreground">Others {sides.others_questions}</dd>
            </div>
            <div>
              <dt className="font-medium">Your monologue</dt>
              <dd>Current {duration(sides.current_monologue_seconds)}</dd>
              <dd className="text-muted-foreground">Longest {duration(sides.longest_monologue_seconds)}</dd>
            </div>
          </dl>
        </>
      ) : (
        <>
          <dl className="flex flex-wrap gap-x-5 gap-y-1 text-xs tabular-nums">
            <div className="flex gap-1.5"><dt className="text-muted-foreground">Speech</dt><dd>{duration(current.totals.speech_seconds)}</dd></div>
            <div className="flex gap-1.5"><dt className="text-muted-foreground">Questions</dt><dd>{current.totals.questions}</dd></div>
            <div className="flex gap-1.5"><dt className="text-muted-foreground">Word estimate</dt><dd>{current.totals.words}</dd></div>
          </dl>
          {current.availability === "labels_off" ? (
            <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
              <p>Me and Participants labels are off.</p>
              <Button asChild variant="outline" size="sm">
                <Link href="/settings?tab=transcription">Open transcription settings</Link>
              </Button>
            </div>
          ) : (
            <p className="text-xs text-muted-foreground">Some speech could not be separated by source.</p>
          )}
        </>
      )}
    </PageSection>
  );
}
