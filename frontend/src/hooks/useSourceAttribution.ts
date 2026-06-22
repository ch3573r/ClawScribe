"use client";

import { useEffect, useState } from "react";
import {
  getSourceAttribution,
  subscribeSourceAttribution,
} from "@/lib/sourceAttribution";

export function useSourceAttribution(): boolean {
  const [enabled, setEnabled] = useState(false);

  useEffect(() => {
    setEnabled(getSourceAttribution());
    return subscribeSourceAttribution(setEnabled);
  }, []);

  return enabled;
}
