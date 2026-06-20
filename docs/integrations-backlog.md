# Integrations backlog

Candidate exports/services beyond the current OneNote / Planner / Calendar
(Microsoft 365) and Confluence (Atlassian). Ranked by value vs. effort. Each new
service is a Graph **scope** — gate behind the user enabling it in
Settings → Add-ons, and keep least-privilege.

## Microsoft 365 — worth it
- [ ] **Microsoft To Do** — personal action items (counterpart to Planner's team
  tasks). Uses the **`Tasks.ReadWrite` scope already requested** (near-zero new
  consent); just a different endpoint (`/me/todo/...`). Another Export-menu item.
  *Lowest-cost, highest-fit — do first.*
- [ ] **Email recap to attendees (Outlook `Mail.Send`)** — attendees are already
  captured from Calendar, so "send summary + action items to everyone invited" is
  one click. New `Mail.Send` scope. *Highest user value.*

## Microsoft 365 — more effort / heavier consent
- [ ] **OneDrive / SharePoint file export** — notes/transcript as `.docx` / `.md`
  (optionally PDF) with a shareable link. Re-adds `Files.ReadWrite` (trimmed in the
  least-privilege pass).
- [ ] **Post recap to a Teams channel/chat** — for Teams meetings, post the summary
  back. Heavier consent (`ChannelMessage.Send` / `Chat.ReadWrite`), fiddlier API.

## Skip / low priority
- Loop components, calendar follow-up events (`Calendars.ReadWrite`), Viva/Bookings
  — niche or API-immature.

## Notes
- Every addition = a new consent scope **except To Do** (covered by
  `Tasks.ReadWrite`). Gate each behind explicit enablement.
- Patterns already in place: To Do / Files = another Export-menu item;
  email / Teams = a new "Send recap" action.
