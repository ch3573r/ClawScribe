# Detecting calls on Windows

In **Settings → Recording → Call detection**, choose the meeting apps to
watch and select **Prompt me to record** or **Auto-start recording**. Use
**Check now** during a call to see which selected apps the detector
recognizes. Detection starts off on a new installation. An existing Teams
preference applies only to Teams until you select more apps. Automatic stopping
has its own switch and starts off.

| Meeting app | Evidence used |
| --- | --- |
| Microsoft Teams | Teams desktop process or a Teams website in a supported browser, with active call controls. |
| Zoom | Zoom desktop process with a meeting-specific exit control, or a Zoom meeting window with its Leave or End button. |
| Webex | Webex desktop process with a leave- or end-meeting button. |
| Google Meet | A supported browser showing a Meet room URL with its leave-call button. |

ClawScribe uses Windows UI Automation locally. Browser detection checks the
address control in the browser interface before inspecting in-call controls.
Chrome, Edge, Brave and Firefox are eligible when their accessibility trees
expose these controls. Button matching includes English and German labels.
App updates, other interface languages, full-screen browser modes, or hidden
toolbars can prevent a match. The check reports provider names. It does not
save browser addresses or inspect the values of web forms.

While detection is enabled, ClawScribe checks about every 12 seconds. The
checks run in the application core, not in the window, so they keep their
schedule while the window is hidden in the tray. It requires a single
identified call before starting a recording and checks the evidence again when
you act on a prompt. Recording uses the selected audio devices, recording mode
and pending calendar title. A calendar selection is optional. Detection
requires no Microsoft sign-in.

With **Prompt me to record**, the question appears in the window and, while it
is waiting, as **Record this ... call** in the tray menu, so it can be answered
without opening the window. It stays until the call ends, you answer, or you
change the detection settings. If an automatic start fails, the same prompt
returns as **Try again**, except for a full disk, where trying again cannot
help.

The settings are kept by the core as well as the window. The first time, the
ones saved by earlier versions are handed to the core, which then keeps its own.

## Joining a Teams invitation from Upcoming

On Home, **Join** opens the meeting link. **Join and record** opens a supported
Teams link from Microsoft 365 and watches for that invitation, including when
call detection is off. The request lasts five minutes or until the event ends,
whichever comes first. Use **Cancel recording request** on Home to withdraw it.
Microsoft sign-out or starting a separate recording also clears the request.

ClawScribe checks for active Teams calls before opening the link. An already
active call window is ineligible. A new browser call with the same Teams room
can start automatically when the invitation is in progress or starts within
five minutes. If the meeting is further away or Teams only exposes call
controls, confirm the event shown on Home, in the recording prompt or in the
tray. A different identified room or several detected calls cannot start the
requested recording. If the initial check cannot finish, Teams still opens
and ClawScribe offers manual recording.

The native recorder rechecks the call and saves the selected invitation with
that session before opening the audio streams. It uses the saved audio devices
and recording mode. A separate **Use for next recording** choice stays available
for an ordinary start. Invitees remain unchecked until you review attendance.
With continuous Teams detection off, stop the recording yourself. The usual
automatic stop applies when continuous Teams detection and automatic stop are on.

## When the call ends

**Stop and save when the detected call ends** applies to a recording started
automatically or through the detector's Record prompt while that switch was
on. After three consecutive completed checks confirm the tracked meeting
window closed or shows a rejoin control, ClawScribe runs its usual stop and
save flow. An active call in the same provider postpones that decision.

A hidden browser tab or missing toolbar is ambiguous. If the browser window
stays open on a different tab after a call ends, ClawScribe may keep recording
until you stop it manually. An unavailable or locked desktop and a failed
check reset the count. You can stop any recording yourself. Manually started
recordings stay under manual control. Switching detection or automatic
stopping off, deselecting the source, or reloading the application relinquishes
automatic stopping for the current recording.

## Implementation and diagnostics

`get_call_detection_status` checks the selected providers off the UI thread.
It returns opaque call/window identifiers, in-call and ended observations,
and whether the scan completed. The core's monitor
(`teams_detection/monitor.rs`) uses the same scan, decides, and counts three
completed checks before a call is over. The window carries out the start,
because it knows the chosen devices and the calendar title, and reports the
session ID it got. The monitor binds the detected start to that session ID and
stops it itself when the call ends. The recorder checks both the detector
ownership and the ID under its stop guard before stopping capture. The usual
post-processing listener saves the stopped meeting on any page.

**Settings → Diagnostics → Teams detection status** remains available for
troubleshooting Teams window-title and process signals. Its compatibility
commands are `get_teams_detection_config` and `get_teams_detection_status`.
**Check for a call** uses the actual automation detector.

## Windows smoke checks

On the designated self-hosted machine, try a pre-join screen and an active call
for each selected provider. Check that prompt and automatic modes record once,
including from another ClawScribe page and with the window closed to the tray.
In prompt mode, answer once from the tray menu and once from the window. Check that switching browser tabs,
locking the desktop, and hiding call controls keep capture running. Leave
the call with automatic stopping on, then verify the saved microphone and
system audio, transcript, and a second recording. Repeat with a manually
started recording and with automatic stopping disabled.
