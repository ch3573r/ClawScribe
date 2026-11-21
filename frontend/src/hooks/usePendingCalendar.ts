"use client";

import { useEffect, useState } from "react";
import { getNextCalendarChoice, PENDING_CALENDAR_CHANGED } from "@/lib/meetingCalendar";

export function useNextCalendarChoice() {
  const [choice, setChoice] = useState(getNextCalendarChoice);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const sync = () => {
      clearTimeout(timer);
      const next = getNextCalendarChoice();
      setChoice(next);
      if (next.expiresAt) timer = setTimeout(sync, Math.min(2_147_483_647, Math.max(1, next.expiresAt - Date.now())));
    };
    window.addEventListener(PENDING_CALENDAR_CHANGED, sync);
    sync();
    return () => {
      clearTimeout(timer);
      window.removeEventListener(PENDING_CALENDAR_CHANGED, sync);
    };
  }, []);
  return choice;
}

export function usePendingCalendar() {
  return useNextCalendarChoice().link;
}
