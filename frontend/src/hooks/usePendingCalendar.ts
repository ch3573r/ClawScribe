"use client";

import { useEffect, useState } from "react";
import { getPendingCalendar, type MeetingCalendarLink } from "@/lib/meetingCalendar";

export function usePendingCalendar() {
  const [selection, setSelection] = useState<MeetingCalendarLink | null>(null);
  useEffect(() => {
    const sync = () => setSelection(getPendingCalendar());
    window.addEventListener("clawscribe-pending-calendar-changed", sync);
    sync();
    return () => window.removeEventListener("clawscribe-pending-calendar-changed", sync);
  }, []);
  return selection;
}
