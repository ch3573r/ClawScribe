# Integrations backlog

ClawScribe supports Microsoft calendar lookup, OneNote, Planner, To Do,
OneDrive or SharePoint file export, and Confluence drafts. Additional Graph
permissions require a user-facing reason and a consent review.

## Microsoft 365 — shelved / not next
- [ ] **Email recap to attendees (Outlook `Mail.Send`)** — shelved for now.
  Attendees are already captured from Calendar, but "send summary + action items
  to everyone invited" creates a new `Mail.Send` consent surface and outbound
  communication risk. Do not bundle this with file export scope work.

## Microsoft 365 — more effort / heavier consent
- [ ] **Post recap to a Teams channel/chat** — for Teams meetings, post the summary
  back. Heavier consent (`ChannelMessage.Send` / `Chat.ReadWrite`), fiddlier API.

## Skip / low priority
- Loop components, calendar follow-up events (`Calendars.ReadWrite`), Viva/Bookings
  — niche or API-immature.

## Shipped
- [x] **OneDrive / SharePoint file export** — export reviewed meeting content as Word or PDF to a selected destination using `Files.ReadWrite`.
- [x] **Microsoft To Do** — personal action items (counterpart to Planner's team
  tasks). Uses the existing `Tasks.ReadWrite` scope and the `/me/todo/...`
  endpoints.

## Notes
- Every remaining addition = a new consent scope. Gate each behind explicit
  enablement.
- Email and Teams posting would each need a separate consent review.
