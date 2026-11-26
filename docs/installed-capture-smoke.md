# Installed Windows dual-source capture

Run this check in the release owner's interactive Windows acceptance session.
It exercises the registered, current-user NSIS installation twice through
`--start-recording` and `--stop-recording`. Use the production release
artifacts from the designated build runner. Keep this interactive check out of
the runner service and GitHub Actions.

## Provision the test desktop once

1. Use a dedicated Windows test account with a synthetic ClawScribe library.
   Complete onboarding, turn off call detection and webhooks, and
   disconnect calendars and providers. For an upgrade check, start with the
   older installed version and at least one saved synthetic meeting.
2. In ClawScribe's recording settings, use the folder picker to choose an
   empty, local folder reserved for these recordings. This registers the
   folder as trusted. You can then select your previous save folder again.
   Leave the window on Home and **Quit** from the tray.
3. Provision an isolated virtual microphone with its own render endpoint for
   injecting sound. Provide a second, independent virtual render endpoint for
   system loopback. Check the driver and signal routing for all three in
   Windows Sound settings. Keep live speech, call and media apps, sound
   monitoring and notifications off the selected routes. From 0.64.1 the
   app opens each explicitly selected Windows device by its exact name.
   Other devices can remain connected. A missing or ambiguous selection
   fails the check. The harness leaves Windows audio settings in place.
