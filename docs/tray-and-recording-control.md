# Recording from the tray, shortcuts and command line

The tray icon, the global shortcuts and the command-line flags all control the
same recording. Starting a recording this way does not change what you are
doing in ClawScribe.

## Starting

- The window keeps the page it is on and any text you have not saved. It is not
  reloaded and is not brought to the front. If it is hidden in the tray, it
  stays hidden.
- ClawScribe starts with the same saved device preferences and recording mode
  as the Start button. The selected calendar event is saved with the recording.
  Automatic calendar matching runs after capture starts.
- Only one start can be under way. A repeated shortcut, a second command-line
  start or a detected call that arrives at the same moment does not start a
  second recording. A start while a recording is running or being saved is
  ignored. An unanswered request expires. A detected call also needs current
  evidence before ClawScribe opens the audio streams.
- If the start fails, for example with a full disk or a missing microphone, the
  window is brought forward and says why.
- Stopping from the tray menu, a shortcut or the command line saves the
  recording through the same flow as the Stop button. If the stop fails, the
  window is brought forward and says why.

## What the tray shows

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
