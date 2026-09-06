# Meeting quality and resource verification

## Unreleased implementation

This work changes chat state, recording lifecycle, transcript updates and resource
handling. Published 0.5.36 installers are unchanged. A future installer needs a new
numeric version. This guide describes verification commands, not a passing run.

### Behaviour to preserve

- Chat waits for history, prevents duplicate sends and send/clear races, preserves
  failed questions, and rejects stale replies after a meeting switch. Native
  per-meeting locking and stable SQLite request IDs protect retries and persistence.
- Long-meeting chat visibly discloses beginning/end excerpts. This is not retrieval
  of every relevant fact from the omitted middle or provider-specific token budgeting.
- Transcript ingestion preserves final corrections and chronological order with
  an indexed append path. Final text is shown immediately, without a typewriter delay.
- The recording clock has a separate subscription; clock ticks do not invalidate
  unrelated lifecycle consumers. Polls do not overlap or overwrite newer events.
- Keyboard-accessible navigation/dialogs, focusable transcript scrolling, readable
  speaker labels and compact transcript/notes views support the smaller minimum
  800 by 560 window. This is not a complete Windows accessibility certification.
- Each raw capture/writer queue is limited to 32 MiB and 4096 items. On overload,
  recording stops and is explicitly marked incomplete. This is NOT lossless overflow
  or a process-wide memory cap. Accepted queued audio drains before finalization;
  final short mixer windows are retained. Failed recordings do not auto-export as
  complete. Checkpoints are retained until merged audio is validated and durable.
- Encoding/finalization run outside async executor threads. Encoder diagnostics are
  bounded and pipe I/O is drained concurrently. A stalled child has a deadline.
- Local-summary prompt decoding uses batches capped at 512 tokens and a conservative
  maximum of four threads. Context overflow is an error, not a successful partial
  answer. No benchmark claim about local model accuracy or target-PC speed follows.

## Automated checks

From the repository root:

```text
cd frontend
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
pnpm build
```

`python scripts/test-quality-core.py` tests actual dependency-light Rust modules
and the chat repository against real in-memory SQLite. It is not the complete
native application. On Windows, stage the pinned helpers/runtime as described in
`windows-release.md`, then run `frontend/scripts/test-windows-native.ps1`. The
required suite list includes the queue, lifecycle, tail, checkpoint and chat cases;
a missing/empty test selection fails rather than pretending validation occurred.

Rendered browser checks live in `frontend/tests/browser`. They bundle the actual
production components and styles, using explicit simulated device/provider
boundaries and system-font fallbacks. They test keyboard focus, destructive-action
recovery, delayed chat responses, recording-clock subscriptions, 3000-row transcript
virtualization and light/dark layouts at 1100x700 and 800x560. Automated axe checks
cover only that fixture surface. They are not screenshots of the entire desktop app.

Install pinned verification tools outside the application dependency tree, set
`UI_TOOLS` to that directory, install Playwright Chromium, then run:

```text
node frontend/tests/browser/browser-checks.cjs .
```

The tool directory needs Playwright 1.56.1, esbuild 0.25.10 and axe-core 4.10.3.
`BROWSER_RESULTS` optionally selects the output directory for JSON and screenshots.
No live provider request, meeting recording, account sign-in or publication occurs.

For the reproducible, isolated transcript algorithm benchmark:

```text
node --expose-gc frontend/tests/performance/transcript-index.bench.mjs
```

It reads and hashes the pinned baseline Git object before comparing actual callback
code with the production index. A shallow checkout must fetch that baseline commit.
Times are cumulative ingestion work, not UI freezes or full-meeting speedups.

## Separate release acceptance

Native build success does not prove real microphone/system-audio capture, accurate
recognition, a stable two-hour meeting, install/upgrade safety, screen-reader
behaviour, or i5-1235U/8 GB performance. Test those on Windows, including pause/stop,
short final speech, device changes, slow disk, recovery and simultaneous model use.
Provider-wide context budgeting, evidence-linked notes and measured model comparisons
remain separate work; review generated notes against the transcript and audio.