4. Have 64-bit Python 3.11 or later, NumPy, PyAudioWPatch 0.2.12.8, Git, Windows
   PowerShell with UI Automation and 7-Zip available. Windows supplies the
   endpoint IDs. PyAudioWPatch plays the test signals and measures system loopback.
   The installed FFmpeg measures the selected virtual microphone independently.
   [PyAudioWPatch documentation](https://github.com/s0d3s/PyAudioWPatch)

From the repository root, enumerate device names and IDs locally. This command
opens no streams. Keep its output on the test desktop.

```powershell
python -B frontend/scripts/installed_capture/signals.py devices
```

Outside the checkout, create a private UTF-8 JSON configuration. Supply the
three exact IDs from the inventory and the absolute path chosen in step 2.
Set `isolated_virtual_routes` to true after checking the routing in step 3.

```json
{
  "isolated_virtual_routes": true,
  "microphone": "<virtual capture endpoint ID>",
  "microphone_feed": "<render endpoint ID wired only to the virtual microphone>",
  "system_output": "<independent virtual render endpoint ID>",
  "recordings_root": "<absolute path to the dedicated recordings folder>"
}
```

## Run the installed smoke

Place the verified NSIS installer and `BUILD-METADATA.txt` from the same build
in a local artifact directory. Use the expected full commit from the release
ref and the NSIS digest from the verified release assets or trusted build
output. The checkout must contain the release commit. Run the asset/signature
checks described in [Windows releases](windows-release.md) for staged releases.

```powershell
$version = '0.68.2-preview.1'
$assets = '<local artifact directory>'
$routes = '<private route JSON path>'
$commit = '<40-character release commit>'
$digest = '<verified 64-character NSIS SHA-256>'
$setup = Join-Path $assets "ClawScribe_${version}_x64-setup.exe"
$metadata = Join-Path $assets 'BUILD-METADATA.txt'
python -B frontend/scripts/installed_capture/run.py --install --upgrade-from 0.65.1 `
    --version $version --commit $commit --installer-sha256 $digest `
    --installer $setup --metadata $metadata --config $routes --stop-seconds 20
if ($LASTEXITCODE -ne 0) { throw 'Installed dual-source capture failed.' }
```

Use the actual artifact paths for a nested build bundle. Pass `--seven-zip`
with the full path to `7z.exe` if it is outside PATH. For a repeat on the
already-installed candidate, omit `--install --upgrade-from 0.65.1`.
`--install` runs the current-user installer silently and leaves that version
installed. `--upgrade-from` also checks the registered starting version and
the retention of all previously saved meeting IDs and folder bindings.

The harness probes each virtual route separately before installation or
recording. It measures the microphone pitch actually received and checks the
other route for leakage. It compares the installed app and FFmpeg bytes to the
verified NSIS payload, saves the prior recording preferences, selects audio-only
mode and launches the installed app. Windows UI Automation waits for its
enabled **Start recording** control.

Each CLI Start must create a fresh recording ID with both named devices. Each
Stop must publish a completed recording with a matching SQLite row within the
specified deadline, including time spent forwarding the command. Packaged
FFmpeg decodes the saved MP4 and the harness checks its duration and both
sustained markers in order. The second Start runs in the same app process with
a different marker pair.

Quit the app from its tray when prompted. After 30 seconds the harness ends its
own app process and records that fact. It reopens SQLite, removes only the
verified synthetic meeting IDs and their recognized files, and checks the
baseline meeting IDs and folder bindings again. It restores the original
recording preference and mode files byte for byte, including prior absence.

Exit code zero and the final JSON record report the release and `cpal`
commits, installer and installed-app hashes, calibrated marker frequencies,
decoded durations, Stop-to-save times, process exit and cleanup. This covers
short installed dual-source recordings. Track in-app player controls, physical
device reconnect, sleep/wake, updater discovery and long-meeting performance
separately.

## Recording controls and player preferences

Use the isolated test profile and a saved synthetic recording with playable
audio for these checks.

1. Start a recording with the default settings. Confirm the floating controls
   appear without taking keyboard focus. Close them, keep recording, and reopen
   them from the recording bar. Stop from the floating window and play the
   saved audio.
2. In **Settings → Recording → Capture**, turn off **Open floating controls
   when recording starts**. Restart the app and start a second recording from
   the tray or command line. Confirm the controls stay closed and can still
   be opened manually. Turn the switch back on and check the next start.
3. In **Settings → Shortcuts**, assign a different key to **Play or pause**.
   Restart the app and open the saved meeting. Check that the player help
   shows the new key, the new key operates playback, and the old key no
   longer does. Type the new key into transcript Find and confirm it enters
   text without changing playback. Reset the player shortcuts and verify
   that Space works again.

## Capture health and call ownership

Use the exact installed Windows GPU candidate on the designated machine.
Record the source commit, installer and installed executable SHA-256, active
`cpal` revision, Windows version, capture devices, model, language and observed
backend. Record each scenario as passed, failed or not run.

1. Play system audio while speaking into the microphone. Silence the output
   for at least 15 seconds, restore it, then repeat. Verify there are no
   silence alerts and the saved meeting shows quiet periods without a capture
   gap. Pause longer than 15 seconds and resume. Verify that paused time
   does not contribute to the silence timer.
2. Disconnect the selected output with the main window hidden. Verify the
   microphone continues. Restore the same output. Reload the main window
   and reopen the floating controls during a missing-source period. Verify
   both windows agree on the session, its warnings and Retry state. While
   the app checks the device, pause and resume, review the calendar event,
   and save a bookmark or note. After
   saving, play both sources around the interruption and inspect the gap
   and quiet-period notices. Repeat by disconnecting the microphone.
3. Move the floating panel onto a secondary display. Pause and resume it
   with a note draft present. Save the note and a bookmark, then check their
   times in the library. Reopen the controls in another recording, remove
   the secondary display and verify accessible placement. Repeat in light
   and dark appearance and with the position preference disabled.
4. Create a live backlog with sustained speech on the target notebook.
   Check live lag and free space, then stop from the floating window. Record
   time to stop capture, drain time, time to save, and time to the next
   successful Start. If speech remains after the 120-second drain and
   five-second cancellation grace, verify the incomplete outcome and playable
   audio. Retranscribe through the saved meeting using a local model.
   Repeat briefly with a hosted file provider configured and verify that
   capture and Stop make no transcription upload. Compare Whisper, Parakeet
   and Nemotron health labels against each engine's actual backend.
5. Save a live recording with no detected speech. Verify its transcript tab
   identifies the empty live result and that playback is available. Check an
   audio-only recording still offers **Transcribe now**.
6. Put Teams in front of the visible main window and type into a Teams input
   while **Prompt me to record** is enabled. Verify the prompt is visible
   and keystrokes stay in Teams until you deliberately click it. Confirm a
   desktop invitation with an unknown room. Check that an already-active
   call, a different identified room, multiple active calls, and an expired
   or canceled invitation cannot start the requested recording.
7. With automatic stopping enabled, move an ongoing Teams desktop call to
   another window and confirm capture continues. Then end the detected call and
   enter a second Teams call in another window. Verify the first save contains
   its own title and playback, then verify the second call gets its own
   prompt or session. A manual recording must survive either call ending.
   Repeat with a timed iCalendar invitation containing a folded Teams link
   in its description and no Microsoft sign-in. Verify the saved event
   title and that attendance remains unreviewed.

## Interrupted or failed checks

The harness refuses an occupied app session, an untrusted or nonempty
recordings root, unexpected endpoints, and unrecognized files during cleanup.
On failure it preserves ambiguous recordings. Inspect the dedicated test
folder and remove a confirmed synthetic meeting using the app's normal delete
flow. Retain the first failure in the release record.

If the harness itself was interrupted, first quit the test app. In the test
account's app data directory (`%APPDATA%\net.rismondo.openclaw.clawscribe`),
inspect `installed-capture-preferences`. A `ready` file means the backup of
`recording_preferences.json` and `recording_mode.json` finished. Restore each
backed-up file to the parent and restore absence for a file missing from that
completed backup. Verify the restoration before removing that backup directory.
A backup without `ready` predates any preference replacement by this run.

## Deterministic development checks

From the repository root, with Python 3.11+, NumPy and FFmpeg on PATH:

```powershell
python -B frontend/scripts/installed_capture/test_capture.py -v
node scripts/verify-public-repo-safety.mjs
```

These checks use generated PCM/AAC, the repository's SQLite migrations and
temporary directories. They exercise release identity, Stop deadlines, shifted
pitch, missing or stale sources, cross-feed, cleanup ownership and preference
restoration. They can run without PyAudioWPatch, an installed app or audio devices.
