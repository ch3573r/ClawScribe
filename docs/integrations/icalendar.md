# iCalendar files and subscriptions

Open **Settings → Add-ons → iCalendar → Add calendar**. Choose **Local file**
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
configured calendars for the coming week. The iCalendar panel lists more
occurrences and lets you review invitees. Choose **Use for next recording**
to use the event title and attendee selection. An invitation records who was
invited. You can edit the attendee checklist before generating a summary.

ClawScribe checks for calendar updates while Home or the iCalendar settings
are open. **Refresh** requests an immediate update. Use the calendar's menu
to remove it and stop future refreshes. Saved meeting associations can still
be used after removing the source.

Files and subscriptions support all-day events, times with time zones,
and recurring meetings, including cancelled or rescheduled occurrences.
A calendar can be up to 16 MiB. ClawScribe does not keep event descriptions,
comments, or attachments. Large recurrence expansions and unsupported
time zones or recurrence formats produce an error beside the affected source.
