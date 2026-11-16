import { useEffect } from "react";

// Memory only: reopening a meeting in this app session must still allow adding labels.
const shownMeetings = new Set<string>();

export function useSpeakerLabelVisibility(meetingId: string, hasAttribution: boolean): boolean {
  useEffect(() => {
    if (hasAttribution) shownMeetings.add(meetingId);
  }, [meetingId, hasAttribution]);
  return hasAttribution || shownMeetings.has(meetingId);
}
