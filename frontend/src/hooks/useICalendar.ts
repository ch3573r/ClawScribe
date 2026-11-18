"use client";

import { useEffect, useSyncExternalStore } from "react";
import {
  getICalendarServerSnapshot, getICalendarSnapshot, ICAL_REFRESH_MS,
  refreshICalendars, subscribeToICalendars,
} from "@/services/icalService";

const refresh = () => refreshICalendars(true);

export function useICalendar() {
  const state = useSyncExternalStore(subscribeToICalendars, getICalendarSnapshot, getICalendarServerSnapshot);
  useEffect(() => {
    const check = () => { void refreshICalendars(); };
    const visible = () => { if (document.visibilityState === "visible") check(); };
    check();
    const timer = window.setInterval(check, ICAL_REFRESH_MS);
    window.addEventListener("focus", check);
    document.addEventListener("visibilitychange", visible);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", check);
      document.removeEventListener("visibilitychange", visible);
    };
  }, []);
  return { ...state, refresh };
}
