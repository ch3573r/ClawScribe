# Automatic notes

ClawScribe can generate notes in the background after a recording is saved.
Enable **Summarize automatically** in **Settings → Summary**. Built-in AI and
Ollama on this PC can use the local preference. Older local preferences are
carried over once when the app first loads the setting.

Remote Ollama, OpenAI, OpenAI-compatible endpoints, OpenRouter, Claude,
and Groq require you to review the provider, model, and destination before
ClawScribe sends transcript text and notes context. The review identifies
unencrypted HTTP connections. Changes to the selected destination, model, or
provider require another review. Earlier external approvals must be renewed before
automatic notes can include the local meeting date and weekday, duration when
known, and title when permitted. Built-in AI and loopback Ollama keep their local
preferences. Use Codex or Claude Code from a saved meeting.
The current Codex protocol exposes a workspace display name, and Claude Code
checks sign-in separately from the generation process. ClawScribe needs a
verifiable account binding at dispatch before either can run automatic notes.

During setup, downloading the Built-in AI notes model prepares manual notes.
Turn on **Generate notes automatically after transcription** to enable background
notes on that PC. You can stop model setup and use **Summary** settings later.

In the audio import dialog, **Generate a summary after transcription** applies
only to that import and uses the destination shown in the dialog. An approved
automatic notes setting selects the option by default. You can clear it for
the file or select it even when automatic notes are off. ClawScribe captures
the selected provider and destination when you begin the import. If an older
import approval no longer covers the external request, transcription can still
finish. Open the saved meeting to generate its summary manually.

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
When you restore a meeting from Trash, ClawScribe also checks its pending
retranscription notes and tries to resume eligible automatic notes for that
meeting. Meeting backups exclude the pending retranscription check.

ClawScribe can retry an interrupted background job once on startup when
automatic notes are still enabled and the same destination is approved. A
recording or transcription can pause automatic Built-in AI notes. They resume
when local inference is available, including after restarting ClawScribe when
the original approval is still valid. **Stop** cancels the current job, including
a paused job. If approval changes, or you restore a meeting with paused notes
from a backup, open the meeting and select **Retry summary** to generate notes
manually.

Summary generation uses a snapshot of the saved transcript. The prompt includes
the displayed local meeting date and weekday, measured duration when available,
and the title allowed by calendar privacy rules. Review the date on an import
before generating notes that depend on relative dates such as “next Tuesday.”
Correcting or replacing a passage during generation prevents publication of an
outdated result. The saved summary remains readable while its replacement runs.
If a provider fails after generating some excerpts, the meeting shows the
available partial notes while they still match the saved transcript.

ClawScribe budgets the request using documented limits for recognized direct API
models, the configured limit for compatible endpoints, or a bounded fallback.
When a provider reports a context or output limit, supported paths try once
with smaller excerpts and a concise final prompt. The saved result shows the
number of excerpts condensed, reduction passes, and any limit retry.

Providers that translate from an English draft preserve each citation's tag
and order. If translation fails, ClawScribe saves the English notes. Select
**Retry translation** to reuse the draft with the same notes settings and
transcript. Editing the notes clears the draft. If the transcript or generation
settings changed, generate notes again.

The summary shows its recorded model and template. Editing the notes preserves
that information. **Transcript changed since generation** means a saved passage,
speaker label, or timestamp has changed since the displayed notes were generated.
Regenerating the notes updates their revision. Older notes may lack revision or
model information. **Follow-up email** shows the draft when the provider included
one in its original response.
