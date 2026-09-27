"use client";

import { useEffect, useState } from "react";
import {
  getCloudTranscription,
  subscribeCloudTranscription,
} from "@/lib/cloudTranscription";

export function useCloudTranscription(): { enabled: boolean; loaded: boolean } {
  const [state, setState] = useState({ enabled: false, loaded: false });

  useEffect(() => {
    setState({ enabled: getCloudTranscription(), loaded: true });
    return subscribeCloudTranscription(enabled => setState({ enabled, loaded: true }));
  }, []);

  return state;
}
