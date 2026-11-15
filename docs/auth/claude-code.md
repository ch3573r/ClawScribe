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
4. When ready, choose `sonnet` (default), `opus`, or `haiku`. Claude Code resolves
   these aliases for your account. Save settings before generating notes or chat.

ClawScribe does not bundle, download, modify, or redistribute Claude Code. It does
not implement sign-in, read Claude credential files or the keychain, accept a
Claude token, or use the Claude Agent SDK. Authentication stays with the user's
Claude Code installation. This integration is not endorsed by Anthropic.

## Program discovery and status

The program is found through `PATH`, respecting supported Windows `PATHEXT`
executable/shim extensions in order, then the native installer location
`%USERPROFILE%\.local\bin\claude.exe`. Empty and relative PATH entries are skipped
to avoid launching a program from an incidental working directory. Under
**Advanced → Program path**, an executable or command shim path overrides
discovery. Leave it empty to restore discovery. Advanced settings are also
available when the program is missing so a mistaken path can be repaired.

**Check again** runs only `--version` (10-second limit) and `auth status --json`
(15-second limit). It does not generate text. Status shows the version and a
recognized plan label, never an email or account identifier. Unknown auth JSON
fails closed. The parser currently accepts explicit `loggedIn` or `authenticated`
booleans at the top level; a false flag wins if they conflict. Optional
`subscriptionType` or `plan` labels are restricted to known subscription names.
This compatibility assumption needs verification against the installed CLI.

## Execution and limitations

- Text only: existing summary context limits, transcript chunking, language
  processing, and chat evidence budgets continue to apply. There are no files,
  audio attachments, tools, slash commands, hooks, or MCP tools.
- Each request uses print mode with JSON output. The prompt is written to stdin;
  only the model alias and system instructions appear in process arguments.
- Each child starts in a fresh empty temporary directory. ClawScribe removes the
  directory after the child has been reaped. Claude Code can still read its own
  normal user settings and sign-in state; the directory is not an OS sandbox.
- Claude Code does not receive ClawScribe's saved Anthropic API key. Inherited
  `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL`,
  `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDECODE`, and every `CLAUDE_CODE_*` variable are
  removed, case-insensitively. The rest of the environment is retained.
- Requests time out after 300 seconds, including a compatibility retry. Cancel
  kills and reaps the owned child. Windows processes are hidden and assigned to
  a kill-on-close job so their descendants are terminated too.
- Windows `.cmd` and `.bat` shims use the system `cmd.exe` with AutoRun and delayed
  expansion disabled. Arguments are individually encoded, including empty tools,
  JSON, quotes, percent signs, and paths with spaces. Shim paths containing shell
  expansion characters are rejected. System-prompt line breaks become spaces for
  command shims because cmd cannot reliably preserve multiline batch arguments;
  user prompt text on stdin is preserved. Overlong shim command lines are rejected
  with advice to choose native `claude.exe` or shorten the system instructions.
  Native executables preserve both prompts without the cmd command-line limit.
- If an older CLI reports that `--no-session-persistence` is unknown, ClawScribe
  retries once without that option. That version may retain its own session data.
  Upgrade Claude Code to retain the intended no-session-persistence behavior.
- Plan limits and expired sign-in are detected conservatively from error words;
  other errors use fixed sanitized messages. CLI output and meeting text are
  never added to diagnostic messages or logs. Output buffers have finite limits.

ClawScribe does not bypass Claude plan limits. To repair sign-in, run
`claude auth login` yourself, then check again. The CLI protocol and wording can
change; an unknown status is an error rather than assumed readiness.
