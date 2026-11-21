# iCalendar files and subscriptions

Open **Settings → Calendars → Add calendar**. Choose **Local file**
to import a UTF-8 `.ics` invitation or calendar export. ClawScribe keeps a copy
on this device. To pick up later edits to the original file, remove the
imported calendar and import the updated copy.

Choose **Subscription** for a feed that your calendar provider can update.
Enter a display name and the subscription link supplied by the provider.
ClawScribe reads HTTPS feeds and opens `webcal` links over HTTPS. Use the
calendar's subscription address, which may give anyone holding it access to
your calendar. The app protects the address with the operating system's
credential storage.

**Upcoming meetings** on Home shows the next five meetings across your
configured calendars for the coming week. The Calendars page lists more
occurrences. **Use for next recording** takes priority over automatic matching.
The selection expires five minutes after the event ends. Choose **Clear selection**
to remove it sooner. Invitations without an end time expire one hour after their
start, or one hour after selection if the start time is also missing.
Without a selection, capture starts first. ClawScribe then looks for a single
timed event that was in progress at the start across enabled Microsoft 365 and
iCalendar sources. For an early join, **Choose event** offers timed invitations
starting within the next five minutes. If several events are plausible, or a
source is unavailable, you choose which event to link. You can unlink a wrong match or use **Skip calendar
for next recording** on Home.

Open **Review attendance** while recording or **Attendance** in the saved
meeting. Invitees start unchecked. Automatic and manual summaries receive only
calendar names you confirm by checking. Review during recording to include
attendance in an automatic summary, or review after saving and regenerate.
Active edits survive a window reload and are included when an interrupted
recording is recovered in the same library. Saved links and attendance are
included in library backups.

Links and attendance are saved with the meeting and included in library backups.

An event that ClawScribe links automatically is local convenience. It can name
the meeting and prefill the invitee list, but its title and invitees are not sent
to a summary or knowledge provider. Saving **Review attendance** is your
confirmation: from then on summaries and knowledge answers can use the event
title, and summaries receive the names of checked people. Invitees shown only by
an email address cannot be named and are never sent. A title you edit
yourself is your own text and is not withheld.

A meeting without a link, or one you unlinked, shows **Link calendar event**.
It lists events from the calendars connected now that overlap the recording.
Imported calendars load only the coming week, so older recordings can be
relinked from Microsoft 365 only. If a source could not be read, the list names
it and offers **Check again**; the same action appears while recording when the
automatic match was partial. A meeting whose saved link cannot be read, for
example after restoring a damaged archive, appears unlinked and can be linked
again. An unlinked meeting's title stays out of provider context because it may
still be the former event's subject.
Older checked selections require review because earlier versions checked each
invitation by default.

In a saved meeting linked to a recurring iCalendar or Microsoft 365 event,
open **Attendance** and choose **Template for this series**. The choice becomes
the default for linked occurrences from that iCalendar source or Microsoft 365
account, including rescheduled occurrences. Choose **Use the default template**
to clear the preference. Changing a series template leaves attendance as it was.
Single events, detached iCalendar imports, and links without a stable series ID
use the regular summary template settings. Series preferences are stored locally.

ClawScribe checks for calendar updates while Home or the Calendars page is
open. **Refresh** requests an immediate update. Use the calendar's menu
to remove it and stop future refreshes. Saved meeting associations can still
be used after removing the source.

Files and subscriptions support all-day events, times with time zones,
and recurring meetings, including cancelled or rescheduled occurrences.
A calendar can be up to 16 MiB. ClawScribe does not keep event descriptions,
comments, or attachments. Large recurrence expansions and unsupported
time zones or recurrence formats produce an error beside the affected source.
