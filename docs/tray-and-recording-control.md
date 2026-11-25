# Recording from the tray, shortcuts and command line

The tray icon, the global shortcuts and the command-line flags all control the
same recording. Starting a recording this way does not change what you are
doing in ClawScribe.

## Starting

- The window keeps the page it is on and any text you have not saved. It is not
  reloaded and is not brought to the front. If it is hidden in the tray, it
  stays hidden.
- ClawScribe starts with the same saved device preferences and recording mode
  as the Start button. Choose the microphone and system output in Home's
  **Signal path** or **Settings → Recording**. Both use the same preferences
  and apply changes to the next recording. The selected calendar event is saved
  with the recording. Automatic calendar matching runs after capture starts.
- Only one start can be under way. A repeated shortcut, a second command-line
  start or a detected call that arrives at the same moment does not start a
  second recording. During a save, **Start after saving** in the window or tray
  menu, or an explicit `--start-recording`, requests one new recording after
  the library commit. The request expires after one minute. A subsequent
  `--stop-recording` during the save cancels it. A toggle during the save
  explains how to request the next recording. A detected call needs current
  evidence before ClawScribe opens the audio streams.
- If the start fails, for example with a full disk or a missing microphone, the
  window is brought forward and says why.
- Stopping from the tray menu, a shortcut or the command line saves the
  recording through the same flow as the Stop button. If the stop fails, the
  window is brought forward and says why.
- `--pause-recording` pauses or resumes the current recording through the same
  path as the tray menu. `--bookmark` adds a bookmark at the current position,
  like the bookmark shortcut. Both act on the running instance without
  bringing its window forward.

## Floating recording controls

The floating controls open when recording starts and stay above other apps
without taking keyboard focus. This includes audio-only recordings and starts
from call detection, the tray, a shortcut or the command line.

To keep the controls closed at startup, turn off **Open floating controls when
recording starts** in **Settings → Recording**. Changes apply to the next
recording. With either setting, you can choose **Open floating controls** on
the recording bar or tray.

- The clock shows captured time and freezes during pauses. **Microphone** and
  **System audio** show levels from the running capture.
- **Pause** and **Resume** control the current session. Capture warnings stay
  visible in the panel. Retry a disconnected audio source from its warning.
- The panel first opens in the bottom-right corner of the current display's
  work area. Drag the bar from an area without a button. The panel remembers
  where you moved it. If that display is disconnected, the next panel opens on
  a connected display. Turn off **Remember floating controls position** in
  **Settings → Recording** to use the default placement.
- **Open main window** brings the ClawScribe window forward during a
  recording. The panel shows whether the recording is **Live** or
  **Audio only**, and warns when the microphone has been silent for 10 seconds.
  While the panel is open, the recording bar inside the main window is hidden.
- Choose **Add note** or press N while the panel has focus. Type a note or leave
  the field empty to bookmark the current time, then press Enter. Escape closes
  the field. Find both in the saved meeting. Tab moves between the controls.
- **Stop** shows the current save stage. The window closes when saving finishes,
  including when no speech was transcribed. If the recording needs recovery,
  choose **Open ClawScribe** to review it in the main app or **Dismiss** to close
  the panel.
- Use **Close controls** or Alt+F4 to close the window while recording continues.
  It stays closed for that recording until you reopen it. You can also reopen
  it from the tray during a save.

The window omits the meeting title and transcript. ClawScribe asks Windows to
exclude the controls from capture. The result depends on the Windows version and
the screen-sharing tool. Check the sharing preview before typing a sensitive note.
[Windows capture protection](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity)

The controls use a separate bundled page and a private WebView2 profile on
Windows. The core supplies the selected theme and accent. The main window continues to own
call prompts, recording starts and post-processing. The controls webview has
no Tauri plugin permissions. Its app commands can read a small status snapshot,
add a note, pause, resume, retry a source, stop its own recording, move, resize
or close the controls, and bring the main window forward for recovery review.
The native webview
label binds those commands to the original session, including after a reload.

## What the tray shows

The window shows the current Stop stage during capture shutdown, transcription
drain and the library save. Separate capture warnings stay visible until each
condition clears or the recording ends. If microphone samples stop arriving
for 15 seconds after previously arriving, check the microphone connection.
Quiet samples still count as a working stream. A newly started or resumed
meter continues to flag a microphone with no audible signal after 10 seconds.

Call detection shows an actionable panel above other windows. You can also
answer it from the tray. Recording problems can show a Windows notification
while the main window is hidden or minimized.

