"use client";

import { useEffect, useState } from "react";
import {
  behaviorSettingsLoaded,
  loadBehaviorSettings,
} from "@/lib/behaviorSettings";
import {
  getCloudTranscription,
  subscribeCloudTranscription,
} from "@/lib/cloudTranscription";

/**
 * The cloud transcription opt-in as the core holds it. It is off until the core
 * has answered and stays off when it cannot be read (`failed`).
 */
export function useCloudTranscription(): { enabled: boolean; loaded: boolean; failed: boolean } {
  const [state, setState] = useState({
    enabled: getCloudTranscription(),
    loaded: behaviorSettingsLoaded(),
    failed: false,
  });

  useEffect(() => {
    let active = true;
    const unsubscribe = subscribeCloudTranscription(enabled => {
      if (active) setState({ enabled, loaded: true, failed: false });
    });
    loadBehaviorSettings()
      .then(() => {
        if (active) setState({ enabled: getCloudTranscription(), loaded: true, failed: false });
      })
      .catch(() => {
        if (active) setState({ enabled: false, loaded: false, failed: true });
      });
    return () => {
      active = false;
      unsubscribe();
    };
  }, []);

  return state;
}
