"use client";

import { useEffect, useState } from "react";
import {
  getCloudTranscription,
  subscribeCloudTranscription,
} from "@/lib/cloudTranscription";

export function useCloudTranscription(): boolean {
  const [enabled, setEnabled] = useState(false);

  useEffect(() => {
    setEnabled(getCloudTranscription());
    return subscribeCloudTranscription(setEnabled);
  }, []);

  return enabled;
}