| State | Icon | Tooltip |
| --- | --- | --- |
| Idle | ClawScribe icon | ClawScribe |
| Starting | ClawScribe icon | Starting recording |
| Recording | Red dot | Recording, minutes so far, and the capture name |
| Paused | Amber dot with two bars | Paused after the minutes recorded, and the capture name |
| Saving | ClawScribe icon | Saving recording |

The time is the recorded time without pauses, in whole minutes, and updates once
a minute. The title is shortened to one line. When a detected call is waiting
for your answer, the idle tooltip names the app and the tray menu offers
**Record this ... call**.

## Keeping the PC awake

While audio is being captured, and while a recording is being saved, ClawScribe
asks Windows not to go to sleep on idle. Without it, a PC left idle during a
meeting could suspend in the middle of the recording.

- Only sleep is held back. The screen can still turn off.
- Nothing is held while a recording is paused, and nothing when idle.
- The request is released when the recording ends or fails, and when
  ClawScribe exits. It is also limited to 12 hours at a time.
- If the PC loses power or ClawScribe is killed, the request ends with the
  process. The partial recording is found again the next time ClawScribe
  starts, as for any interrupted recording.

`powercfg /requests` lists the request under ClawScribe while it is held.

## Windows sleep, sign-out and shutdown

When Windows reports that it is going to sleep, ClawScribe asks its recorder to
save the current session, including a paused session. Windows can sleep while
the save is pending. The save can continue after wake, and the old session
does not restart automatically. Sleep during active capture flags a possible
gap in the saved audio. If an audio driver prevents Stop from completing,
ClawScribe asks you to retry Stop and keeps the available audio on disk.
You can generate notes from the saved meeting. A wake notice reopens recording
admission. It cannot identify which recording was running if Windows did not
deliver a sleep notice.

When Windows asks to sign out or shut down, ClawScribe requests a transcript
recovery snapshot and acknowledges the request immediately. A canceled sign-out
can leave the recording running. Windows may close the app before the snapshot
or the latest audio buffers reach disk. The audio already written to the
recovery spool remains available through **Interrupted recordings**. The same
recovery applies if the process ends between notifications or without warning.
If the recording had audio saving disabled, recovery depends on the transcript
snapshots that reached disk.

### Implementation and synthetic checks

The locked Tao 0.35.2 Windows event loop
(`src/platform_impl/windows/event_loop.rs`) ignores `WM_QUERYENDSESSION` and
dispatches loop destruction on a confirmed `WM_ENDSESSION`. Tauri Runtime Wry
2.11.1 (`src/lib.rs`, `handle_event_loop`) maps that to `RunEvent::Exit`,
bypassing the normal `ExitRequested` recording gate. It maps `RunEvent::Resumed`
from `NewEvents(StartCause::Poll)`. The pinned Windows event loop has no
`WM_POWERBROADCAST` handler for Windows suspend and wake.

ClawScribe listens on its own top-level window. The callback queues work and
returns before the worker requests any checkpoint or Stop. System session end
skips ClawScribe's synchronous normal-exit cleanup. The Windows shutdown metric
covers delivery to Tao's separate window ahead of ClawScribe's window.

On the designated runner, `cargo test -p clawscribe --lib windows_lifecycle::tests`
exercises the message mappings, start admission, stale work and duplicate
signals with synthetic messages.

## Windows smoke checks

On the designated self-hosted machine with audio devices:

1. With the window on Settings and text typed into a field, start a recording
   from the tray, then from the shortcut, then with `--start-recording`. The
   page and the text stay; the window does not come to the front or move.
2. Start with the window hidden in the tray. The icon turns red and the tooltip
   shows the time. Pause, then resume: amber, then red again.
3. Press the start shortcut twice quickly, and run `--start-recording` twice.
   One recording starts.
4. While recording, run `powercfg /requests`: the system request is listed.
   Pause: it is gone. Resume: it is back. Stop: it is gone.
5. Start a recording, then close ClawScribe from the tray. It stops and saves
   first, and `powercfg /requests` no longer lists it.
6. Start a recording and end the process from Task Manager. Restart ClawScribe:
   the recording is offered for recovery.
7. Start with automatic floating controls enabled. Check both meters, pause from the
   tray and verify the clock freezes. Use Tab and Enter to add a note and a
   bookmark. Close the controls with Alt+F4, reopen from the tray, then stop.
   Verify the save progress, playback and note times in the saved meeting.
8. With another app focused, start a second recording from a shortcut. The
   controls should open again while keystrokes still go to the other app.
   Turn off automatic opening, stop from the tray and verify that the controls
   close after saving. Restart ClawScribe, start another recording and confirm
   that the controls stay closed until opened explicitly. Check light and dark
   appearance and how the controls appear in a sharing preview.
