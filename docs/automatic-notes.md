# Automatic notes

ClawScribe can generate notes in the background after a recording is saved.
Enable **Summarize automatically** in **Settings → Summary**. Built-in AI and
Ollama on this PC can use the local preference. Older local preferences are
carried over once when the app first loads the setting.

Remote Ollama, OpenAI, OpenAI-compatible endpoints, OpenClaw, OpenRouter, Claude,
and Groq require you to review the provider, model, and destination before
ClawScribe sends transcript text and notes context. The review identifies
unencrypted HTTP connections. Changes to the selected destination, model, or
provider require another review. Codex and Claude Code manage their own
connections and are available when you generate notes from a saved meeting.

In the audio import dialog, **Generate a summary after transcription** applies
only to that import and uses the destination shown in the dialog. An approved
automatic notes setting selects the option by default. You can clear it for
the file or select it even when automatic notes are off. ClawScribe captures
the selected provider and destination when you begin the import.

Open the saved meeting to check progress or see why automatic notes were skipped.
Capture gaps stay visible in the meeting. Notes can cover the available speech
when the saved transcript is usable. A failed audio save or incomplete
transcription pauses automatic notes until repair. You can also review the
transcript and select **Generate summary** in the meeting.

After you transcribe a recorded meeting, including an audio-only recording,
ClawScribe checks the automatic setting and destination approval again. If the
recording-folder update fails, the transcript is saved in the library. Choose
**Retry file update** to finish publication. ClawScribe keeps existing notes.
If ClawScribe closes just after saving the recording files, the same profile
checks the transcript and current destination approval at the next startup.
Meeting backups exclude this pending check.

ClawScribe can retry an interrupted background job once on
startup when automatic notes are still enabled and the same destination is
approved.

Summary generation uses a snapshot of the saved transcript. Correcting or
replacing a passage while generation is in progress prevents publication of
outdated results. Renaming or changing the meeting date does not invalidate
notes. If a provider fails after generating some excerpts, the meeting shows
the available partial notes while they still match the saved transcript.

The summary shows its recorded model and template. Editing the notes preserves
that information. **Transcript changed since generation** means a saved passage,
speaker label, or timestamp has changed since the displayed notes were generated.
Regenerating the notes updates their revision. Older notes may lack revision or
model information. **Follow-up email** shows the draft when the provider included
one in its original response.
