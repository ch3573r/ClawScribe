# Claude Code

Uses the Claude Code you installed and signed in to. Your Claude plan's limits apply.

Claude Code is an optional summary and meeting-chat provider. It is separate from
the existing Claude API-key provider. Claude Code sends the selected meeting text
to its service; this provider is not offline inference.

## Setup

1. Install Claude Code yourself using the [official install guide](https://code.claude.com/docs/en/setup).
   On Windows, the settings card offers `winget install Anthropic.ClaudeCode` to
   copy and run yourself. ClawScribe does not run an installer.
2. Open a terminal and run `claude auth login`. Complete sign-in in Claude Code.
3. In ClawScribe Summary settings, choose **Claude Code** and **Check again**.
4. When ready, choose **Sonnet** (default), **Opus**, or **Haiku**. Claude Code
   resolves these aliases for your account. Save settings before generating notes
   or chat.

ClawScribe does not bundle, download, modify, or redistribute Claude Code. It does
not implement sign-in, read Claude credential files or the keychain, accept a
Claude token, or use the Claude Agent SDK. Authentication stays with the user's
Claude Code installation. This integration is not endorsed by Anthropic.

## Program discovery and status

The program is found through `PATH`, respecting supported Windows `PATHEXT`
executable/shim extensions in order, then the native installer location
`%USERPROFILE%\.local\bin\claude.exe` on Windows or `~/.local/bin/claude` on macOS
and Linux. Empty and relative PATH entries are skipped to avoid launching a
program from an incidental working directory. Under
**Advanced → Program path**, the full path to an existing program overrides
discovery. Windows accepts `.exe`, `.com`, `.cmd`, and `.bat` files; macOS and
Linux require an executable file. Leave it empty to restore discovery. Invalid
paths show an error. Advanced settings are also available when the program is
missing so a mistaken path can be repaired.

**Check again** runs only `--version` (10-second limit) and `auth status --json`
(15-second limit). It does not generate text. Status shows the version and a
recognized plan label, never an email or account identifier. Unknown auth JSON
fails closed. The parser currently accepts explicit `loggedIn` or `authenticated`
booleans at the top level; a false flag wins if they conflict. Optional
`subscriptionType` or `plan` labels are restricted to known subscription names.

Claude Code still loads your own memory files (`CLAUDE.md` in your profile) and
user settings, which can influence summaries. ClawScribe does not read or change
these files or settings.

## Execution and limitations

- Text only: Sonnet, Opus, and Haiku reuse the context and output budgets of the
  Claude API provider. Transcript chunking, language processing, and chat evidence
  budgets continue to apply. There are no user file or audio attachments, tools,
  slash commands, hooks, or MCP tools.
- Each request uses print mode with JSON output. The user prompt goes through
  stdin; system instructions are written as UTF-8 to a file in the request's
  temporary directory and passed with `--system-prompt-file`. Process arguments
  contain only fixed options, the allowed model alias, and the temporary file
  path. Neither prompt is placed on the command line, including for long prompts.
- Each child starts in a fresh temporary directory containing only that system
  prompt file. ClawScribe removes the directory after the child has been reaped.
  The directory is not an OS sandbox.
- Claude Code does not receive ClawScribe's saved Anthropic API key. Inherited
  `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL`,
  `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDECODE`, and every `CLAUDE_CODE_*` variable are
  removed, case-insensitively. The rest of the environment is retained.
- Requests time out after 300 seconds, including a compatibility retry. Cancel
  kills and reaps the owned child. Windows processes are hidden and assigned to
  a kill-on-close job so their descendants are terminated too. After the direct
  child exits, output pipes get up to two seconds to drain; a descendant holding
  a pipe open cannot postpone the result until the request timeout.
- Windows `.cmd` and `.bat` shims use the system `cmd.exe` with AutoRun and delayed
  expansion disabled. Arguments are individually encoded, including empty tools,
  JSON, quotes, percent signs, and paths with spaces. Shim paths containing shell
  expansion characters are rejected. Paths with spaces and non-ASCII text are
  supported. Both prompt texts retain their line breaks and special characters
  through native executables and command shims.
- If an older CLI reports that `--no-session-persistence` is unknown, ClawScribe
  retries once without that option. That version may retain its own session data.
  Upgrade Claude Code to retain the intended no-session-persistence behavior.
- Plan limits and expired sign-in are detected conservatively from explicit error
  results and failure diagnostics, never from successful model output;
  other errors use fixed sanitized messages. CLI output and meeting text are
  never added to diagnostic messages or logs. Output buffers have finite limits.

ClawScribe does not bypass Claude plan limits. To repair sign-in, run
`claude auth login` yourself, then check again. The CLI protocol and wording can
change; an unknown status is an error rather than assumed readiness.
