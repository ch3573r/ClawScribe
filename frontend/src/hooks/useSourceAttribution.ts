"use client";

import { useEffect, useState } from "react";
import { loadBehaviorSettings } from "@/lib/behaviorSettings";
import {
  getSourceAttribution,
  subscribeSourceAttribution,
} from "@/lib/sourceAttribution";

export function useSourceAttribution(): boolean {
  const [enabled, setEnabled] = useState(getSourceAttribution());

  useEffect(() => {
    let active = true;
    const unsubscribe = subscribeSourceAttribution(value => {
      if (active) setEnabled(value);
    });
    loadBehaviorSettings()
      .then(() => {
        if (active) setEnabled(getSourceAttribution());
      })
      .catch(() => {
        if (active) setEnabled(false);
      });
    return () => {
      active = false;
      unsubscribe();
    };
  }, []);

  return enabled;
}
